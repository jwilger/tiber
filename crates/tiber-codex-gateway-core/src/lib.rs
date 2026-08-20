//! Pure policy boundary between the native Codex TUI and a Codex backend.
//!
//! The gateway parses untrusted JSON-RPC once, keeps presentation traffic
//! byte-for-byte intact, and converts backend requests into inert typed data.

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec::Vec};
use core::{error::Error, fmt};

use serde_json::{Map, Value, json};

/// Maximum encoded size of one admitted JSON-RPC message.
const MAX_MESSAGE_BYTES: usize = 0x0010_0000;
/// Maximum admitted structural nesting depth.
const MAX_NESTING_DEPTH: usize = 64;
/// Maximum UTF-8 byte length of one method.
const MAX_METHOD_BYTES: usize = 256;
/// Maximum UTF-8 byte length of one string request identity.
/// Maximum reviewed JSON-RPC and thread/turn identity byte length.
pub const MAX_PROTOCOL_ID_BYTES: usize = 256;
/// Maximum UTF-8 byte length of each Tiber instruction field.
const MAX_INSTRUCTION_BYTES: usize = 0x0001_0000;
/// Maximum number of Tiber-owned dynamic-tool declarations.
const MAX_DYNAMIC_TOOLS: usize = 128;

/// One bounded JSON-RPC message ready for transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedMessage(Vec<u8>);

impl BoundedMessage {
    /// Returns the exact encoded message bytes.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Tiber-owned values inserted into reviewed authority requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayPolicy {
    /// Tiber authority constraints layered over Codex's reviewed defaults.
    developer_instructions: String,
    /// Tiber-owned dynamic-tool declarations.
    dynamic_tools: Vec<Value>,
}

impl GatewayPolicy {
    /// Constructs a bounded authority policy.
    ///
    /// # Errors
    ///
    /// Returns a typed error when instructions or dynamic-tool declarations
    /// exceed the gateway bounds or a tool declaration is not an object.
    #[inline]
    pub fn new<D>(developer: D, dynamic_tools: Vec<Value>) -> Result<Self, GatewayError>
    where
        D: Into<String>,
    {
        let developer_instructions = developer.into();
        if developer_instructions.len() > MAX_INSTRUCTION_BYTES {
            return Err(GatewayError::new(
                "codex_gateway_policy_too_large",
                "Tiber-owned instructions exceed the gateway bound",
                false,
            ));
        }
        if dynamic_tools.len() > MAX_DYNAMIC_TOOLS
            || dynamic_tools.iter().any(|tool| !tool.is_object())
        {
            return Err(GatewayError::new(
                "codex_gateway_dynamic_tools_invalid",
                "dynamic tools must be a bounded list of object declarations",
                false,
            ));
        }
        let mut remaining_policy_bytes = MAX_MESSAGE_BYTES;
        charge_policy_budget(&mut remaining_policy_bytes, 2)?;
        charge_policy_budget(
            &mut remaining_policy_bytes,
            dynamic_tools.len().saturating_sub(1),
        )?;
        for tool in &dynamic_tools {
            preflight_policy_value(tool, 1, &mut remaining_policy_bytes)?;
        }
        let encoded_tools = serde_json::to_vec(&dynamic_tools).map_err(|source| {
            GatewayError::with_source(
                "codex_gateway_dynamic_tools_invalid",
                "dynamic tools could not be encoded",
                false,
                source,
            )
        })?;
        if encoded_tools.len() > MAX_MESSAGE_BYTES {
            return Err(GatewayError::new(
                "codex_gateway_policy_too_large",
                "dynamic-tool declarations exceed the gateway bound",
                false,
            ));
        }
        Ok(Self {
            developer_instructions,
            dynamic_tools,
        })
    }
}

/// Result of routing a native-TUI message toward the backend.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TuiAction {
    /// Forward this bounded message to the backend.
    Forward(
        /// Exact bounded transport bytes.
        BoundedMessage,
    ),
    /// Suspend a user turn until Tiber durably admits its exact prompt.
    TurnStart(
        /// Parsed prompt intent plus the rewritten transport message.
        TurnStartRequest,
    ),
}

