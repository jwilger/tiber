//! Private Unix-domain WebSocket transport for the native Codex TUI.
//!
//! The transport has one authority path: every JSON-RPC data frame crosses the
//! pure gateway core before it can reach the opposite peer. Backend effect
//! requests are delivered as inert typed values to the owning application.

extern crate alloc;

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::{error::Error, fmt, str::from_utf8};
use std::{
    fs,
    io::ErrorKind,
    os::unix::{
        fs::{FileTypeExt as _, PermissionsExt as _},
        net::UnixStream as BlockingUnixStream,
    },
    path::{Path, PathBuf},
};

use futures::{Sink, SinkExt as _, StreamExt as _};
use serde_json::Value;
use tiber_codex_gateway_core::{
    BackendAction, EffectKind, EffectRequest, GatewayPolicy, RequestId, TuiAction,
    route_backend_message, route_tui_message, validate_thread_start_response,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, watch},
};
use tokio_tungstenite::{
    accept_async_with_config, client_async_with_config,
    tungstenite::{Error as WebSocketError, Message, protocol::WebSocketConfig},
};

/// Maximum admitted WebSocket message and frame payload.
const MAX_MESSAGE_BYTES: usize = 0x0010_0000;

/// Configuration for one private, single-client gateway session.
#[derive(Debug)]
pub struct GatewayConfig {
    /// Private socket exposed to the native TUI.
    listen_path: PathBuf,
    /// Tiber-owned protocol policy.
    policy: GatewayPolicy,
    /// Private socket exposed by Codex app-server.
    upstream_path: PathBuf,
}

/// A bound gateway ready for the native Codex TUI to connect.
#[derive(Debug)]
pub struct Gateway {
    /// Immutable connection policy and paths.
    config: GatewayConfig,
    /// Bounded application effect channel.
    effects: mpsc::Sender<EffectCall>,
    /// Bound readiness boundary.
    listener: UnixListener,
    /// Removes the owned socket pathname on drop.
    socket_guard: SocketGuard,
}

/// One inert backend effect plus its application-owned completion capability.
#[derive(Debug)]
pub struct EffectCall {
    /// Single-use completion capability.
    completion: oneshot::Sender<EffectResponse>,
    /// Inert request inspected by application policy.
    request: EffectRequest,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the request inspector precedes the consuming completion operation"
)]
impl EffectCall {
    /// Returns the bounded effect request inspected by application policy.
    #[must_use]
    #[inline]
    pub const fn request(&self) -> &EffectRequest {
        &self.request
    }

    /// Completes the request with an application-created bounded response.
    ///
    /// # Errors
    ///
    /// Returns the response when the gateway session has already stopped.
    #[inline]
    pub fn complete(self, response: EffectResponse) -> Result<(), EffectResponse> {
        if response
            .kind
            .is_some_and(|kind| kind != self.request.kind())
        {
            return Err(response);
        }
        self.completion.send(response)
    }
}

/// A bounded application-owned result for an intercepted backend effect.
#[derive(Debug)]
pub struct EffectResponse {
    /// Effect kind this successful result may complete.
    kind: Option<EffectKind>,
    /// Bounded result or error payload.
    payload: EffectPayload,
}

#[derive(Debug)]
/// Closed wire payload produced only after application policy completes an effect.
enum EffectPayload {
    /// JSON-RPC error response.
    Error {
        /// Stable application-selected JSON-RPC error code.
        code: i64,
        /// Optional bounded diagnostic data.
        data: Option<Value>,
        /// Bounded human-readable summary.
        message: String,
    },
    /// Successful bounded JSON-RPC result.
    Result(Value),
}

#[expect(
    clippy::missing_errors_doc,
    reason = "all public constructors share the documented bounded-response failure contract"
)]
impl EffectResponse {
    /// Constructs a bounded authentication-refresh result.
    #[inline]
    pub fn authentication_refresh(result: Value) -> Result<Self, TransportError> {
        Self::success(EffectKind::AuthenticationRefresh, result)
    }

    /// Constructs a bounded command-approval result.
    #[inline]
    pub fn command_approval(result: Value) -> Result<Self, TransportError> {
        Self::success(EffectKind::CommandApproval, result)
    }