/// One inert native Codex turn awaiting application-owned durable admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnStartRequest {
    /// Exact native client request identity.
    id: RequestId,
    /// Exact rewritten message forwarded only after durable admission.
    message: BoundedMessage,
    /// Bounded text prompt represented by the reviewed Codex input schema.
    prompt: String,
    /// Reviewed Codex thread identity owning the turn.
    thread_id: String,
}

impl TurnStartRequest {
    /// Returns the native client request identity.
    #[must_use]
    #[inline]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the rewritten bounded transport message.
    #[must_use]
    #[inline]
    pub const fn message(&self) -> &BoundedMessage {
        &self.message
    }

    /// Returns the exact bounded user prompt.
    #[must_use]
    #[inline]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// Returns the reviewed Codex thread identity.
    #[must_use]
    #[inline]
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }
}

/// Result of routing a backend message toward the native TUI.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackendAction {
    /// Forward the reviewed Codex credential-refresh exchange through Tiber.
    AuthenticationRefresh(
        /// Exact bounded request bytes; token material stays transport-only.
        BoundedMessage,
    ),
    /// Suspend transport and hand an inert request to Tiber policy.
    Effect(
        /// Parsed inert request data.
        EffectRequest,
    ),
    /// Forward byte-identical presentation traffic to the native TUI.
    Forward(
        /// Exact bounded transport bytes.
        BoundedMessage,
    ),
    /// Suspend terminal presentation until Tiber records the observation.
    TurnCompleted(
        /// Bounded assistant text plus the exact presentation message.
        TurnCompleted,
    ),
}

/// One completed native Codex turn awaiting durable observation publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnCompleted {
    /// Bounded final assistant text for a successful turn.
    assistant: Option<String>,
    /// Exact terminal transport message.
    message: BoundedMessage,
    /// Closed reviewed terminal status.
    outcome: TurnOutcome,
    /// Reviewed Codex thread identity owning the terminal turn.
    thread_id: String,
    /// Reviewed Codex terminal turn identity.
    turn_id: String,
}

impl TurnCompleted {
    /// Returns the bounded assistant observation for a successful turn.
    #[must_use]
    #[inline]
    pub fn assistant(&self) -> Option<&str> {
        self.assistant.as_deref()
    }

    /// Returns the exact terminal presentation message.
    #[must_use]
    #[inline]
    pub const fn message(&self) -> &BoundedMessage {
        &self.message
    }

    /// Returns the closed reviewed terminal outcome.
    #[must_use]
    #[inline]
    pub const fn outcome(&self) -> TurnOutcome {
        self.outcome
    }

    /// Returns the reviewed Codex thread identity.
    #[must_use]
    #[inline]
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// Returns the reviewed Codex turn identity.
    #[must_use]
    #[inline]
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }
}

/// Closed terminal outcome vocabulary from the reviewed Codex protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TurnOutcome {
    /// The turn completed with one bounded assistant observation.
    Completed,
    /// The backend failed the turn without a successful observation.
    Failed,
    /// The backend interrupted the turn without a successful observation.
    Interrupted,
}

/// Closed effect-bearing backend request vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EffectKind {
    /// A request to refresh Codex-owned authentication tokens.
    AuthenticationRefresh,
    /// A request to approve command execution.
    CommandApproval,
    /// A model-requested Tiber-declared dynamic tool call.
    DynamicToolCall,
    /// A request to approve a file change.
    FileChangeApproval,
}

/// A bounded JSON-RPC request identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RequestId {
    /// A non-negative integer identity.
    Number(
        /// Exact numeric identity.
        u64,
    ),
    /// A bounded string identity.
    String(
        /// Exact string identity.
        String,
    ),
}

/// Inert typed data for an effect that only Tiber may authorize.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectRequest {
    /// Bounded request identity.
    id: RequestId,
    /// Closed effect classification.
    kind: EffectKind,
    /// Bounded method name.
    method: String,
    /// Bounded untrusted parameters.
    params: Value,
}

impl EffectRequest {
    /// Returns the request identity.
    #[must_use]
    #[inline]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the closed effect classification.
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> EffectKind {
        self.kind
    }

    /// Returns the exact bounded method name.
    #[must_use]
    #[inline]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the bounded untrusted request parameters.
    #[must_use]
    #[inline]
    pub const fn params(&self) -> &Value {
        &self.params
    }
}

/// Stable typed failure from the pure gateway boundary.
#[derive(Debug)]
pub struct GatewayError {
    /// Stable failure code.
    code: &'static str,
    /// Structured bounded failure context.
    context: BTreeMap<&'static str, String>,
    /// Human-readable failure summary.
    message: &'static str,
    /// Whether identical-input retry may succeed.
    retryable: bool,
    /// Retained low-level cause.
    source: Option<Box<dyn Error + Send + Sync>>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "private constructors precede the public inspectors they support"
)]
impl GatewayError {
    /// Constructs a gateway failure without a lower-level cause.
    #[inline]
    fn new(code: &'static str, message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            context: BTreeMap::new(),
            message,
            retryable,
            source: None,
        }
    }

    /// Constructs a gateway failure retaining its lower-level cause.
    #[inline]
    fn with_source<E>(code: &'static str, message: &'static str, retryable: bool, source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self {
            source: Some(Box::new(source)),
            ..Self::new(code, message, retryable)
        }
    }

    /// Adds one structured context value.
    #[inline]
    fn with_context<V>(mut self, key: &'static str, value: V) -> Self
    where
        V: Into<String>,
    {
        self.context.insert(key, value.into());
        self
    }

    /// Returns the stable machine-readable error code.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Returns whether retrying the same input may succeed.
    #[must_use]
    #[inline]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Returns one structured context value.
    #[must_use]
    #[inline]
    pub fn context(&self, key: &str) -> Option<&str> {
        self.context.get(key).map(String::as_str)
    }
}

impl fmt::Display for GatewayError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the default legacy Error methods correctly delegate to Display and source"
)]
impl Error for GatewayError {
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| -> &(dyn Error + 'static) { source })
    }
}

/// Parses and routes one message from the native TUI.
///
/// Thread creation, resumption, forking, and turn admission are reconstructed
/// with Tiber-owned authority fields. Other valid app-server traffic remains
/// byte-identical.
///
/// # Errors
///
/// Returns a typed error for malformed, invalid, or out-of-bounds messages.
#[inline]
pub fn route_tui_message(input: &[u8], policy: &GatewayPolicy) -> Result<TuiAction, GatewayError> {
    let mut message = parse_message(input)?;
    let requested_method = method(&message)?;
    let Some(method) = requested_method else {
        return Ok(TuiAction::Forward(BoundedMessage(input.to_vec())));
    };
    if method == "thread/settings/update" {
        return Err(GatewayError::new(
            "codex_gateway_authority_request_unsupported",
            "the reviewed Codex version does not admit mutable thread settings",
            false,
        ));
    }
    let is_thread_authority = matches!(
        method.as_str(),
        "thread/start" | "thread/resume" | "thread/fork"
    );
    let is_turn_authority = method == "turn/start";
    if !is_thread_authority && !is_turn_authority {
        return Ok(TuiAction::Forward(BoundedMessage(input.to_vec())));
    }
    let turn_request_id = is_turn_authority
        .then(|| parse_request_id(message.get("id")))
        .transpose()?;
    let object = message.as_object_mut().ok_or_else(invalid_message)?;
    let params = object
        .entry("params")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            GatewayError::new(
                "codex_gateway_authority_request_invalid",
                "authority-bearing request params must be an object",
                false,
            )
        })?;
    params.insert("approvalPolicy".into(), json!("never"));
    params.insert("approvalsReviewer".into(), json!("user"));
    if is_thread_authority {
        params.insert("sandbox".into(), json!("read-only"));
        params.insert("config".into(), Value::Object(Map::new()));
        params.remove("baseInstructions");
        params.insert(
            "developerInstructions".into(),
            Value::String(policy.developer_instructions.clone()),
        );
        if method == "thread/start" {
            params.insert(
                "dynamicTools".into(),
                Value::Array(policy.dynamic_tools.clone()),
            );
        } else {
            params.remove("dynamicTools");
        }
    } else {
        params.insert(
            "sandboxPolicy".into(),
            json!({"type": "readOnly", "networkAccess": false}),
        );
    }
    let turn_prompt = (method == "turn/start")
        .then(|| turn_prompt(params))
        .transpose()?;
    let turn_thread_id = is_turn_authority
        .then(|| bounded_identity(params.get("threadId"), "turn thread identity"))
        .transpose()?;
    let encoded = serde_json::to_vec(&message).map_err(|source| {
        GatewayError::with_source(
            "codex_gateway_encode_failed",
            "rewritten authority request could not be encoded",
            false,
            source,
        )
    })?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err(GatewayError::new(
            "codex_gateway_message_too_large",
            "rewritten authority request exceeds the message bound",
            false,
        ));
    }
    let encoded_message = BoundedMessage(encoded);
    if let Some(prompt) = turn_prompt {
        return Ok(TuiAction::TurnStart(TurnStartRequest {
            id: turn_request_id.ok_or_else(invalid_message)?,
            message: encoded_message,
            prompt,
            thread_id: turn_thread_id.ok_or_else(invalid_message)?,
        }));
    }
    Ok(TuiAction::Forward(encoded_message))
}