    /// Constructs a bounded dynamic-tool result.
    #[inline]
    pub fn dynamic_tool_call(result: Value) -> Result<Self, TransportError> {
        Self::success(EffectKind::DynamicToolCall, result)
    }

    /// Constructs a bounded JSON-RPC error applicable to any effect kind.
    #[inline]
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "borrowed optional diagnostic data avoids cloning"
    )]
    pub fn failure<M>(code: i64, summary: M, data: Option<Value>) -> Result<Self, TransportError>
    where
        M: Into<String>,
    {
        let message = summary.into();
        if message.len() > 4096 {
            return Err(TransportError::new("codex_gateway_effect_error_too_large"));
        }
        if let Some(value) = &data {
            validate_value(value)?;
        }
        Ok(Self {
            kind: None,
            payload: EffectPayload::Error {
                code,
                data,
                message,
            },
        })
    }

    /// Constructs a bounded file-change-approval result.
    #[inline]
    pub fn file_change_approval(result: Value) -> Result<Self, TransportError> {
        Self::success(EffectKind::FileChangeApproval, result)
    }

    /// Constructs one kind-bound successful JSON-RPC result value.
    ///
    /// # Errors
    ///
    /// Rejects values whose depth or encoded size exceeds transport bounds.
    #[inline]
    fn success(kind: EffectKind, result: Value) -> Result<Self, TransportError> {
        validate_value(&result)?;
        Ok(Self {
            kind: Some(kind),
            payload: EffectPayload::Result(result),
        })
    }
}

impl GatewayConfig {
    /// Creates a gateway configuration.
    #[must_use]
    #[inline]
    pub fn new<L, U>(listen_path: L, upstream_path: U, policy: GatewayPolicy) -> Self
    where
        L: Into<PathBuf>,
        U: Into<PathBuf>,
    {
        Self {
            listen_path: listen_path.into(),
            upstream_path: upstream_path.into(),
            policy,
        }
    }
}

/// A stable transport failure.
#[derive(Debug)]
pub struct TransportError {
    /// Stable machine-readable code.
    code: &'static str,
    /// Retained low-level cause.
    source: Option<Box<dyn Error + Send + Sync>>,
}

/// Removes the private socket owned by one bound gateway.
#[derive(Debug)]
struct SocketGuard(PathBuf);

impl TransportError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Constructs a failure without a lower-level cause.
    #[inline]
    fn new(code: &'static str) -> Self {
        Self { code, source: None }
    }

    /// Constructs a failure retaining its lower-level cause.
    #[inline]
    fn with_source<E>(code: &'static str, source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for TransportError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "default legacy Error methods correctly delegate to Display and source"
)]
impl Error for TransportError {
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| -> &(dyn Error + 'static) { source })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.0));
    }
}

impl Gateway {
    /// Binds the private socket, providing an explicit readiness boundary.
    ///
    /// # Errors
    ///
    /// Fails when the path is occupied or private permissions cannot be set.
    #[inline]
    pub fn bind(
        config: GatewayConfig,
        effects: mpsc::Sender<EffectCall>,
    ) -> Result<Self, TransportError> {
        let listener = bind_private(&config.listen_path)?;
        let socket_guard = SocketGuard(config.listen_path.clone());
        Ok(Self {
            config,
            effects,
            listener,
            socket_guard,
        })
    }