/// Parses one bounded JSON-RPC identity.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the reviewed request-id parser keeps borrowed wire matching separate from turn authority routing"
)]
fn parse_request_id(value: Option<&Value>) -> Result<RequestId, GatewayError> {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .map(RequestId::Number)
            .ok_or_else(invalid_request_id),
        Some(Value::String(text)) if is_bounded_identity(text) => {
            Ok(RequestId::String(text.clone()))
        }
        _ => Err(invalid_request_id()),
    }
}

/// Parses one bounded protocol identity string.
fn bounded_identity(value: Option<&Value>, label: &'static str) -> Result<String, GatewayError> {
    value
        .and_then(Value::as_str)
        .filter(|text| is_bounded_identity(text))
        .map(str::to_owned)
        .ok_or_else(|| GatewayError::new("codex_gateway_identity_invalid", label, false))
}

/// Returns whether a protocol identity is safe to retain for correlation.
fn is_bounded_identity(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_PROTOCOL_ID_BYTES && !text.chars().any(char::is_control)
}

/// Constructs the stable invalid-request-identity diagnostic.
fn invalid_request_id() -> GatewayError {
    GatewayError::new(
        "codex_gateway_request_id_invalid",
        "request identity must be a bounded string or non-negative integer",
        false,
    )
}

/// Extracts the reviewed text-only prompt without admitting attachment authority.
#[expect(
    clippy::single_call_fn,
    reason = "keeps the reviewed turn-input parser separate from authority rewriting"
)]
fn turn_prompt(params: &Map<String, Value>) -> Result<String, GatewayError> {
    let inputs = params
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            GatewayError::new(
                "codex_gateway_turn_input_invalid",
                "turn input must be a bounded text array",
                false,
            )
        })?;
    let mut prompt = String::new();
    for input in inputs {
        let Some(text) = input.as_object().and_then(|value| {
            (value.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| value.get("text").and_then(Value::as_str))
                .flatten()
        }) else {
            return Err(GatewayError::new(
                "codex_gateway_turn_input_unsupported",
                "native Tiber currently admits only text turn input",
                false,
            ));
        };
        if !prompt.is_empty() {
            prompt.push('\n');
        }
        prompt.push_str(text);
        if prompt.len() > MAX_INSTRUCTION_BYTES {
            return Err(GatewayError::new(
                "codex_gateway_turn_input_too_large",
                "turn text exceeds the durable prompt bound",
                false,
            ));
        }
    }
    if prompt.trim().is_empty() {
        return Err(GatewayError::new(
            "codex_gateway_turn_input_invalid",
            "turn text must not be empty",
            false,
        ));
    }
    Ok(prompt)
}

/// Parses and routes one message from the backend.
///
/// Notifications and responses are presentation pass-through. Every request
/// is effect-bearing: recognized methods become inert typed requests and an
/// unknown method fails closed.
///
/// # Errors
///
/// Returns a typed error for malformed, invalid, out-of-bounds, or unknown
/// effect-bearing messages.
#[inline]
pub fn route_backend_message(input: &[u8]) -> Result<BackendAction, GatewayError> {
    let message = parse_message(input)?;
    let Some(method) = method(&message)? else {
        return Ok(BackendAction::Forward(BoundedMessage(input.to_vec())));
    };
    if method == "turn/completed" {
        return Ok(BackendAction::TurnCompleted(completed_turn(
            &message, input,
        )?));
    }
    let Some(id) = message.get("id") else {
        return Ok(BackendAction::Forward(BoundedMessage(input.to_vec())));
    };
    let request_id = if let Some(number) = id.as_u64() {
        RequestId::Number(number)
    } else if let Some(text) = id.as_str()
        && !text.is_empty()
        && text.len() <= MAX_PROTOCOL_ID_BYTES
        && !text.chars().any(char::is_control)
    {
        RequestId::String(text.to_owned())
    } else {
        return Err(GatewayError::new(
            "codex_gateway_request_id_invalid",
            "effect request identity must be a bounded string or non-negative integer",
            false,
        ));
    };
    let kind = match method.as_str() {
        "item/tool/call" => EffectKind::DynamicToolCall,
        "item/commandExecution/requestApproval" => EffectKind::CommandApproval,
        "item/fileChange/requestApproval" => EffectKind::FileChangeApproval,
        "account/chatgptAuthTokens/refresh" => {
            return Ok(BackendAction::AuthenticationRefresh(BoundedMessage(
                input.to_vec(),
            )));
        }
        _ => {
            return Err(GatewayError::new(
                "codex_gateway_unknown_effect_request",
                "backend requested an effect outside the closed vocabulary",
                false,
            )
            .with_context("method", method));
        }
    };
    Ok(BackendAction::Effect(EffectRequest {
        id: request_id,
        method,
        kind,
        params: message.get("params").cloned().unwrap_or(Value::Null),
    }))
}

/// Extracts one reviewed terminal turn and any successful assistant message.
#[expect(
    clippy::single_call_fn,
    reason = "keeps terminal observation parsing separate from backend request routing"
)]
fn completed_turn(message: &Value, input: &[u8]) -> Result<TurnCompleted, GatewayError> {
    let thread_id = bounded_identity(
        message.pointer("/params/threadId"),
        "turn completion thread identity is invalid",
    )?;
    let turn = message
        .pointer("/params/turn")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            GatewayError::new(
                "codex_gateway_turn_completion_invalid",
                "turn completion must contain a turn object",
                false,
            )
        })?;
    let (assistant, outcome) = match turn.get("status").and_then(Value::as_str) {
        Some("completed") => {
            let assistant = turn
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().rev().find(|item| {
                        item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    })
                })
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= MAX_INSTRUCTION_BYTES)
                .ok_or_else(|| {
                    GatewayError::new(
                        "codex_gateway_turn_observation_invalid",
                        "completed turn must contain one bounded assistant message",
                        false,
                    )
                })?;
            (Some(assistant.to_owned()), TurnOutcome::Completed)
        }
        Some("failed") => (None, TurnOutcome::Failed),
        Some("interrupted") => (None, TurnOutcome::Interrupted),
        _ => {
            return Err(GatewayError::new(
                "codex_gateway_turn_completion_invalid",
                "turn completion must contain a reviewed terminal status",
                false,
            ));
        }
    };
    let turn_id = bounded_identity(turn.get("id"), "turn completion identity is invalid")?;
    Ok(TurnCompleted {
        assistant,
        message: BoundedMessage(input.to_vec()),
        outcome,
        thread_id,
        turn_id,
    })
}

/// Validates effective authority reported by a thread start/resume/fork response.
///
/// # Errors
///
/// Returns a typed failure unless the backend confirms `never` approval, the
/// user-owned approvals reviewer, and the reviewed read-only sandbox mode.
#[inline]
pub fn validate_thread_start_response(input: &[u8]) -> Result<(), GatewayError> {
    let message = parse_message(input)?;
    let matches = message.pointer("/result/approvalPolicy") == Some(&json!("never"))
        && message.pointer("/result/approvalsReviewer") == Some(&json!("user"))
        && message.pointer("/result/sandbox") == Some(&json!("read-only"));
    if !matches {
        return Err(GatewayError::new(
            "codex_gateway_authority_policy_mismatch",
            "backend did not confirm Tiber-owned effective authority policy",
            false,
        ));
    }
    Ok(())
}