    /// Accepts one TUI connection and bridges until close or cancellation.
    ///
    /// # Errors
    ///
    /// Fails closed on handshake, routing, authority, or I/O failure.
    #[expect(
        clippy::integer_division_remainder_used,
        reason = "tokio::select macro internals use remainder arithmetic"
    )]
    #[inline]
    pub async fn serve_one(
        mut self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), TransportError> {
        let _socket_guard = &mut self.socket_guard;
        if *shutdown.borrow() {
            return Ok(());
        }
        let accepted = tokio::select! {
            accepted = self.listener.accept() => accepted,
            () = cancelled(&mut shutdown) => return Ok(()),
        };
        let (tui_stream, _) = accepted
            .map_err(|source| TransportError::with_source("codex_gateway_accept_failed", source))?;
        let tui = tokio::select! {
            result = accept_async_with_config(tui_stream, Some(websocket_config())) => result.map_err(|source| TransportError::with_source("codex_gateway_tui_websocket_failed", source))?,
            () = cancelled(&mut shutdown) => return Ok(()),
        };
        let upstream_stream = tokio::select! {
            result = UnixStream::connect(&self.config.upstream_path) => result.map_err(|source| TransportError::with_source("codex_gateway_upstream_connect_failed", source))?,
            () = cancelled(&mut shutdown) => return Ok(()),
        };
        let (upstream, _) = tokio::select! {
            result = client_async_with_config("ws://localhost/", upstream_stream, Some(websocket_config())) => result.map_err(|source| TransportError::with_source("codex_gateway_upstream_websocket_failed", source))?,
            () = cancelled(&mut shutdown) => return Ok(()),
        };
        bridge(
            tui,
            upstream,
            &self.config.policy,
            self.effects,
            &mut shutdown,
        )
        .await
    }
}

/// Serves exactly one native-TUI connection and returns after either peer closes.
///
/// `effects` must be a bounded application-owned channel. Backpressure on that
/// channel suspends backend reads, so effects cannot bypass the application or
/// accumulate in an unbounded transport queue.
///
/// # Errors
///
/// Fails closed when socket setup, WebSocket negotiation, core routing,
/// authority validation, or effect delivery fails.
#[inline]
pub async fn serve_one(
    config: GatewayConfig,
    effects: mpsc::Sender<EffectCall>,
) -> Result<(), TransportError> {
    let gateway = Gateway::bind(config, effects)?;
    let (_shutdown_sender, shutdown) = watch::channel(false);
    gateway.serve_one(shutdown).await
}

/// Binds a private socket after recovering an ownerless stale pathname.
#[expect(
    clippy::single_call_fn,
    reason = "separates filesystem recovery from session setup"
)]
fn bind_private(path: &Path) -> Result<UnixListener, TransportError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|source| {
            TransportError::with_source("codex_gateway_socket_inspection_failed", source)
        })?;
        if !metadata.file_type().is_socket() {
            return Err(TransportError::new("codex_gateway_socket_exists"));
        }
        match BlockingUnixStream::connect(path) {
            Ok(_) => return Err(TransportError::new("codex_gateway_socket_active")),
            Err(source) if source.kind() == ErrorKind::ConnectionRefused => {
                fs::remove_file(path).map_err(|removal_source| {
                    TransportError::with_source(
                        "codex_gateway_stale_socket_removal_failed",
                        removal_source,
                    )
                })?;
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TransportError::with_source(
                    "codex_gateway_socket_probe_failed",
                    source,
                ));
            }
        }
    }
    let listener = UnixListener::bind(path)
        .map_err(|source| TransportError::with_source("codex_gateway_bind_failed", source))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        TransportError::with_source("codex_gateway_permissions_failed", source)
    })?;
    Ok(listener)
}