/// Charges conservative encoded bytes against the remaining policy budget.
fn charge_policy_budget(remaining: &mut usize, amount: usize) -> Result<(), GatewayError> {
    let Some(updated) = remaining.checked_sub(amount) else {
        return Err(GatewayError::new(
            "codex_gateway_policy_too_large",
            "dynamic-tool declarations exceed the gateway bound",
            false,
        ));
    };
    *remaining = updated;
    Ok(())
}

/// Parses the common bounded JSON-RPC envelope.
fn parse_message(input: &[u8]) -> Result<Value, GatewayError> {
    if input.len() > MAX_MESSAGE_BYTES {
        return Err(GatewayError::new(
            "codex_gateway_message_too_large",
            "JSON-RPC message exceeds the gateway bound",
            false,
        ));
    }
    let value: Value = serde_json::from_slice(input).map_err(|source| {
        GatewayError::with_source(
            "codex_gateway_invalid_json",
            "message is not valid JSON",
            false,
            source,
        )
    })?;
    let object = value.as_object().ok_or_else(invalid_message)?;
    if object
        .get("jsonrpc")
        .is_some_and(|version| version != &json!("2.0"))
        || nesting_depth(&value) > MAX_NESTING_DEPTH
    {
        return Err(invalid_message());
    }
    let has_method = object.contains_key("method");
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if !matches!(
        (has_method, has_result, has_error),
        (true, false, false) | (false, true, false) | (false, false, true)
    ) {
        return Err(invalid_message());
    }
    if !has_method && !object.contains_key("id") {
        return Err(invalid_message());
    }
    Ok(value)
}

/// Preflights one dynamic-tool JSON value before recursive serialization.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "match ergonomics keep bounded JSON traversal readable without moving policy values"
)]
fn preflight_policy_value(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), GatewayError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(GatewayError::new(
            "codex_gateway_policy_too_deep",
            "dynamic-tool declarations exceed the nesting bound",
            false,
        ));
    }
    match value {
        Value::Null => charge_policy_budget(remaining, 4),
        Value::Bool(_) => charge_policy_budget(remaining, 5),
        Value::Number(_) => charge_policy_budget(remaining, 32),
        Value::String(text) => {
            charge_policy_budget(remaining, text.len().saturating_mul(6).saturating_add(2))
        }
        Value::Array(values) => {
            charge_policy_budget(remaining, values.len().saturating_add(1))?;
            let next_depth = depth.saturating_add(1);
            for child in values {
                preflight_policy_value(child, next_depth, remaining)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            charge_policy_budget(remaining, values.len().saturating_add(1))?;
            let next_depth = depth.saturating_add(1);
            for (key, child) in values {
                charge_policy_budget(remaining, key.len().saturating_mul(6).saturating_add(3))?;
                preflight_policy_value(child, next_depth, remaining)?;
            }
            Ok(())
        }
    }
}

/// Parses the optional bounded method name.
fn method(message: &Value) -> Result<Option<String>, GatewayError> {
    let Some(value) = message.get("method") else {
        return Ok(None);
    };
    let method = value.as_str().ok_or_else(invalid_message)?;
    if method.is_empty() || method.len() > MAX_METHOD_BYTES || method.chars().any(char::is_control)
    {
        return Err(invalid_message());
    }
    Ok(Some(method.to_owned()))
}

/// Computes the structural JSON nesting depth using saturating arithmetic.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "match ergonomics keep recursive JSON traversal readable without moving values"
)]
fn nesting_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => values
            .iter()
            .map(nesting_depth)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        Value::Object(values) => values
            .values()
            .map(nesting_depth)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 0,
    }
}

/// Constructs the stable invalid-envelope failure.
fn invalid_message() -> GatewayError {
    GatewayError::new(
        "codex_gateway_message_invalid",
        "message is not a valid bounded Codex app-server envelope",
        false,
    )
}