/// Routes one connected pair until close or cancellation.
#[expect(
    clippy::cognitive_complexity,
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "one select loop visibly owns the closed bidirectional authority boundary; tokio macro internals use remainder arithmetic"
)]
async fn bridge(
    tui: tokio_tungstenite::WebSocketStream<UnixStream>,
    upstream: tokio_tungstenite::WebSocketStream<UnixStream>,
    policy: &GatewayPolicy,
    effects: mpsc::Sender<EffectCall>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), TransportError> {
    let (mut tui_sink, mut tui_stream) = tui.split();
    let (mut upstream_sink, mut upstream_stream) = upstream.split();
    let mut pending_thread_authority: Option<Value> = None;
    loop {
        tokio::select! {
            () = cancelled(shutdown) => break,
            from_tui = tui_stream.next() => {
                let Some(frame_result) = from_tui else { break };
                let frame = frame_result.map_err(|source| TransportError::with_source("codex_gateway_tui_read_failed", source))?;
                if frame.is_close() {
                    send_frame(&mut upstream_sink, frame, shutdown, "codex_gateway_upstream_write_failed").await?;
                    break;
                }
                if frame.is_ping() || frame.is_pong() {
                    if !send_frame(&mut upstream_sink, frame, shutdown, "codex_gateway_upstream_write_failed").await? { break; }
                    continue;
                }
                let bytes = data_bytes(&frame)?;
                let parsed: Value = serde_json::from_slice(bytes).map_err(|source| TransportError::with_source("codex_gateway_invalid_json", source))?;
                if parsed.get("method").and_then(Value::as_str).is_some_and(|method| {
                    matches!(method, "thread/start" | "thread/resume" | "thread/fork")
                }) {
                    if pending_thread_authority.is_some() {
                        return Err(TransportError::new("codex_gateway_thread_start_already_pending"));
                    }
                    pending_thread_authority = Some(thread_start_id(&parsed)?);
                }
                let TuiAction::Forward(message) = route_tui_message(bytes, policy)
                    .map_err(|source| TransportError::with_source(source.code(), source))?
                else {
                    return Err(TransportError::new("codex_gateway_unknown_tui_action"));
                };
                if !send_frame(&mut upstream_sink, Message::Text(text(message.as_bytes())?.into()), shutdown, "codex_gateway_upstream_write_failed").await? { break; }
            }
            from_upstream = upstream_stream.next() => {
                let Some(frame_result) = from_upstream else { break };
                let frame = frame_result.map_err(|source| TransportError::with_source("codex_gateway_upstream_read_failed", source))?;
                if frame.is_close() {
                    send_frame(&mut tui_sink, frame, shutdown, "codex_gateway_tui_write_failed").await?;
                    break;
                }
                if frame.is_ping() || frame.is_pong() {
                    if !send_frame(&mut tui_sink, frame, shutdown, "codex_gateway_tui_write_failed").await? { break; }
                    continue;
                }
                let bytes = data_bytes(&frame)?;
                match route_backend_message(bytes).map_err(|source| TransportError::with_source(source.code(), source))? {
                    BackendAction::AuthenticationRefresh(message) => {
                        if !send_frame(&mut tui_sink, Message::Text(text(message.as_bytes())?.into()), shutdown, "codex_gateway_tui_write_failed").await? { break; }
                    }
                    BackendAction::Effect(effect) => {
                        let id = request_id_value(effect.id());
                        let (completion, completed) = oneshot::channel();
                        let delivery = tokio::select! {
                            result = effects.send(EffectCall { completion, request: effect }) => Some(result),
                            () = cancelled(shutdown) => None,
                        };
                        let Some(delivery_result) = delivery else { break };
                        delivery_result.map_err(|_closed| TransportError::new("codex_gateway_effect_receiver_closed"))?;
                        let response = tokio::select! {
                            response = completed => response.map_err(|_dropped| TransportError::new("codex_gateway_effect_completion_dropped"))?,
                            () = cancelled(shutdown) => break,
                        };
                        let envelope = match response.payload {
                            EffectPayload::Result(result) => serde_json::json!({
                                "id": id, "result": result,
                            }),
                            EffectPayload::Error { code, data, message } => serde_json::json!({
                                "id": id,
                                "error": {"code": code, "message": message, "data": data},
                            }),
                        };
                        let encoded = serde_json::to_vec(&envelope).map_err(|source| TransportError::with_source("codex_gateway_effect_response_invalid", source))?;
                        if encoded.len() > MAX_MESSAGE_BYTES {
                            return Err(TransportError::new("codex_gateway_effect_response_too_large"));
                        }
                        if !send_frame(&mut upstream_sink, Message::Text(text(&encoded)?.into()), shutdown, "codex_gateway_upstream_write_failed").await? { break; }
                    }
                    BackendAction::Forward(message) => {
                        if response_matches(message.as_bytes(), pending_thread_authority.as_ref())? {
                            validate_thread_start_response(message.as_bytes())
                                .map_err(|source| TransportError::with_source(source.code(), source))?;
                            pending_thread_authority = None;
                        }
                        if !send_frame(&mut tui_sink, Message::Text(text(message.as_bytes())?.into()), shutdown, "codex_gateway_tui_write_failed").await? { break; }
                    }
                    _ => return Err(TransportError::new("codex_gateway_unknown_backend_action")),
                }
            }
        }
    }
    Ok(())
}

/// Waits until cancellation is asserted or every sender disappears.
async fn cancelled(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() || shutdown.changed().await.is_err() {
            return;
        }
    }
}

/// Converts a core-owned request identity into an application response identity.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "keeps non-exhaustive identity handling fail closed"
)]
fn request_id_value(id: &RequestId) -> Value {
    match id {
        RequestId::Number(number) => Value::from(*number),
        RequestId::String(text) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

/// Preflights one application result without recursive traversal.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "borrowed JSON traversal is explicit and isolated from transport flow"
)]
fn validate_value(root: &Value) -> Result<(), TransportError> {
    let mut pending: Vec<(&Value, usize)> = vec![(root, 0)];
    while let Some((current, depth)) = pending.pop() {
        if depth > 64 {
            return Err(TransportError::new(
                "codex_gateway_effect_response_too_deep",
            ));
        }
        match current {
            Value::Array(values) => {
                pending.extend(values.iter().map(|child| (child, depth.saturating_add(1))));
            }
            Value::Object(values) => pending.extend(
                values
                    .values()
                    .map(|child| (child, depth.saturating_add(1))),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    let encoded = serde_json::to_vec(root).map_err(|source| {
        TransportError::with_source("codex_gateway_effect_response_invalid", source)
    })?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err(TransportError::new(
            "codex_gateway_effect_response_too_large",
        ));
    }
    Ok(())
}

/// Extracts the mandatory correlatable identity of a thread start.
#[expect(
    clippy::single_call_fn,
    reason = "keeps thread authority correlation validation isolated"
)]
fn thread_start_id(message: &Value) -> Result<Value, TransportError> {
    let Some(id) = message.get("id") else {
        return Err(TransportError::new("codex_gateway_thread_start_id_invalid"));
    };
    if id.as_u64().is_some() {
        return Ok(id.clone());
    }
    let Some(text) = id.as_str() else {
        return Err(TransportError::new("codex_gateway_thread_start_id_invalid"));
    };
    if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control) {
        return Err(TransportError::new("codex_gateway_thread_start_id_invalid"));
    }
    Ok(id.clone())
}

/// Constructs bounded WebSocket buffering limits matching the core boundary.
fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(0x0001_0000)
        .write_buffer_size(0x0001_0000)
        .max_write_buffer_size(0x0011_0000)
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Returns data bytes while rejecting unsupported control-state frames.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "borrowed frame matching avoids copies"
)]
fn data_bytes(frame: &Message) -> Result<&[u8], TransportError> {
    match frame {
        Message::Text(text) => Ok(text.as_bytes()),
        Message::Binary(bytes) => Ok(bytes),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {
            Err(TransportError::new("codex_gateway_unexpected_frame"))
        }
    }
}

/// Requires a UTF-8 JSON-RPC text message.
fn text(bytes: &[u8]) -> Result<&str, TransportError> {
    from_utf8(bytes)
        .map_err(|source| TransportError::with_source("codex_gateway_non_text_message", source))
}

/// Identifies the response to the outstanding rewritten thread start.
#[expect(
    clippy::single_call_fn,
    reason = "keeps authority-response correlation explicit"
)]
fn response_matches(bytes: &[u8], pending_id: Option<&Value>) -> Result<bool, TransportError> {
    let Some(expected_id) = pending_id else {
        return Ok(false);
    };
    let message: Value = serde_json::from_slice(bytes)
        .map_err(|source| TransportError::with_source("codex_gateway_invalid_json", source))?;
    Ok(message.get("method").is_none() && message.get("id") == Some(expected_id))
}

/// Sends one frame unless cancellation wins first.
#[expect(
    clippy::integer_division_remainder_used,
    reason = "tokio::select macro internals use remainder arithmetic"
)]
async fn send_frame<S>(
    sink: &mut S,
    frame: Message,
    shutdown: &mut watch::Receiver<bool>,
    code: &'static str,
) -> Result<bool, TransportError>
where
    S: Sink<Message, Error = WebSocketError> + Unpin,
{
    tokio::select! {
        result = sink.send(frame) => {
            result.map_err(|source| TransportError::with_source(code, source))?;
            Ok(true)
        }
        () = cancelled(shutdown) => Ok(false),
    }
}
