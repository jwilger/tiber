//! Harness-owned, bounded adapters for configured RMCP integrations.
//!
//! Authority is decided by `tiber-external-tools-core`. This crate accepts
//! only its opaque authorization tokens and interprets them over one bounded
//! direct-argv stdio process or one bounded loopback Streamable HTTP session.

extern crate alloc;

use alloc::{collections::BTreeSet, sync::Arc, vec::Vec};
use core::{
    error::Error,
    fmt, future, mem,
    pin::Pin,
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll},
    time::Duration,
};
use std::{collections::HashMap, io, process::Stdio};

use bytes::Bytes;
use futures::{Sink, SinkExt as _, Stream, StreamExt as _, channel::mpsc, sink, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
    redirect::Policy,
    retry,
};
use rmcp::{
    ClientHandler, RoleClient, ServiceExt as _,
    model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
        ClientJsonRpcMessage, ClientRequest, CustomNotification, CustomRequest, CustomResult,
        ElicitRequestParams, ElicitResult, ElicitationCreateRequestMethod, ErrorCode, ErrorData,
        Extensions, GetPromptRequest, GetPromptRequestParams, Implementation, JsonObject,
        JsonRpcMessage, ListPromptsRequest, ListPromptsRequestMethod, ListResourcesRequest,
        ListResourcesRequestMethod, ListToolsRequest, ListToolsRequestMethod,
        PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion, ReadResourceRequest,
        ReadResourceRequestParams, ResourceUpdatedNotificationParam, RootsCapabilities,
        ServerJsonRpcMessage, ServerResult, SubscriptionsAcknowledgedNotificationParams,
        TaskStatusNotificationParams,
    },
    service::{
        NotificationContext, PeerRequestOptions, RequestContext, RequestHandle, RunningService,
        RxJsonRpcMessage, TxJsonRpcMessage,
    },
    transport::{
        IntoTransport, StreamableHttpClientTransport,
        common::client_side_sse::NeverRetry,
        sink_stream::SinkStreamTransport,
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpClientTransportConfig,
            StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};
use tiber_external_tools_core::{
    AuthorizedPromptGet, AuthorizedPromptListing, AuthorizedReconciliation,
    AuthorizedResourceListing, AuthorizedResourceRead, AuthorizedRootDeclaration,
    AuthorizedServerObservation, AuthorizedToolCall, AuthorizedToolListing,
    BoundReconciliationFailure, BoundReconciliationOutcome, BoundToolCallFailure,
    BoundToolCallOutcome, LiteralArgument, MAX_CONFIGURED_TOOLS, MAX_SEMANTIC_TEXT_BYTES,
    McpIntegration, McpTransport, ReconciliationOutcome, ServerObservationKind, ToolArguments,
    ToolClass, ToolName, UntrustedPayload,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    process::{Child, Command},
    sync::mpsc as tokio_mpsc,
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;

/// Maximum newline-delimited JSON-RPC frame accepted from a stdio server.
const MAX_STDIO_MESSAGE_BYTES: usize = 128 * 1024;
/// Maximum raw JSON response body or SSE event accepted from a local HTTP server.
const MAX_HTTP_RESPONSE_BYTES: usize = 128 * 1024;
/// `MAX_HTTP_RESPONSE_BYTES` represented in a trusted HTTP content-length domain.
const MAX_HTTP_RESPONSE_BYTES_U64: u64 = 128 * 1024;
/// Maximum incremental read used while discarding a hostile oversized frame.
const STDIO_READ_CHUNK_BYTES: usize = 4 * 1024;
/// Maximum server pages accepted for one tool-list operation.
const MAX_TOOL_LIST_PAGES: usize = 16;
/// Maximum untrusted resources or prompts retained from one optional catalog operation.
const MAX_OPTIONAL_CATALOG_ITEMS: usize = MAX_CONFIGURED_TOOLS;
/// Maximum server pages accepted for one optional resource or prompt catalog operation.
const MAX_OPTIONAL_CATALOG_PAGES: usize = MAX_TOOL_LIST_PAGES;
/// Maximum retained notifications for each explicit, separately authorized kind.
const MAX_PENDING_OBSERVATIONS_PER_KIND: usize = 16;
/// A bounded grace period for cancellation and child cleanup.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// A bounded, untrusted notification observed from a connected tool server.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "new server notifications require an explicit bounded projection decision and follow policy flow"
)]
pub enum ServerObservation {
    /// The server announced that its untrusted prompt catalog may have changed.
    PromptListChanged,
    /// The server announced that its untrusted resource catalog may have changed.
    ResourceListChanged,
    /// A bounded untrusted server resource-update notification.
    ResourceUpdated(UntrustedPayload),
    /// The server announced that its untrusted tool catalog may have changed.
    ToolListChanged,
    /// A bounded untrusted server progress notification.
    Progress(UntrustedPayload),
    /// A bounded untrusted server logging notification.
    Logging(UntrustedPayload),
}

impl ServerObservation {
    /// Returns the precise policy kind that gates delivery of this observation.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the total projection is clearest as a tail match over a borrowed observation"
    )]
    const fn kind(&self) -> ServerObservationKind {
        match self {
            Self::PromptListChanged => ServerObservationKind::PromptListChanged,
            Self::ResourceListChanged => ServerObservationKind::ResourceListChanged,
            Self::ResourceUpdated(_) => ServerObservationKind::ResourceUpdated,
            Self::ToolListChanged => ServerObservationKind::ToolListChanged,
            Self::Progress(_) => ServerObservationKind::Progress,
            Self::Logging(_) => ServerObservationKind::Logging,
        }
    }
}

/// Fixed-capacity ingress senders partitioned by explicit policy kind.
#[derive(Clone)]
struct ObservationSenders {
    /// Bounded logging ingress.
    logging: tokio_mpsc::Sender<ServerObservation>,
    /// Bounded progress ingress.
    progress: tokio_mpsc::Sender<ServerObservation>,
    /// Bounded prompt-catalog ingress.
    prompt_list_changed: tokio_mpsc::Sender<ServerObservation>,
    /// Bounded resource-catalog ingress.
    resource_list_changed: tokio_mpsc::Sender<ServerObservation>,
    /// Bounded resource-update ingress.
    resource_updated: tokio_mpsc::Sender<ServerObservation>,
    /// Bounded tool-catalog ingress.
    tool_list_changed: tokio_mpsc::Sender<ServerObservation>,
}

/// Fixed-capacity ingress receivers partitioned by explicit policy kind.
struct ObservationReceivers {
    /// Bounded logging observations awaiting an explicit logging token.
    logging: tokio_mpsc::Receiver<ServerObservation>,
    /// Bounded progress observations awaiting an explicit progress token.
    progress: tokio_mpsc::Receiver<ServerObservation>,
    /// Bounded prompt-catalog observations awaiting an explicit prompt token.
    prompt_list_changed: tokio_mpsc::Receiver<ServerObservation>,
    /// Bounded resource-catalog observations awaiting an explicit resource token.
    resource_list_changed: tokio_mpsc::Receiver<ServerObservation>,
    /// Bounded resource-update observations awaiting an explicit resource token.
    resource_updated: tokio_mpsc::Receiver<ServerObservation>,
    /// Bounded tool-catalog observations awaiting an explicit list-change token.
    tool_list_changed: tokio_mpsc::Receiver<ServerObservation>,
}

#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "channel construction and kind-directed delivery follow the notification data flow"
)]
impl ObservationSenders {
    /// Creates one independent bounded channel for every policy kind.
    fn channels() -> (Self, ObservationReceivers) {
        let (logging_sender, logging_receiver) =
            tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        let (progress_sender, progress_receiver) =
            tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        let (prompt_sender, prompt_receiver) =
            tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        let (resource_list_sender, resource_list_receiver) =
            tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        let (resource_updated_sender, resource_updated_receiver) =
            tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        let (tool_sender, tool_receiver) = tokio_mpsc::channel(MAX_PENDING_OBSERVATIONS_PER_KIND);
        (
            Self {
                logging: logging_sender,
                progress: progress_sender,
                prompt_list_changed: prompt_sender,
                resource_list_changed: resource_list_sender,
                resource_updated: resource_updated_sender,
                tool_list_changed: tool_sender,
            },
            ObservationReceivers {
                logging: logging_receiver,
                progress: progress_receiver,
                prompt_list_changed: prompt_receiver,
                resource_list_changed: resource_list_receiver,
                resource_updated: resource_updated_receiver,
                tool_list_changed: tool_receiver,
            },
        )
    }

    /// Enqueues one observation only in its independently bounded kind partition.
    fn try_send(
        &self,
        observation: ServerObservation,
    ) -> Result<(), tokio_mpsc::error::TrySendError<ServerObservation>> {
        match observation.kind() {
            ServerObservationKind::PromptListChanged => {
                self.prompt_list_changed.try_send(observation)
            }
            ServerObservationKind::ResourceListChanged => {
                self.resource_list_changed.try_send(observation)
            }
            ServerObservationKind::ResourceUpdated => self.resource_updated.try_send(observation),
            ServerObservationKind::ToolListChanged => self.tool_list_changed.try_send(observation),
            ServerObservationKind::Progress => self.progress.try_send(observation),
            ServerObservationKind::Logging => self.logging.try_send(observation),
        }
    }
}

#[expect(
    clippy::implicit_return,
    reason = "the exact authorized kind directly selects its independent bounded receiver"
)]
impl ObservationReceivers {
    /// Removes the oldest buffered observation of one explicitly authorized kind.
    fn try_recv(&mut self, kind: ServerObservationKind) -> Option<ServerObservation> {
        match kind {
            ServerObservationKind::PromptListChanged => self.prompt_list_changed.try_recv().ok(),
            ServerObservationKind::ResourceListChanged => {
                self.resource_list_changed.try_recv().ok()
            }
            ServerObservationKind::ResourceUpdated => self.resource_updated.try_recv().ok(),
            ServerObservationKind::ToolListChanged => self.tool_list_changed.try_recv().ok(),
            ServerObservationKind::Progress => self.progress.try_recv().ok(),
            ServerObservationKind::Logging => self.logging.try_recv().ok(),
        }
    }
}

/// Cancellation and deadline controls for one bounded protocol operation.
#[derive(Clone, Debug)]
pub struct RequestOptions {
    /// Cancellation signal owned by the harness invocation.
    cancellation: CancellationToken,
    /// Total budget for the public adapter operation.
    timeout: Duration,
}

#[expect(
    clippy::implicit_return,
    reason = "the small value constructor is clearer as an idiomatic tail expression"
)]
impl RequestOptions {
    /// Creates controls for one operation.
    #[must_use]
    #[inline]
    pub fn new(timeout: Duration, cancellation: CancellationToken) -> Self {
        Self {
            cancellation,
            timeout,
        }
    }
}

/// A known configured tool observed in untrusted server metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedTool {
    /// Bounded untrusted server description, when present.
    description: Option<UntrustedPayload>,
    /// Bounded untrusted server input schema.
    input_schema: UntrustedPayload,
    /// Trusted configured name that matched the untrusted metadata.
    name: ToolName,
    /// Bounded untrusted server output schema, when present.
    output_schema: Option<UntrustedPayload>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "accessors are ordered from trusted identity to untrusted metadata use"
)]
impl ListedTool {
    /// Returns the trusted configured name matched against this server metadata.
    #[must_use]
    #[inline]
    pub fn name(&self) -> &ToolName {
        &self.name
    }

    /// Returns the bounded, untrusted server description when one was supplied.
    #[must_use]
    #[inline]
    pub fn description(&self) -> Option<&UntrustedPayload> {
        self.description.as_ref()
    }

    /// Returns the bounded, untrusted server input schema.
    #[must_use]
    #[inline]
    pub fn input_schema(&self) -> &UntrustedPayload {
        &self.input_schema
    }

    /// Returns the bounded, untrusted server output schema when one was supplied.
    #[must_use]
    #[inline]
    pub fn output_schema(&self) -> Option<&UntrustedPayload> {
        self.output_schema.as_ref()
    }
}

/// Whether retrying a bounded RMCP adapter failure can plausibly change its outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the retryable outcome is presented first to communicate recovery policy"
)]
pub enum RmcpClientRetryability {
    /// A bounded retry may succeed after a transient timeout or transport failure.
    Retryable,
    /// Repeating unchanged inputs cannot repair the failure.
    Permanent,
}

/// Stable semantic operation associated with one RMCP adapter failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "operations follow the adapter lifecycle rather than alphabetical order"
)]
pub enum RmcpClientOperation {
    /// Validate caller-owned request controls.
    ValidateRequest,
    /// Create the direct stdio child and its bounded pipes.
    StdioProcessSetup,
    /// Create the loopback-only HTTP transport.
    HttpClientSetup,
    /// Complete the MCP initialization handshake.
    Initialize,
    /// Admit only the supported server capability set.
    NegotiateCapabilities,
    /// Initialize and disclose only an authorized Tiber-owned root set.
    DeclareRoots,
    /// List the configured subset of server tools.
    ListTools,
    /// List bounded server resource metadata.
    ListResources,
    /// Read one authorized server resource.
    ReadResource,
    /// List bounded server prompt metadata.
    ListPrompts,
    /// Retrieve one authorized server prompt.
    GetPrompt,
    /// Invoke one authorized tool without replay.
    InvokeTool,
    /// Reconcile one ambiguous mutation through its status tool.
    ReconcileMutation,
    /// Deliver one explicitly authorized server observation.
    ObserveServer,
}

/// Sanitized actionable context for one stable RMCP adapter error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RmcpClientErrorContext {
    /// Fixed recovery guidance that never contains server or configured values.
    action: &'static str,
    /// Semantic adapter operation that failed.
    operation: RmcpClientOperation,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "context inspectors follow the owner recovery flow"
)]
impl RmcpClientErrorContext {
    /// Returns the semantic adapter operation that failed.
    #[must_use]
    #[inline]
    pub const fn operation(self) -> RmcpClientOperation {
        self.operation
    }

    /// Returns fixed sanitized recovery guidance.
    #[must_use]
    #[inline]
    pub const fn action(self) -> &'static str {
        self.action
    }
}

/// A direct sanitized cause retained when the adapter can do so safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "causes follow direct stdio then HTTP setup flow"
)]
pub enum RmcpClientCause {
    /// The host returned a process-spawn I/O class; paths and OS messages are not retained.
    ProcessSpawn(io::ErrorKind),
    /// A spawned child did not expose the piped standard input requested by Tiber.
    StandardInputUnavailable,
    /// A spawned child did not expose the piped standard output requested by Tiber.
    StandardOutputUnavailable,
    /// The fixed loopback HTTP client configuration could not be constructed.
    HttpClientConstruction,
    /// The server or transport rejected the MCP initialization handshake.
    ProtocolInitialization,
    /// The initialized transport closed before a matching response was observed.
    TransportClosed,
}

impl fmt::Display for RmcpClientCause {
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "causal display exposes only a stable sanitized classification"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessSpawn(_kind) => f.write_str("host process spawn failed"),
            Self::StandardInputUnavailable => f.write_str("child standard input unavailable"),
            Self::StandardOutputUnavailable => f.write_str("child standard output unavailable"),
            Self::HttpClientConstruction => f.write_str("loopback HTTP client construction failed"),
            Self::ProtocolInitialization => f.write_str("MCP initialization failed"),
            Self::TransportClosed => f.write_str("MCP transport closed"),
        }
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the sanitized cause is itself the terminal safe causal projection"
)]
impl Error for RmcpClientCause {}

/// Stable machine-readable classes at the bounded imperative RMCP boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "failure kinds follow adapter lifecycle flow rather than alphabetical order"
)]
pub enum RmcpClientErrorKind {
    /// The caller cancelled before a non-mutating operation produced a response.
    Cancelled,
    /// The bounded operation elapsed before a non-mutating operation produced a response.
    TimedOut,
    /// The configured process could not be spawned or its standard streams were unavailable.
    ProcessUnavailable,
    /// The configured local HTTP client could not be created.
    HttpClientUnavailable,
    /// Initialization did not complete over the bounded transport.
    InitializationFailed,
    /// The server did not advertise the mandatory tools capability.
    ToolsCapabilityRequired,
    /// The server advertised the unsupported MCP tasks extension.
    TasksUnsupported,
    /// The server requested or advertised an MCP capability outside this tools-only client.
    UnsupportedServerCapability,
    /// An authorization token selected a different configured integration.
    IntegrationMismatch,
    /// A server response was malformed, unsupported, or not a matching protocol result.
    InvalidServerResponse,
    /// A server frame, body, schema, metadata item, or result exceeded a fixed bound.
    UntrustedPayloadTooLarge,
    /// Server tool metadata exceeded the fixed page or catalog bound.
    ToolListLimitExceeded,
    /// Server resource or prompt metadata exceeded the fixed page or catalog bound.
    OptionalCatalogLimitExceeded,
    /// A caller-supplied timeout was zero and therefore could not define a request boundary.
    InvalidTimeout,
    /// A call token carried JSON that is valid generally but not an MCP argument object.
    InvalidToolArguments,
    /// The connection closed, a protocol handler rejected an unsupported capability, or I/O failed.
    TransportLost,
}

/// Structured, sanitized failure from the public RMCP adapter boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RmcpClientError {
    /// Direct cause retained only after projection into a safe bounded classification.
    cause: Option<RmcpClientCause>,
    /// Exact semantic operation and fixed owner-facing recovery guidance.
    context: RmcpClientErrorContext,
    /// Stable machine-readable failure class.
    kind: RmcpClientErrorKind,
    /// Whether retrying this exact failed operation can plausibly succeed.
    retryability: RmcpClientRetryability,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "constructors and inspectors follow error creation then recovery flow"
)]
impl RmcpClientError {
    /// Creates one error without retaining an external causal source.
    const fn new(kind: RmcpClientErrorKind, operation: RmcpClientOperation) -> Self {
        let retryability = match kind {
            RmcpClientErrorKind::TimedOut
            | RmcpClientErrorKind::InitializationFailed
            | RmcpClientErrorKind::TransportLost => RmcpClientRetryability::Retryable,
            RmcpClientErrorKind::Cancelled
            | RmcpClientErrorKind::ProcessUnavailable
            | RmcpClientErrorKind::HttpClientUnavailable
            | RmcpClientErrorKind::ToolsCapabilityRequired
            | RmcpClientErrorKind::TasksUnsupported
            | RmcpClientErrorKind::UnsupportedServerCapability
            | RmcpClientErrorKind::IntegrationMismatch
            | RmcpClientErrorKind::InvalidServerResponse
            | RmcpClientErrorKind::UntrustedPayloadTooLarge
            | RmcpClientErrorKind::ToolListLimitExceeded
            | RmcpClientErrorKind::OptionalCatalogLimitExceeded
            | RmcpClientErrorKind::InvalidTimeout
            | RmcpClientErrorKind::InvalidToolArguments => RmcpClientRetryability::Permanent,
        };
        Self {
            cause: None,
            context: RmcpClientErrorContext {
                action: recovery_action(kind),
                operation,
            },
            kind,
            retryability,
        }
    }

    /// Creates one error with a sanitized direct causal source.
    const fn caused(
        kind: RmcpClientErrorKind,
        operation: RmcpClientOperation,
        cause: RmcpClientCause,
        retryability: RmcpClientRetryability,
    ) -> Self {
        Self {
            cause: Some(cause),
            context: RmcpClientErrorContext {
                action: recovery_action(kind),
                operation,
            },
            kind,
            retryability,
        }
    }

    /// Returns the stable machine-readable failure class.
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> RmcpClientErrorKind {
        self.kind
    }

    /// Returns the retained sanitized direct cause when one was safe to preserve.
    #[must_use]
    #[inline]
    pub const fn retained_cause(&self) -> Option<RmcpClientCause> {
        self.cause
    }

    /// Returns the stable code for this adapter failure.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        match self.kind {
            RmcpClientErrorKind::Cancelled => "rmcp_client_cancelled",
            RmcpClientErrorKind::TimedOut => "rmcp_client_timed_out",
            RmcpClientErrorKind::ProcessUnavailable => "rmcp_client_process_unavailable",
            RmcpClientErrorKind::HttpClientUnavailable => "rmcp_client_http_client_unavailable",
            RmcpClientErrorKind::InitializationFailed => "rmcp_client_initialization_failed",
            RmcpClientErrorKind::ToolsCapabilityRequired => "rmcp_client_tools_capability_required",
            RmcpClientErrorKind::TasksUnsupported => "rmcp_client_tasks_unsupported",
            RmcpClientErrorKind::UnsupportedServerCapability => {
                "rmcp_client_unsupported_server_capability"
            }
            RmcpClientErrorKind::IntegrationMismatch => "rmcp_client_integration_mismatch",
            RmcpClientErrorKind::InvalidServerResponse => "rmcp_client_invalid_server_response",
            RmcpClientErrorKind::UntrustedPayloadTooLarge => {
                "rmcp_client_untrusted_payload_too_large"
            }
            RmcpClientErrorKind::ToolListLimitExceeded => "rmcp_client_tool_list_limit_exceeded",
            RmcpClientErrorKind::OptionalCatalogLimitExceeded => {
                "rmcp_client_optional_catalog_limit_exceeded"
            }
            RmcpClientErrorKind::InvalidTimeout => "rmcp_client_invalid_timeout",
            RmcpClientErrorKind::InvalidToolArguments => "rmcp_client_invalid_tool_arguments",
            RmcpClientErrorKind::TransportLost => "rmcp_client_transport_lost",
        }
    }

    /// Returns sanitized actionable context for the failed adapter operation.
    #[must_use]
    #[inline]
    pub const fn context(&self) -> RmcpClientErrorContext {
        self.context
    }

    /// Returns whether a bounded retry can plausibly change this failure.
    #[must_use]
    #[inline]
    pub const fn retryability(&self) -> RmcpClientRetryability {
        self.retryability
    }
}

impl fmt::Display for RmcpClientError {
    #[expect(
        clippy::implicit_return,
        reason = "the standard display implementation directly delegates to the stable code table"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the only meaningful default override is the retained sanitized source"
)]
impl Error for RmcpClientError {
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "only sanitized setup causes are retained across the public adapter boundary"
    )]
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self {
                cause: Some(cause), ..
            } => Some(cause),
            Self { cause: None, .. } => None,
        }
    }
}

/// A sanitized adapter failure bound to the exact consumed call authorization.
///
/// The provenance transcript contains no arguments, integration configuration,
/// or replay authority. This wrapper deliberately omits derived `Debug` so
/// diagnostics cannot expose future provenance fields by accident.
pub struct RmcpToolCallError {
    /// Stable sanitized adapter error.
    error: RmcpClientError,
    /// Safe originating call identity with all invocation authority removed.
    provenance: BoundToolCallFailure,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the private constructor precedes the public safe inspectors in lifecycle order"
)]
impl RmcpToolCallError {
    /// Binds an adapter failure to consumed, non-replayable call provenance.
    const fn new(error: RmcpClientError, provenance: BoundToolCallFailure) -> Self {
        Self { error, provenance }
    }

    /// Returns the stable sanitized adapter error.
    #[must_use]
    #[inline]
    pub const fn error(&self) -> &RmcpClientError {
        &self.error
    }

    /// Returns safe originating provenance with no invocation authority.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &BoundToolCallFailure {
        &self.provenance
    }
}

impl fmt::Debug for RmcpToolCallError {
    #[expect(
        clippy::implicit_return,
        reason = "debug deliberately exposes only the stable sanitized adapter error"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RmcpToolCallError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for RmcpToolCallError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates only to the stable sanitized adapter error"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the sanitized adapter error is the only meaningful causal override"
)]
impl Error for RmcpToolCallError {
    #[expect(
        clippy::implicit_return,
        reason = "the sanitized adapter error is the only exposed causal source"
    )]
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

/// A sanitized adapter failure bound to the exact recovery authorization.
///
/// The provenance transcript contains no arguments, integration configuration,
/// payload, or invocation replay authority. This wrapper deliberately omits
/// derived `Debug` so diagnostics cannot expose future provenance fields by
/// accident.
pub struct RmcpReconciliationError {
    /// Stable sanitized adapter error.
    error: RmcpClientError,
    /// Safe originating recovery identity with all invocation authority removed.
    provenance: BoundReconciliationFailure,
}

#[expect(
    clippy::implicit_return,
    reason = "the public safe inspectors return direct references to the redacted transcript"
)]
impl RmcpReconciliationError {
    /// Returns the stable sanitized adapter error.
    #[must_use]
    #[inline]
    pub const fn error(&self) -> &RmcpClientError {
        &self.error
    }

    /// Returns exact safe recovery provenance with no invocation authority.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &BoundReconciliationFailure {
        &self.provenance
    }
}

impl fmt::Debug for RmcpReconciliationError {
    #[expect(
        clippy::implicit_return,
        reason = "debug deliberately exposes only the stable sanitized adapter error"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RmcpReconciliationError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for RmcpReconciliationError {
    #[expect(
        clippy::implicit_return,
        reason = "display delegates only to the stable sanitized adapter error"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the sanitized adapter error is the only meaningful causal override"
)]
impl Error for RmcpReconciliationError {
    #[expect(
        clippy::implicit_return,
        reason = "the sanitized adapter error is the only exposed causal source"
    )]
    #[inline]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

/// Failures of the raw, Tiber-owned bounded stdio frame reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StdioReadError {
    /// The peer exceeded the frame limit before a newline delimiter.
    LineTooLong,
    /// The underlying stdio reader failed before a complete frame.
    ReadFailed,
}

/// Raw newline-delimited JSON-RPC reader with a fixed untrusted frame cap.
struct BoundedStdioReader<R> {
    /// Bytes retained until the next complete newline-delimited frame.
    buffered: Vec<u8>,
    /// Whether an oversized frame is being discarded through its delimiter.
    discard_until_newline: bool,
    /// Maximum accepted frame length, clamped to the client hard limit.
    maximum_bytes: usize,
    /// Direct child stdout or deterministic fixture reader.
    reader: R,
}

impl<R> BoundedStdioReader<R> {
    /// Creates a deterministic test reader whose frame bound cannot exceed Tiber's hard cap.
    #[cfg(test)]
    #[expect(
        clippy::implicit_return,
        reason = "the raw-reader fixture constructor is clearest as a cap-clamped tail expression"
    )]
    fn new(reader: R, maximum_bytes: usize) -> Self {
        Self {
            buffered: Vec::new(),
            discard_until_newline: false,
            maximum_bytes: maximum_bytes.min(MAX_STDIO_MESSAGE_BYTES),
            reader,
        }
    }

    /// Reads one bounded raw frame, discarding an oversized line through its delimiter.
    #[expect(
        clippy::implicit_return,
        clippy::indexing_slicing,
        clippy::question_mark_used,
        reason = "the fixed remaining-byte invariant makes the chunk slice safe and the parser readable"
    )]
    async fn next_message(&mut self) -> Result<Option<Vec<u8>>, StdioReadError>
    where
        R: AsyncRead + Unpin,
    {
        loop {
            if self.discard_until_newline {
                if let Some(newline) = self.buffered.iter().position(|byte| *byte == b'\n') {
                    self.buffered.drain(..=newline);
                    self.discard_until_newline = false;
                    continue;
                }
                self.buffered.clear();
            }
            if let Some(newline) = self.buffered.iter().position(|byte| *byte == b'\n') {
                let mut message = self.buffered.drain(..=newline).collect::<Vec<_>>();
                let _discarded_newline = message.pop();
                if message.last() == Some(&b'\r') {
                    let _discarded_carriage_return = message.pop();
                }
                return Ok(Some(message));
            }
            if self.buffered.len() > self.maximum_bytes {
                self.discard_until_newline = true;
                return Err(StdioReadError::LineTooLong);
            }

            let remaining = if self.discard_until_newline {
                STDIO_READ_CHUNK_BYTES
            } else {
                self.maximum_bytes
                    .saturating_add(1)
                    .saturating_sub(self.buffered.len())
                    .min(STDIO_READ_CHUNK_BYTES)
            };
            let mut chunk = [0; STDIO_READ_CHUNK_BYTES];
            let read = self
                .reader
                .read(&mut chunk[..remaining])
                .await
                .map_err(|_source| StdioReadError::ReadFailed)?;
            if read == 0 {
                if self.discard_until_newline || self.buffered.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(mem::take(&mut self.buffered)));
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    /// Runs the raw bounded-reader loop and records its first terminal transport fault.
    #[expect(
        clippy::ignored_unit_patterns,
        clippy::integer_division_remainder_used,
        reason = "the Tokio select macro implements cancellation precedence for the bounded reader"
    )]
    async fn run(
        mut self,
        mut sender: mpsc::Sender<RxJsonRpcMessage<RoleClient>>,
        health: ConnectionHealth,
        cancellation: CancellationToken,
    ) where
        R: AsyncRead + Send + Unpin + 'static,
    {
        loop {
            let inbound_frame = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                frame = self.next_message() => frame,
            };
            let raw_message = match inbound_frame {
                Ok(Some(frame)) => frame,
                Ok(None) => return,
                Err(StdioReadError::LineTooLong) => {
                    health.record(ConnectionFault::OversizedInboundMessage);
                    cancellation.cancel();
                    return;
                }
                Err(StdioReadError::ReadFailed) => {
                    health.record(ConnectionFault::ReadFailure);
                    cancellation.cancel();
                    return;
                }
            };
            let parsed_message = match serde_json::from_slice(&raw_message) {
                Ok(parsed) => parsed,
                Err(_source) => {
                    health.record(ConnectionFault::MalformedInboundMessage);
                    cancellation.cancel();
                    return;
                }
            };
            if sender.send(parsed_message).await.is_err() {
                return;
            }
        }
    }
}

/// Direct stdio writer that applies the Tiber-owned outbound JSON-RPC frame limit.
struct BoundedStdioWriter<W>(W);

#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the writer-owned sink keeps framing and its I/O conversion at one narrow boundary"
)]
impl<W> BoundedStdioWriter<W>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    /// Converts this direct child writer into its bounded newline-delimited sink.
    fn into_sink(
        self,
    ) -> impl Sink<TxJsonRpcMessage<RoleClient>, Error = io::Error> + Send + Unpin + 'static {
        Box::pin(sink::unfold(
            self.0,
            |mut sink_writer, outbound_message| async move {
                let serialized_message = serde_json::to_vec(&outbound_message)
                    .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
                if serialized_message.len() > MAX_STDIO_MESSAGE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "outbound RMCP message exceeds bounded stdio frame",
                    ));
                }
                sink_writer.write_all(&serialized_message).await?;
                sink_writer.write_all(b"\n").await?;
                sink_writer.flush().await?;
                Ok(sink_writer)
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// First terminal fault recorded for one transport connection.
enum ConnectionFault {
    /// No terminal fault has been observed.
    Healthy = 0,
    /// A peer delivered JSON-RPC bytes that RMCP could not parse.
    MalformedInboundMessage = 1,
    /// A peer exceeded a Tiber-owned inbound size limit.
    OversizedInboundMessage = 2,
    /// Raw transport input failed before a protocol response.
    ReadFailure = 3,
    /// A peer exercised a capability this client does not admit.
    UnsupportedCapability = 4,
}

impl ConnectionFault {
    /// Returns the stable compact value retained in the shared health cell.
    #[expect(
        clippy::implicit_return,
        reason = "the total compact-value mapping is clearest as a tail match"
    )]
    const fn as_u8(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::MalformedInboundMessage => 1,
            Self::OversizedInboundMessage => 2,
            Self::ReadFailure => 3,
            Self::UnsupportedCapability => 4,
        }
    }
}

#[derive(Clone, Debug)]
/// Shared first-fault state between the bounded transport and the adapter.
struct ConnectionHealth(Arc<AtomicU8>);

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "health transitions are ordered as create, record, then project into a public error"
)]
impl ConnectionHealth {
    /// Creates a healthy connection state.
    fn new() -> Self {
        Self(Arc::new(AtomicU8::new(ConnectionFault::Healthy.as_u8())))
    }

    /// Retains the first terminal fault without overwriting its causal boundary.
    fn record(&self, fault: ConnectionFault) {
        let _first_fault = self.0.compare_exchange(
            ConnectionFault::Healthy.as_u8(),
            fault.as_u8(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Maps the recorded terminal fault into a stable adapter error.
    fn client_error(&self, operation: RmcpClientOperation) -> Option<RmcpClientError> {
        match self.0.load(Ordering::Acquire) {
            value if value == ConnectionFault::Healthy.as_u8() => None,
            value if value == ConnectionFault::UnsupportedCapability.as_u8() => Some(
                RmcpClientError::new(RmcpClientErrorKind::UnsupportedServerCapability, operation),
            ),
            _ => Some(RmcpClientError::caused(
                RmcpClientErrorKind::TransportLost,
                operation,
                RmcpClientCause::TransportClosed,
                RmcpClientRetryability::Retryable,
            )),
        }
    }
}

/// Client-side RMCP callbacks restricted to the admitted capability subset.
struct RestrictedClientHandler {
    /// Connection-wide cancellation for terminal capability refusals.
    cancellation: CancellationToken,
    /// Shared terminal-fault projection read by public operations.
    health: ConnectionHealth,
    /// Bounded untrusted observations awaiting explicit policy delivery.
    observations: ObservationSenders,
    /// Opaque authorization that alone permits roots capability advertisement and disclosure.
    root_declaration: Option<AuthorizedRootDeclaration>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "refusal precedes bounded projection because terminal capability handling is the handler's first concern"
)]
impl RestrictedClientHandler {
    /// Records an unauthorized peer action and terminally cancels the connection.
    fn refuse(&self) {
        self.health.record(ConnectionFault::UnsupportedCapability);
        self.cancellation.cancel();
    }

    /// Projects a progress notification into bounded untrusted data.
    fn observe_progress(&self, params: &ProgressNotificationParam) {
        match bounded_serialized_payload(
            serde_json::to_string(params),
            RmcpClientOperation::ObserveServer,
        ) {
            Ok(payload) => {
                drop(
                    self.observations
                        .try_send(ServerObservation::Progress(payload)),
                );
            }
            Err(_) => self.refuse(),
        }
    }

    /// Projects a resource-update notification into bounded untrusted data.
    fn observe_resource_updated(&self, params: &ResourceUpdatedNotificationParam) {
        match bounded_serialized_payload(
            serde_json::to_string(params),
            RmcpClientOperation::ObserveServer,
        ) {
            Ok(payload) => {
                drop(
                    self.observations
                        .try_send(ServerObservation::ResourceUpdated(payload)),
                );
            }
            Err(_) => self.refuse(),
        }
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "RMCP callbacks follow protocol authority order; only admitted callbacks override defaults"
)]
impl ClientHandler for RestrictedClientHandler {
    // RMCP 3.1.2 has no replacement for this callback; it is retained solely
    // to refuse sampling and terminally mark the connection unhealthy.
    #[expect(
        clippy::absolute_paths,
        clippy::allow_attributes,
        reason = "the required item-level RMCP 3.1.2 sampling deprecation exception is deliberately visible"
    )]
    #[allow(
        deprecated,
        reason = "RMCP 3.1.2 sampling callback has no non-deprecated replacement"
    )]
    fn create_message(
        &self,
        _params: rmcp::model::CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl future::Future<Output = Result<rmcp::model::CreateMessageResult, ErrorData>> + Send + '_
    {
        self.refuse();
        future::ready(Err(ErrorData::method_not_found::<
            rmcp::model::CreateMessageRequestMethod,
        >()))
    }

    fn create_elicitation(
        &self,
        _params: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl future::Future<Output = Result<ElicitResult, ErrorData>> + Send + '_ {
        self.refuse();
        future::ready(Err(ErrorData::method_not_found::<
            ElicitationCreateRequestMethod,
        >()))
    }

    // RMCP 3.1.2 has no replacement for this callback. It remains the narrow
    // hook through which the ADR-0008 Tiber-owned roots response will flow.
    #[expect(
        clippy::absolute_paths,
        clippy::allow_attributes,
        reason = "the required item-level RMCP 3.1.2 roots deprecation exception is deliberately visible"
    )]
    #[allow(
        deprecated,
        reason = "RMCP 3.1.2 roots callback has no non-deprecated replacement"
    )]
    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl future::Future<Output = Result<rmcp::model::ListRootsResult, ErrorData>> + Send + '_
    {
        let Some(authorization) = self.root_declaration.as_ref() else {
            self.refuse();
            return future::ready(Err(ErrorData::method_not_found::<
                rmcp::model::ListRootsRequestMethod,
            >()));
        };
        let roots = authorization
            .roots()
            .iter()
            .map(|root| rmcp::model::Root::new(root.as_uri().to_owned()))
            .collect();
        future::ready(Ok(rmcp::model::ListRootsResult::new(roots)))
    }

    fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleClient>,
    ) -> impl future::Future<Output = Result<CustomResult, ErrorData>> + Send + '_ {
        self.refuse();
        future::ready(Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            request.method,
            None,
        )))
    }

    fn on_task_status(
        &self,
        _params: TaskStatusNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        self.refuse();
        future::ready(())
    }

    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        self.observe_progress(&params);
        future::ready(())
    }

    // RMCP 3.1.2 has no replacement for this callback; it projects logging
    // only into the bounded observation buffer.
    #[expect(
        clippy::absolute_paths,
        clippy::allow_attributes,
        reason = "the required item-level RMCP 3.1.2 logging deprecation exception is deliberately visible"
    )]
    #[allow(
        deprecated,
        reason = "RMCP 3.1.2 logging callback has no non-deprecated replacement"
    )]
    fn on_logging_message(
        &self,
        params: rmcp::model::LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        match bounded_serialized_payload(
            serde_json::to_string(&params),
            RmcpClientOperation::ObserveServer,
        ) {
            Ok(payload) => {
                drop(
                    self.observations
                        .try_send(ServerObservation::Logging(payload)),
                );
            }
            Err(_) => self.refuse(),
        }
        future::ready(())
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        drop(
            self.observations
                .try_send(ServerObservation::ToolListChanged),
        );
        future::ready(())
    }

    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        self.observe_resource_updated(&params);
        future::ready(())
    }

    fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        drop(
            self.observations
                .try_send(ServerObservation::ResourceListChanged),
        );
        future::ready(())
    }

    fn on_prompt_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        drop(
            self.observations
                .try_send(ServerObservation::PromptListChanged),
        );
        future::ready(())
    }

    fn on_subscriptions_acknowledged(
        &self,
        _params: SubscriptionsAcknowledgedNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        self.refuse();
        future::ready(())
    }

    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) -> impl future::Future<Output = ()> + Send + '_ {
        if notification.method == "notifications/progress"
            && let Ok(Some(params)) = notification.params_as::<ProgressNotificationParam>()
        {
            self.observe_progress(&params);
            return future::ready(());
        }
        self.refuse();
        future::ready(())
    }

    fn get_info(&self) -> ClientInfo {
        let mut capabilities = ClientCapabilities::default();
        if self.root_declaration.is_some() {
            capabilities.roots = Some(RootsCapabilities::default());
        }
        ClientInfo::new(
            capabilities,
            Implementation::new("tiber-rmcp-client", env!("CARGO_PKG_VERSION")),
        )
    }
}

/// A live harness-owned connection to one configured integration.
pub struct RmcpClient {
    /// Connection-wide bridge that immediately aborts active transport work.
    cancellation: CancellationToken,
    /// Direct child to kill and reap for stdio connections.
    child: Option<Child>,
    /// First terminal transport or capability fault.
    health: ConnectionHealth,
    /// Exact configured integration identity bound at connection time.
    integration: McpIntegration,
    /// Independently bounded untrusted server observations awaiting policy-gated delivery.
    observations: ObservationReceivers,
    /// RMCP peer service that owns initialization and request routing.
    service: RunningService<RoleClient, RestrictedClientHandler>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "connection methods follow authorization and terminal-lifecycle order; tail expressions preserve the state machine"
)]
impl RmcpClient {
    /// Connects only for an opaque authorized listing operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_listing(
        authorization: &AuthorizedToolListing,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ListTools,
        )
        .await
    }

    /// Connects only for an opaque authorized Tiber-owned root declaration.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_root_declaration(
        authorization: &AuthorizedRootDeclaration,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            Some(authorization.clone()),
            options,
            RmcpClientOperation::DeclareRoots,
        )
        .await
    }

    /// Connects only for an opaque authorized resource-listing operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_resource_listing(
        authorization: &AuthorizedResourceListing,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ListResources,
        )
        .await
    }

    /// Connects only for an opaque authorized resource-read operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_resource_read(
        authorization: &AuthorizedResourceRead,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ReadResource,
        )
        .await
    }

    /// Connects only for an opaque authorized prompt-listing operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_prompt_listing(
        authorization: &AuthorizedPromptListing,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ListPrompts,
        )
        .await
    }

    /// Connects only for an opaque authorized prompt retrieval operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_prompt_get(
        authorization: &AuthorizedPromptGet,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::GetPrompt,
        )
        .await
    }

    /// Connects only for an opaque authorized tool call.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_call(
        authorization: &AuthorizedToolCall,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::InvokeTool,
        )
        .await
    }

    /// Connects only for an opaque authorized reconciliation operation.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_reconciliation(
        authorization: &AuthorizedReconciliation,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ReconcileMutation,
        )
        .await
    }

    /// Connects only for an opaque, capability-specific server-observation authorization.
    ///
    /// # Errors
    ///
    /// Returns a stable error when initialization, the bounded transport, or the
    /// caller's deadline/cancellation prevents connecting.
    #[inline]
    pub async fn connect_for_observation(
        authorization: &AuthorizedServerObservation,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError> {
        Self::connect(
            authorization.integration().clone(),
            None,
            options,
            RmcpClientOperation::ObserveServer,
        )
        .await
    }

    /// Lists only configured tools under a separate opaque listing authorization.
    ///
    /// # Errors
    ///
    /// Returns a stable error when authorization identity mismatches, server
    /// metadata is invalid or over-bounded, or the operation cannot complete.
    #[expect(
        clippy::too_many_lines,
        reason = "pagination, configured-tool filtering, and every untrusted metadata bound stay together at the authority boundary"
    )]
    #[inline]
    pub async fn list_tools(
        &mut self,
        authorization: &AuthorizedToolListing,
        options: RequestOptions,
    ) -> Result<Vec<ListedTool>, RmcpClientError> {
        let operation = RmcpClientOperation::ListTools;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(error);
        }
        if authorization.integration() != &self.integration {
            return Err(failure(RmcpClientErrorKind::IntegrationMismatch));
        }

        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut seen_tools = BTreeSet::new();
        let mut listed_tools = Vec::new();

        for _page in 0..MAX_TOOL_LIST_PAGES {
            let request_options = match remaining_options(&options, deadline, operation) {
                Ok(remaining) => remaining,
                Err(error) => {
                    self.close_if_connection_failure(error).await;
                    return Err(error);
                }
            };
            let request = ClientRequest::ListToolsRequest(ListToolsRequest {
                method: ListToolsRequestMethod,
                params: cursor
                    .clone()
                    .map(|value| PaginatedRequestParams::default().with_cursor(Some(value))),
                extensions: Extensions::default(),
            });
            let server_result = match self.request(request, &request_options, operation).await {
                Ok(response) => response,
                Err(error) => {
                    self.close_inner().await;
                    return Err(error.into_client_error(operation));
                }
            };
            let ServerResult::ListToolsResult(list_response) = server_result else {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            };
            if list_response
                .result_type
                .as_ref()
                .is_some_and(|result_type| !result_type.is_complete())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.ttl_ms.is_some() || list_response.cache_scope.is_some() {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.tools.len() > MAX_CONFIGURED_TOOLS {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::ToolListLimitExceeded));
            }
            for tool in list_response.tools {
                let name = match ToolName::parse(tool.name.as_ref()) {
                    Ok(name) => name,
                    Err(_source) => {
                        self.close_inner().await;
                        return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
                    }
                };
                if self.integration.tool_class(&name).is_none() || !seen_tools.insert(name.clone())
                {
                    continue;
                }
                if listed_tools.len() == MAX_CONFIGURED_TOOLS {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::ToolListLimitExceeded));
                }
                let description = match tool.description {
                    Some(server_description) => {
                        match UntrustedPayload::bounded(server_description.as_ref()) {
                            Ok(bounded_description) => Some(bounded_description),
                            Err(_source) => {
                                self.close_inner().await;
                                return Err(failure(RmcpClientErrorKind::UntrustedPayloadTooLarge));
                            }
                        }
                    }
                    None => None,
                };
                let input_schema = match bounded_serialized_payload(
                    serde_json::to_string(tool.input_schema.as_ref()),
                    operation,
                ) {
                    Ok(input_schema) => input_schema,
                    Err(error) => {
                        self.close_inner().await;
                        return Err(error);
                    }
                };
                let output_schema = match tool.output_schema {
                    Some(server_schema) => {
                        match bounded_serialized_payload(
                            serde_json::to_string(server_schema.as_ref()),
                            operation,
                        ) {
                            Ok(bounded_schema) => Some(bounded_schema),
                            Err(error) => {
                                self.close_inner().await;
                                return Err(error);
                            }
                        }
                    }
                    None => None,
                };
                listed_tools.push(ListedTool {
                    description,
                    input_schema,
                    name,
                    output_schema,
                });
            }

            let Some(next_cursor) = list_response.next_cursor else {
                return Ok(listed_tools);
            };
            if next_cursor.len() > MAX_SEMANTIC_TEXT_BYTES
                || !seen_cursors.insert(next_cursor.clone())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            }
            cursor = Some(next_cursor);
        }

        self.close_inner().await;
        Err(failure(RmcpClientErrorKind::ToolListLimitExceeded))
    }

    /// Lists bounded, untrusted resource metadata under an opaque authorization.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the authorization targets another integration,
    /// resources were not advertised, a bounded response is invalid, or the
    /// read-only operation cannot complete. The adapter never retries it.
    #[inline]
    pub async fn list_resources(
        &mut self,
        authorization: &AuthorizedResourceListing,
        options: RequestOptions,
    ) -> Result<Vec<UntrustedPayload>, RmcpClientError> {
        let operation = RmcpClientOperation::ListResources;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(error);
        }
        if authorization.integration() != &self.integration {
            return Err(failure(RmcpClientErrorKind::IntegrationMismatch));
        }
        if !self.supports_resources() {
            return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
        }

        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut resources = Vec::new();
        for _page in 0..MAX_OPTIONAL_CATALOG_PAGES {
            let request_options = match remaining_options(&options, deadline, operation) {
                Ok(remaining) => remaining,
                Err(error) => {
                    self.close_if_connection_failure(error).await;
                    return Err(error);
                }
            };
            let request = ClientRequest::ListResourcesRequest(ListResourcesRequest {
                method: ListResourcesRequestMethod,
                params: cursor
                    .clone()
                    .map(|value| PaginatedRequestParams::default().with_cursor(Some(value))),
                extensions: Extensions::default(),
            });
            let server_result = match self.request(request, &request_options, operation).await {
                Ok(response) => response,
                Err(error) => {
                    self.close_inner().await;
                    return Err(error.into_client_error(operation));
                }
            };
            let ServerResult::ListResourcesResult(list_response) = server_result else {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            };
            if list_response
                .result_type
                .as_ref()
                .is_some_and(|result_type| !result_type.is_complete())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.ttl_ms.is_some() || list_response.cache_scope.is_some() {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.resources.len() > MAX_OPTIONAL_CATALOG_ITEMS {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded));
            }
            for server_resource in list_response.resources {
                if resources.len() == MAX_OPTIONAL_CATALOG_ITEMS {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded));
                }
                let bounded_resource = match bounded_serialized_payload(
                    serde_json::to_string(&server_resource),
                    operation,
                ) {
                    Ok(payload) => payload,
                    Err(error) => {
                        self.close_inner().await;
                        return Err(error);
                    }
                };
                resources.push(bounded_resource);
            }
            let Some(next_cursor) = list_response.next_cursor else {
                return Ok(resources);
            };
            if next_cursor.len() > MAX_SEMANTIC_TEXT_BYTES
                || !seen_cursors.insert(next_cursor.clone())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            }
            cursor = Some(next_cursor);
        }
        self.close_inner().await;
        Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded))
    }

    /// Reads one exact authorized resource into a bounded untrusted payload.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the authorization targets another integration,
    /// resources were not advertised, the response requests continuation or
    /// caching, or the read-only operation cannot complete. The adapter never retries it.
    #[inline]
    pub async fn read_resource(
        &mut self,
        authorization: &AuthorizedResourceRead,
        options: RequestOptions,
    ) -> Result<UntrustedPayload, RmcpClientError> {
        let operation = RmcpClientOperation::ReadResource;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(error);
        }
        if authorization.integration() != &self.integration {
            return Err(failure(RmcpClientErrorKind::IntegrationMismatch));
        }
        if !self.supports_resources() {
            return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
        }
        let request = ClientRequest::ReadResourceRequest(ReadResourceRequest::new(
            ReadResourceRequestParams::new(authorization.uri().as_str().to_owned()),
        ));
        let request_options = match remaining_options(&options, deadline, operation) {
            Ok(remaining) => remaining,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        match self.request(request, &request_options, operation).await {
            Ok(ServerResult::ReadResourceResult(result)) => {
                if result
                    .result_type
                    .as_ref()
                    .is_some_and(|result_type| !result_type.is_complete())
                {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
                }
                if result.ttl_ms.is_some() || result.cache_scope.is_some() {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
                }
                match bounded_serialized_payload(serde_json::to_string(&result), operation) {
                    Ok(payload) => Ok(payload),
                    Err(error) => {
                        self.close_inner().await;
                        Err(error)
                    }
                }
            }
            Ok(ServerResult::InputRequiredResult(_input_required)) => {
                self.close_inner().await;
                Err(failure(RmcpClientErrorKind::UnsupportedServerCapability))
            }
            Ok(_) => {
                self.close_inner().await;
                Err(failure(RmcpClientErrorKind::InvalidServerResponse))
            }
            Err(error) => {
                self.close_inner().await;
                Err(error.into_client_error(operation))
            }
        }
    }

    /// Lists bounded, untrusted prompt metadata under an opaque authorization.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the authorization targets another integration,
    /// prompts were not advertised, a bounded response is invalid, or the
    /// read-only operation cannot complete. The adapter never retries it.
    #[inline]
    pub async fn list_prompts(
        &mut self,
        authorization: &AuthorizedPromptListing,
        options: RequestOptions,
    ) -> Result<Vec<UntrustedPayload>, RmcpClientError> {
        let operation = RmcpClientOperation::ListPrompts;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(error);
        }
        if authorization.integration() != &self.integration {
            return Err(failure(RmcpClientErrorKind::IntegrationMismatch));
        }
        if !self.supports_prompts() {
            return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
        }

        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut prompts = Vec::new();
        for _page in 0..MAX_OPTIONAL_CATALOG_PAGES {
            let request_options = match remaining_options(&options, deadline, operation) {
                Ok(remaining) => remaining,
                Err(error) => {
                    self.close_if_connection_failure(error).await;
                    return Err(error);
                }
            };
            let request = ClientRequest::ListPromptsRequest(ListPromptsRequest {
                method: ListPromptsRequestMethod,
                params: cursor
                    .clone()
                    .map(|value| PaginatedRequestParams::default().with_cursor(Some(value))),
                extensions: Extensions::default(),
            });
            let server_result = match self.request(request, &request_options, operation).await {
                Ok(response) => response,
                Err(error) => {
                    self.close_inner().await;
                    return Err(error.into_client_error(operation));
                }
            };
            let ServerResult::ListPromptsResult(list_response) = server_result else {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            };
            if list_response
                .result_type
                .as_ref()
                .is_some_and(|result_type| !result_type.is_complete())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.ttl_ms.is_some() || list_response.cache_scope.is_some() {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
            }
            if list_response.prompts.len() > MAX_OPTIONAL_CATALOG_ITEMS {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded));
            }
            for server_prompt in list_response.prompts {
                if prompts.len() == MAX_OPTIONAL_CATALOG_ITEMS {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded));
                }
                let bounded_prompt = match bounded_serialized_payload(
                    serde_json::to_string(&server_prompt),
                    operation,
                ) {
                    Ok(payload) => payload,
                    Err(error) => {
                        self.close_inner().await;
                        return Err(error);
                    }
                };
                prompts.push(bounded_prompt);
            }
            let Some(next_cursor) = list_response.next_cursor else {
                return Ok(prompts);
            };
            if next_cursor.len() > MAX_SEMANTIC_TEXT_BYTES
                || !seen_cursors.insert(next_cursor.clone())
            {
                self.close_inner().await;
                return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
            }
            cursor = Some(next_cursor);
        }
        self.close_inner().await;
        Err(failure(RmcpClientErrorKind::OptionalCatalogLimitExceeded))
    }

    /// Retrieves one exact authorized prompt into a bounded untrusted payload.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the authorization targets another integration,
    /// prompts were not advertised, the response requests continuation, or the
    /// read-only operation cannot complete. The adapter never retries it.
    #[inline]
    pub async fn get_prompt(
        &mut self,
        authorization: &AuthorizedPromptGet,
        options: RequestOptions,
    ) -> Result<UntrustedPayload, RmcpClientError> {
        let operation = RmcpClientOperation::GetPrompt;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(error);
        }
        if authorization.integration() != &self.integration {
            return Err(failure(RmcpClientErrorKind::IntegrationMismatch));
        }
        if !self.supports_prompts() {
            return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
        }
        let parsed_arguments = match authorization.arguments() {
            Some(core_arguments) => {
                match serde_json::from_str::<JsonObject>(core_arguments.as_json()) {
                    Ok(json_arguments) => Some(json_arguments),
                    Err(_source) => {
                        return Err(failure(RmcpClientErrorKind::InvalidServerResponse));
                    }
                }
            }
            None => None,
        };
        let mut params = GetPromptRequestParams::new(authorization.name().as_str().to_owned());
        if let Some(json_arguments) = parsed_arguments {
            params = params.with_arguments(json_arguments);
        }
        let request = ClientRequest::GetPromptRequest(GetPromptRequest::new(params));
        let request_options = match remaining_options(&options, deadline, operation) {
            Ok(remaining) => remaining,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(error);
            }
        };
        match self.request(request, &request_options, operation).await {
            Ok(ServerResult::GetPromptResult(result)) => {
                if result
                    .result_type
                    .as_ref()
                    .is_some_and(|result_type| !result_type.is_complete())
                {
                    self.close_inner().await;
                    return Err(failure(RmcpClientErrorKind::UnsupportedServerCapability));
                }
                match bounded_serialized_payload(serde_json::to_string(&result), operation) {
                    Ok(payload) => Ok(payload),
                    Err(error) => {
                        self.close_inner().await;
                        Err(error)
                    }
                }
            }
            Ok(ServerResult::InputRequiredResult(_input_required)) => {
                self.close_inner().await;
                Err(failure(RmcpClientErrorKind::UnsupportedServerCapability))
            }
            Ok(_) => {
                self.close_inner().await;
                Err(failure(RmcpClientErrorKind::InvalidServerResponse))
            }
            Err(error) => {
                self.close_inner().await;
                Err(error.into_client_error(operation))
            }
        }
    }

    /// Invokes exactly one opaque authorized tool call without any replay route.
    ///
    /// The authorization is consumed and every result carries only its safe
    /// originating provenance, so neither success nor failure can replay it.
    ///
    /// # Errors
    ///
    /// Returns a stable provenance-bound error for an invalid operation. A
    /// dispatched mutating operation instead returns its required ambiguous
    /// outcome bound to the exact consumed authorization.
    #[inline]
    pub async fn call(
        &mut self,
        authorization: AuthorizedToolCall,
        options: RequestOptions,
    ) -> Result<BoundToolCallOutcome, RmcpToolCallError> {
        let operation = RmcpClientOperation::InvokeTool;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(RmcpToolCallError::new(error, authorization.bind_failure()));
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(RmcpToolCallError::new(error, authorization.bind_failure()));
        }
        if authorization.integration() != &self.integration {
            return Err(RmcpToolCallError::new(
                failure(RmcpClientErrorKind::IntegrationMismatch),
                authorization.bind_failure(),
            ));
        }
        let class = authorization.class();
        let request = match call_request(authorization.tool(), authorization.arguments(), operation)
        {
            Ok(request) => request,
            Err(error) => {
                return Err(RmcpToolCallError::new(error, authorization.bind_failure()));
            }
        };
        let request_options = match remaining_options(&options, deadline, operation) {
            Ok(remaining) => remaining,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(RmcpToolCallError::new(error, authorization.bind_failure()));
            }
        };
        let request_result = self.request(request, &request_options, operation).await;
        match request_result {
            Ok(ServerResult::CallToolResult(call_result)) => {
                match bounded_call_result(&call_result, operation) {
                    Ok(payload) => Ok(authorization.bind_observation(payload)),
                    Err(error) => self.ambiguous_or_error(authorization, class, error).await,
                }
            }
            Ok(_) => {
                self.ambiguous_or_error(
                    authorization,
                    class,
                    failure(RmcpClientErrorKind::InvalidServerResponse),
                )
                .await
            }
            Err(request_failure) => {
                self.ambiguous_or_error(
                    authorization,
                    class,
                    request_failure.into_client_error(operation),
                )
                .await
            }
        }
    }

    /// Reconciles one opaque ambiguous mutation through its configured status tool only.
    /// Every success or failure is bound to that exact recovery authorization.
    ///
    /// # Errors
    ///
    /// Returns a stable provenance-bound error when the authorization targets
    /// another integration or the operation cannot be started; an invalid
    /// status is a provenance-bound `StillUnknown` outcome.
    #[inline]
    pub async fn reconcile(
        &mut self,
        authorization: &AuthorizedReconciliation,
        options: RequestOptions,
    ) -> Result<BoundReconciliationOutcome, RmcpReconciliationError> {
        let operation = RmcpClientOperation::ReconcileMutation;
        let failure = |kind| RmcpClientError::new(kind, operation);
        let bind_failure = |error| RmcpReconciliationError {
            error,
            provenance: authorization.bind_failure(),
        };
        let deadline = match operation_deadline(&options, operation) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(bind_failure(error));
            }
        };
        if let Err(error) = self.ensure_timeout(&options, operation) {
            self.close_if_connection_failure(error).await;
            return Err(bind_failure(error));
        }
        if authorization.integration() != &self.integration {
            return Err(bind_failure(failure(
                RmcpClientErrorKind::IntegrationMismatch,
            )));
        }
        let request = match call_request(
            authorization.status_tool(),
            &authorization.arguments(),
            operation,
        ) {
            Ok(request) => request,
            Err(error) => return Err(bind_failure(error)),
        };
        let request_options = match remaining_options(&options, deadline, operation) {
            Ok(remaining) => remaining,
            Err(error) => {
                self.close_if_connection_failure(error).await;
                return Err(bind_failure(error));
            }
        };
        let Ok(ServerResult::CallToolResult(reconciliation_result)) =
            self.request(request, &request_options, operation).await
        else {
            self.close_inner().await;
            return Ok(authorization.bind_outcome(ReconciliationOutcome::StillUnknown));
        };
        if bounded_call_result(&reconciliation_result, operation).is_err() {
            self.close_inner().await;
            return Ok(authorization.bind_outcome(ReconciliationOutcome::StillUnknown));
        }
        let reconciliation_status = StrictReconciliationStatus::from(&reconciliation_result);
        let outcome = match reconciliation_status {
            StrictReconciliationStatus::Committed => ReconciliationOutcome::Committed,
            StrictReconciliationStatus::NotCommitted => ReconciliationOutcome::NotCommitted,
            StrictReconciliationStatus::StillUnknown => ReconciliationOutcome::StillUnknown,
        };
        Ok(authorization.bind_outcome(outcome))
    }

    /// Cancels the service and explicitly kills and reaps a spawned stdio child.
    #[inline]
    pub async fn close(&mut self) {
        self.close_inner().await;
    }

    /// Returns one bounded notification only when an exact opaque observation authorization permits it.
    ///
    /// Notifications of each kind retain at most a fixed number of pending values;
    /// excess server telemetry is deliberately dropped rather than growing memory.
    ///
    /// # Errors
    ///
    /// Returns an integration-mismatch error when the opaque observation token
    /// is not bound to this live connection.
    #[inline]
    pub fn try_next_observation(
        &mut self,
        authorization: &AuthorizedServerObservation,
    ) -> Result<Option<ServerObservation>, RmcpClientError> {
        if authorization.integration() != &self.integration {
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::IntegrationMismatch,
                RmcpClientOperation::ObserveServer,
            ));
        }
        Ok(self.observations.try_recv(authorization.kind()))
    }

    /// Creates one transport selected by trusted integration configuration.
    #[expect(
        clippy::pattern_type_mismatch,
        clippy::too_many_lines,
        reason = "matching the borrowed core configuration avoids cloning trusted transport values"
    )]
    async fn connect(
        integration: McpIntegration,
        root_declaration: Option<AuthorizedRootDeclaration>,
        options: RequestOptions,
        requested_operation: RmcpClientOperation,
    ) -> Result<Self, RmcpClientError> {
        if options.timeout.is_zero() {
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::InvalidTimeout,
                requested_operation,
            ));
        }
        if options.cancellation.is_cancelled() {
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::Cancelled,
                requested_operation,
            ));
        }
        match integration.transport() {
            McpTransport::Stdio { program, arguments } => {
                let mut command = Command::new(program.as_path());
                command
                    .args(arguments.iter().map(LiteralArgument::as_str))
                    // Direct stdio configuration grants argv execution, not
                    // ambient repository or credential context. Authorized
                    // roots remain available only through the MCP roots callback.
                    .env_clear()
                    .current_dir("/")
                    .kill_on_drop(true)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(source) => {
                        let kind = source.kind();
                        return Err(RmcpClientError::caused(
                            RmcpClientErrorKind::ProcessUnavailable,
                            RmcpClientOperation::StdioProcessSetup,
                            RmcpClientCause::ProcessSpawn(kind),
                            retryability_for_process_io(kind),
                        ));
                    }
                };
                let Some(stdin) = child.stdin.take() else {
                    terminate_child(&mut child).await;
                    return Err(RmcpClientError::caused(
                        RmcpClientErrorKind::ProcessUnavailable,
                        RmcpClientOperation::StdioProcessSetup,
                        RmcpClientCause::StandardInputUnavailable,
                        RmcpClientRetryability::Permanent,
                    ));
                };
                let Some(stdout) = child.stdout.take() else {
                    terminate_child(&mut child).await;
                    return Err(RmcpClientError::caused(
                        RmcpClientErrorKind::ProcessUnavailable,
                        RmcpClientOperation::StdioProcessSetup,
                        RmcpClientCause::StandardOutputUnavailable,
                        RmcpClientRetryability::Permanent,
                    ));
                };
                let cancellation = CancellationToken::new();
                let health = ConnectionHealth::new();
                let (sender, receiver) = mpsc::channel(1);
                let reader = BoundedStdioReader {
                    buffered: Vec::new(),
                    discard_until_newline: false,
                    maximum_bytes: MAX_STDIO_MESSAGE_BYTES,
                    reader: stdout,
                };
                drop(tokio::spawn(reader.run(
                    sender,
                    health.clone(),
                    cancellation.clone(),
                )));
                let transport =
                    SinkStreamTransport::new(BoundedStdioWriter(stdin).into_sink(), receiver);
                Self::initialize_with_roots(
                    integration,
                    root_declaration,
                    transport,
                    Some(child),
                    health,
                    cancellation,
                    options,
                    requested_operation,
                )
                .await
            }
            McpTransport::StreamableHttp { endpoint } => {
                let cancellation = CancellationToken::new();
                let http_client = match BoundedHttpClient::new(cancellation.clone()) {
                    Ok(client) => client,
                    Err(error) => return Err(error),
                };
                let mut configuration =
                    StreamableHttpClientTransportConfig::with_uri(endpoint.as_str())
                        .max_sse_event_size(MAX_HTTP_RESPONSE_BYTES)
                        // Never let RMCP recreate a session then replay an in-flight request.
                        .reinit_on_expired_session(false);
                configuration.channel_buffer_capacity = 1;
                let never_retry = Arc::new(NeverRetry::default());
                configuration.retry_config = never_retry;
                let health = ConnectionHealth::new();
                let transport =
                    StreamableHttpClientTransport::with_client(http_client, configuration);
                Self::initialize_with_roots(
                    integration,
                    root_declaration,
                    transport,
                    None,
                    health,
                    cancellation,
                    options,
                    requested_operation,
                )
                .await
            }
        }
    }

    /// Runs RMCP initialization and rejects every unsupported advertised capability.
    #[expect(
        clippy::ignored_unit_patterns,
        clippy::integer_division_remainder_used,
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the Tokio select macro gives cancellation precedence during initialization"
    )]
    async fn initialize_with_roots<T, E, A>(
        integration: McpIntegration,
        root_declaration: Option<AuthorizedRootDeclaration>,
        transport: T,
        child_process: Option<Child>,
        health: ConnectionHealth,
        cancellation: CancellationToken,
        options: RequestOptions,
        requested_operation: RmcpClientOperation,
    ) -> Result<Self, RmcpClientError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: Error + Send + Sync + 'static,
    {
        let mut child_to_reap = child_process;
        if root_declaration
            .as_ref()
            .is_some_and(|authorization| authorization.integration() != &integration)
        {
            cancellation.cancel();
            if let Some(process) = child_to_reap.as_mut() {
                terminate_child(process).await;
            }
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::IntegrationMismatch,
                requested_operation,
            ));
        }
        let (observation_sender, observation_receiver) = ObservationSenders::channels();
        let handler = RestrictedClientHandler {
            cancellation: cancellation.clone(),
            health: health.clone(),
            observations: observation_sender,
            root_declaration,
        };
        let initialization = handler.serve_with_ct(transport, cancellation.clone());
        tokio::pin!(initialization);
        let service = tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => {
                cancellation.cancel();
                if let Some(process) = child_to_reap.as_mut() {
                    terminate_child(process).await;
                }
                return Err(RmcpClientError::new(
                    RmcpClientErrorKind::Cancelled,
                    requested_operation,
                ));
            }
            result = timeout(options.timeout, &mut initialization) => match result {
                Ok(Ok(service)) => service,
                Ok(Err(_source)) => {
                    cancellation.cancel();
                    if let Some(process) = child_to_reap.as_mut() {
                        terminate_child(process).await;
                    }
                    return Err(health
                        .client_error(RmcpClientOperation::Initialize)
                        .unwrap_or(RmcpClientError::caused(
                            RmcpClientErrorKind::InitializationFailed,
                            RmcpClientOperation::Initialize,
                            RmcpClientCause::ProtocolInitialization,
                            RmcpClientRetryability::Retryable,
                        )));
                }
                Err(_elapsed) => {
                    cancellation.cancel();
                    if let Some(process) = child_to_reap.as_mut() {
                        terminate_child(process).await;
                    }
                    return Err(RmcpClientError::new(
                        RmcpClientErrorKind::TimedOut,
                        RmcpClientOperation::Initialize,
                    ));
                }
            },
        };
        let mut client = Self {
            cancellation,
            child: child_to_reap,
            health,
            integration,
            observations: observation_receiver,
            service,
        };
        if let Some(error) = client.health.client_error(RmcpClientOperation::Initialize) {
            client.close_inner().await;
            return Err(error);
        }
        let Some(peer_info) = client.service.peer().peer_info() else {
            client.close_inner().await;
            return Err(RmcpClientError::caused(
                RmcpClientErrorKind::InitializationFailed,
                RmcpClientOperation::Initialize,
                RmcpClientCause::ProtocolInitialization,
                RmcpClientRetryability::Retryable,
            ));
        };
        if peer_info.protocol_version >= ProtocolVersion::STANDARD_HEADERS {
            client.close_inner().await;
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::UnsupportedServerCapability,
                RmcpClientOperation::NegotiateCapabilities,
            ));
        }
        if peer_info.capabilities.supports_tasks() {
            client.close_inner().await;
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::TasksUnsupported,
                RmcpClientOperation::NegotiateCapabilities,
            ));
        }
        if peer_info.capabilities.tools.is_none() {
            client.close_inner().await;
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::ToolsCapabilityRequired,
                RmcpClientOperation::NegotiateCapabilities,
            ));
        }
        if peer_info.capabilities.completions.is_some()
            || peer_info.capabilities.experimental.is_some()
            || peer_info.capabilities.extensions.is_some()
        {
            client.close_inner().await;
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::UnsupportedServerCapability,
                RmcpClientOperation::NegotiateCapabilities,
            ));
        }
        Ok(client)
    }

    /// Initializes a deterministic fixture without granting a roots declaration.
    #[cfg(test)]
    async fn initialize<T, E, A>(
        integration: McpIntegration,
        transport: T,
        child_process: Option<Child>,
        health: ConnectionHealth,
        cancellation: CancellationToken,
        options: RequestOptions,
    ) -> Result<Self, RmcpClientError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: Error + Send + Sync + 'static,
    {
        Self::initialize_with_roots(
            integration,
            None,
            transport,
            child_process,
            health,
            cancellation,
            options,
            RmcpClientOperation::Initialize,
        )
        .await
    }

    /// Checks caller and connection cancellation before a new operation stage.
    fn ensure_timeout(
        &self,
        options: &RequestOptions,
        operation: RmcpClientOperation,
    ) -> Result<(), RmcpClientError> {
        if options.timeout.is_zero() {
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::InvalidTimeout,
                operation,
            ));
        }
        if options.cancellation.is_cancelled() {
            return Err(RmcpClientError::new(
                RmcpClientErrorKind::Cancelled,
                operation,
            ));
        }
        if let Some(error) = self.health.client_error(operation) {
            return Err(error);
        }
        if self.cancellation.is_cancelled() {
            return Err(RmcpClientError::caused(
                RmcpClientErrorKind::TransportLost,
                operation,
                RmcpClientCause::TransportClosed,
                RmcpClientRetryability::Retryable,
            ));
        }
        Ok(())
    }

    /// Returns whether the initialized peer explicitly advertised resources.
    fn supports_resources(&self) -> bool {
        self.service
            .peer()
            .peer_info()
            .is_some_and(|peer_info| peer_info.capabilities.resources.is_some())
    }

    /// Returns whether the initialized peer explicitly advertised prompts.
    fn supports_prompts(&self) -> bool {
        self.service
            .peer()
            .peer_info()
            .is_some_and(|peer_info| peer_info.capabilities.prompts.is_some())
    }

    /// Terminally closes the connection for errors that invalidate future use.
    async fn close_if_connection_failure(&mut self, error: RmcpClientError) {
        if matches!(
            error.kind(),
            RmcpClientErrorKind::Cancelled
                | RmcpClientErrorKind::TimedOut
                | RmcpClientErrorKind::TransportLost
                | RmcpClientErrorKind::UnsupportedServerCapability
        ) {
            self.close_inner().await;
        }
    }

    /// Enqueues and awaits exactly one RMCP request within the supplied remaining budget.
    #[expect(
        clippy::ignored_unit_patterns,
        clippy::integer_division_remainder_used,
        reason = "the Tokio select macro gives cancellation and deadline precedence around dispatch"
    )]
    async fn request(
        &mut self,
        request: ClientRequest,
        options: &RequestOptions,
        operation: RmcpClientOperation,
    ) -> Result<ServerResult, RequestFailure> {
        if self
            .health
            .client_error(operation)
            .is_some_and(|error| error.kind() == RmcpClientErrorKind::UnsupportedServerCapability)
        {
            return Err(RequestFailure::UnsupportedServerCapability);
        }
        if self.health.client_error(operation).is_some() || self.cancellation.is_cancelled() {
            return Err(RequestFailure::TransportLost);
        }
        let deadline = sleep(options.timeout);
        tokio::pin!(deadline);
        let peer = self.service.peer().clone();
        let enqueue = peer.send_cancellable_request(request, PeerRequestOptions::no_options());
        tokio::pin!(enqueue);
        let enqueue_result = tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => {
                self.cancellation.cancel();
                Err(RequestFailure::Cancelled)
            }
            _ = self.cancellation.cancelled() => {
                if self
                    .health
                    .client_error(operation)
                    .is_some_and(|error| error.kind() == RmcpClientErrorKind::UnsupportedServerCapability)
                {
                    Err(RequestFailure::UnsupportedServerCapability)
                } else {
                    Err(RequestFailure::TransportLost)
                }
            }
            _ = &mut deadline => {
                self.cancellation.cancel();
                Err(RequestFailure::TimedOut)
            }
            result = &mut enqueue => result.map_err(|_source| RequestFailure::TransportLost),
        };
        let mut handle = match enqueue_result {
            Ok(request_handle) => request_handle,
            Err(failure) => return Err(failure),
        };
        tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => {
                self.cancellation.cancel();
                cancel_request(handle, "caller cancelled request");
                Err(RequestFailure::Cancelled)
            }
            _ = self.cancellation.cancelled() => {
                cancel_request(handle, "connection cancelled request");
                if self
                    .health
                    .client_error(operation)
                    .is_some_and(|error| error.kind() == RmcpClientErrorKind::UnsupportedServerCapability)
                {
                    Err(RequestFailure::UnsupportedServerCapability)
                } else {
                    Err(RequestFailure::TransportLost)
                }
            }
            _ = &mut deadline => {
                self.cancellation.cancel();
                cancel_request(handle, "request timeout");
                Err(RequestFailure::TimedOut)
            }
            response_result = &mut handle.rx => match response_result {
                Ok(Ok(server_response)) => Ok(server_response),
                Ok(Err(_)) | Err(_) => Err(RequestFailure::TransportLost),
            },
        }
    }

    /// Converts a post-dispatch mutating failure into its exact reconciliation outcome.
    async fn ambiguous_or_error(
        &mut self,
        authorization: AuthorizedToolCall,
        class: ToolClass,
        error: RmcpClientError,
    ) -> Result<BoundToolCallOutcome, RmcpToolCallError> {
        self.close_inner().await;
        if class == ToolClass::Mutate {
            return authorization
                .bind_ambiguity()
                .map_err(|provenance| RmcpToolCallError::new(error, provenance));
        }
        Err(RmcpToolCallError::new(error, authorization.bind_failure()))
    }

    /// Cancels active work, closes RMCP, and reaps a direct stdio child.
    async fn close_inner(&mut self) {
        self.cancellation.cancel();
        drop(self.service.close_with_timeout(SHUTDOWN_TIMEOUT).await);
        if let Some(child) = self.child.as_mut() {
            terminate_child(child).await;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Internal request state used to preserve dispatch ambiguity semantics.
enum RequestFailure {
    /// The caller cancelled before an observed response.
    Cancelled,
    /// The request budget elapsed before an observed response.
    TimedOut,
    /// The peer or transport closed without an observed response.
    TransportLost,
    /// The peer exercised an unsupported capability during the request.
    UnsupportedServerCapability,
}

#[expect(
    clippy::implicit_return,
    reason = "the total request-failure projection is clearest as a tail match"
)]
impl RequestFailure {
    /// Converts one internal request failure into the stable public error.
    const fn into_client_error(self, operation: RmcpClientOperation) -> RmcpClientError {
        match self {
            Self::Cancelled => RmcpClientError::new(RmcpClientErrorKind::Cancelled, operation),
            Self::TimedOut => RmcpClientError::new(RmcpClientErrorKind::TimedOut, operation),
            Self::TransportLost => RmcpClientError::caused(
                RmcpClientErrorKind::TransportLost,
                operation,
                RmcpClientCause::TransportClosed,
                RmcpClientRetryability::Retryable,
            ),
            Self::UnsupportedServerCapability => {
                RmcpClientError::new(RmcpClientErrorKind::UnsupportedServerCapability, operation)
            }
        }
    }
}

/// Strictly parsed terminal status from the configured reconciliation tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StrictReconciliationStatus {
    /// The mutation is proven committed by an exact non-error status result.
    Committed,
    /// The mutation is proven not committed by an exact non-error status result.
    NotCommitted,
    /// No exact, successful status result proved either terminal state.
    StillUnknown,
}

#[expect(
    clippy::implicit_return,
    reason = "the conservative exact-status conversion is clearest as a total tail match"
)]
impl From<&CallToolResult> for StrictReconciliationStatus {
    fn from(result: &CallToolResult) -> Self {
        if result.is_error == Some(true) {
            return Self::StillUnknown;
        }
        let Some(status) = result
            .structured_content
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .filter(|object| object.len() == 1)
            .and_then(|object| object.get("status"))
            .and_then(serde_json::Value::as_str)
        else {
            return Self::StillUnknown;
        };
        match status {
            "committed" => Self::Committed,
            "not_committed" => Self::NotCommitted,
            _ => Self::StillUnknown,
        }
    }
}

/// Errors raised by the bounded loopback HTTP transport.
#[derive(Debug)]
enum BoundedHttpClientError {
    /// A JSON body or SSE event exceeded a Tiber-owned bound.
    BodyTooLarge,
    /// The connection-wide cancellation bridge interrupted active I/O.
    Cancelled,
    /// Reqwest could not complete a bounded request.
    Request(reqwest::Error),
    /// RMCP's message could not be serialized into an HTTP body.
    RequestSerialization,
    /// Session cleanup exceeded its independent bounded grace period.
    ShutdownTimedOut,
}

#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the borrowed external error projection is clearest as a total display match"
)]
impl fmt::Display for BoundedHttpClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge => f.write_str("bounded HTTP body exceeded limit"),
            Self::Cancelled => f.write_str("bounded HTTP request cancelled"),
            Self::Request(_source) => f.write_str("HTTP request failed"),
            Self::RequestSerialization => f.write_str("HTTP request serialization failed"),
            Self::ShutdownTimedOut => f.write_str("bounded HTTP cleanup timed out"),
        }
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "only the causal reqwest error is exposed from this bounded external-error adapter"
)]
impl Error for BoundedHttpClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(source) => Some(source),
            Self::BodyTooLarge
            | Self::Cancelled
            | Self::RequestSerialization
            | Self::ShutdownTimedOut => None,
        }
    }
}

#[expect(
    clippy::implicit_return,
    reason = "the direct reqwest-error wrapper is clearest as one tail expression"
)]
impl From<reqwest::Error> for BoundedHttpClientError {
    fn from(source: reqwest::Error) -> Self {
        Self::Request(source)
    }
}

/// HTTP response classes admitted by the Streamable HTTP adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseContentType {
    /// A request-scoped server-sent-event response.
    EventStream,
    /// A single JSON-RPC response body.
    Json,
    /// Any unadmitted or absent media type.
    Other,
}

/// Errors emitted by the raw SSE byte limiter before RMCP can reconnect.
#[derive(Debug)]
enum BoundedSseError {
    /// The accumulated non-comment event bytes exceeded the bound.
    EventTooLarge,
    /// The reqwest source stream failed before a complete event.
    Source(reqwest::Error),
}

#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the borrowed SSE error projection is clearest as a total display match"
)]
impl fmt::Display for BoundedSseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventTooLarge => f.write_str("bounded SSE event exceeded limit"),
            Self::Source(_source) => f.write_str("bounded SSE source failed"),
        }
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "only the underlying reqwest source is meaningful as an SSE causal error"
)]
impl Error for BoundedSseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(source) => Some(source),
            Self::EventTooLarge => None,
        }
    }
}

#[derive(Clone)]
/// Loopback-only reqwest client with terminal cancellation and body bounds.
struct BoundedHttpClient {
    /// Bridge that drops active post/get/body/SSE futures on terminal failure.
    cancellation: CancellationToken,
    /// No-proxy, no-redirect, no-retry reqwest handle shared by one session.
    client: reqwest::Client,
}

#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the named HTTP construction and bounded send helpers keep the no-replay boundary explicit"
)]
impl BoundedHttpClient {
    /// Constructs the explicitly non-replaying loopback HTTP client.
    fn new(cancellation: CancellationToken) -> Result<Self, RmcpClientError> {
        let client = reqwest::Client::builder()
            .connect_timeout(SHUTDOWN_TIMEOUT)
            .no_proxy()
            .redirect(Policy::none())
            .retry(retry::never())
            .build()
            .map_err(|_source| {
                RmcpClientError::caused(
                    RmcpClientErrorKind::HttpClientUnavailable,
                    RmcpClientOperation::HttpClientSetup,
                    RmcpClientCause::HttpClientConstruction,
                    RmcpClientRetryability::Permanent,
                )
            })?;
        Ok(Self {
            cancellation,
            client,
        })
    }

    /// Sends an active protocol request unless terminal cancellation fires first.
    #[expect(
        clippy::ignored_unit_patterns,
        clippy::integer_division_remainder_used,
        reason = "the Tokio select macro gives terminal cancellation precedence over the active HTTP future"
    )]
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, BoundedHttpClientError> {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(BoundedHttpClientError::Cancelled),
            response = request.send() => response.map_err(BoundedHttpClientError::Request),
        }
    }

    /// Sends only bounded session deletion after active work has been cancelled.
    async fn send_cleanup(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, BoundedHttpClientError> {
        // Closing an established session is the only request permitted after
        // the connection-wide cancellation bridge fires. It has its own short
        // deadline so it cannot delay a caller whose active tool request was
        // already aborted.
        timeout(SHUTDOWN_TIMEOUT, request.send())
            .await
            .map_err(|_elapsed| BoundedHttpClientError::ShutdownTimedOut)?
            .map_err(BoundedHttpClientError::Request)
    }
}

#[derive(Debug)]
/// Incremental size accounting for one untrusted SSE event stream.
struct SseEventSizeLimiter {
    /// Whether the current line is an ignored SSE comment line.
    line_is_comment: bool,
    /// Bytes accumulated on the current SSE field line.
    line_size: usize,
    /// Maximum retained non-comment event bytes.
    max_size: usize,
    /// Whether the preceding byte was a carriage return awaiting line completion.
    previous_was_cr: bool,
    /// Bytes retained across lines of the current SSE event.
    retained_size: usize,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the limiter stays ordered as construct, observe, commit, and check to make the bound invariant auditable"
)]
impl SseEventSizeLimiter {
    /// Creates an event limiter with a fixed retained-byte cap.
    fn new(max_size: usize) -> Self {
        Self {
            line_is_comment: false,
            line_size: 0,
            max_size,
            previous_was_cr: false,
            retained_size: 0,
        }
    }

    /// Accounts for one raw chunk and rejects an oversized event before parsing.
    fn observe(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_was_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    if self.line_size == 0 {
                        self.line_is_comment = byte == b':';
                    }
                    self.line_size = self.line_size.saturating_add(1);
                    self.check_limit()?;
                }
            }
        }
        Ok(())
    }

    /// Commits one SSE line into the retained-event budget.
    fn finish_line(&mut self) -> Result<(), ()> {
        if self.line_size == 0 {
            self.retained_size = 0;
        }
        if self.line_size != 0 && !self.line_is_comment {
            self.retained_size = self
                .retained_size
                .saturating_add(self.line_size)
                .saturating_add(1);
        }
        self.line_size = 0;
        self.line_is_comment = false;
        self.check_limit()
    }

    /// Checks the current accumulated event budget.
    fn check_limit(&self) -> Result<(), ()> {
        if self.retained_size.saturating_add(self.line_size) > self.max_size {
            return Err(());
        }
        Ok(())
    }
}

/// Raw reqwest byte stream that terminates on cancellation or an inbound bound violation.
struct BoundedSseByteStream {
    /// Terminal connection cancellation bridge.
    cancellation: CancellationToken,
    /// Whether one terminal error has already been yielded.
    failed: bool,
    /// Underlying reqwest byte stream.
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    /// Incremental event-size accounting.
    limiter: SseEventSizeLimiter,
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    reason = "the futures Stream trait fixes the poll signature and total bounded-byte projection"
)]
impl Stream for BoundedSseByteStream {
    type Item = Result<Bytes, BoundedSseError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.failed || this.cancellation.is_cancelled() {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(chunk))) => {
                if this.limiter.observe(&chunk).is_err() {
                    this.failed = true;
                    return Poll::Ready(Some(Err(BoundedSseError::EventTooLarge)));
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(source))) => {
                this.failed = true;
                Poll::Ready(Some(Err(BoundedSseError::Source(source))))
            }
            Poll::Ready(None) => Poll::Ready(None),
        }
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    reason = "RMCP's externally fixed HTTP trait signatures and protocol lifecycle require borrowed DTO paths and sequential request state"
)]
impl StreamableHttpClient for BoundedHttpClient {
    type Error = BoundedHttpClientError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            MAX_HTTP_RESPONSE_BYTES,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let body = serde_json::to_vec(&message).map_err(|_source| {
            StreamableHttpError::Client(BoundedHttpClientError::RequestSerialization)
        })?;
        if body.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(StreamableHttpError::Client(
                BoundedHttpClientError::BodyTooLarge,
            ));
        }
        let session_was_attached = session_id.is_some();
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(session_id) = session_id {
            request = request.header("mcp-session-id", session_id.as_ref());
        }
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        let response = self
            .send(apply_protocol_headers(request, custom_headers))
            .await
            .map_err(StreamableHttpError::Client)?;
        let status = response.status();
        let session_id = response_session_id(&response)?;
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response_content_type(&response);
        if !status.is_success() {
            return non_success_response(response, content_type, session_id, &self.cancellation)
                .await;
        }
        match content_type {
            ResponseContentType::EventStream => Ok(StreamableHttpPostResponse::Sse(
                bounded_sse_stream(
                    response.bytes_stream(),
                    max_sse_event_size,
                    self.cancellation.clone(),
                ),
                session_id,
            )),
            ResponseContentType::Json => {
                let body = read_bounded_body(response, &self.cancellation)
                    .await
                    .map_err(StreamableHttpError::Client)?;
                let message =
                    serde_json::from_slice(&body).map_err(StreamableHttpError::Deserialize)?;
                Ok(StreamableHttpPostResponse::Json(message, session_id))
            }
            ResponseContentType::Other => Err(StreamableHttpError::UnexpectedContentType(None)),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request = self
            .client
            .delete(uri.as_ref())
            .header("mcp-session-id", session_id.as_ref());
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        let response = self
            .send_cleanup(apply_protocol_headers(request, custom_headers))
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED || response.status().is_success() {
            return Ok(());
        }
        Err(StreamableHttpError::UnexpectedServerResponse(
            "bounded HTTP session deletion failed".into(),
        ))
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            MAX_HTTP_RESPONSE_BYTES,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        if last_event_id.is_some() {
            // RMCP's request-stream resume path is a hidden retry route. It
            // must never reach the configured server for a mutation.
            return Err(StreamableHttpError::UnexpectedServerResponse(
                "SSE stream resumption is unsupported by the bounded tools client".into(),
            ));
        }
        let mut request = self
            .client
            .get(uri.as_ref())
            .header(ACCEPT, "text/event-stream");
        if let Some(session_id) = session_id {
            request = request.header("mcp-session-id", session_id.as_ref());
        }
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        let response = self
            .send(apply_protocol_headers(request, custom_headers))
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if !response.status().is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(
                "bounded SSE request failed".into(),
            ));
        }
        if response_content_type(&response) != ResponseContentType::EventStream {
            return Err(StreamableHttpError::UnexpectedContentType(None));
        }
        Ok(bounded_sse_stream(
            response.bytes_stream(),
            max_sse_event_size,
            self.cancellation.clone(),
        ))
    }
}

/// Returns fixed guidance without incorporating configured or server-provided text.
#[expect(
    clippy::implicit_return,
    reason = "the stable recovery table is clearest as a total tail match"
)]
const fn recovery_action(kind: RmcpClientErrorKind) -> &'static str {
    match kind {
        RmcpClientErrorKind::Cancelled
        | RmcpClientErrorKind::TimedOut
        | RmcpClientErrorKind::InvalidTimeout => {
            "use a fresh cancellation token and a sufficient bounded timeout"
        }
        RmcpClientErrorKind::ProcessUnavailable => {
            "check the configured executable and host process availability"
        }
        RmcpClientErrorKind::HttpClientUnavailable => {
            "check the fixed loopback HTTP client configuration"
        }
        RmcpClientErrorKind::InitializationFailed => {
            "check that the configured server completes a valid MCP handshake"
        }
        RmcpClientErrorKind::ToolsCapabilityRequired
        | RmcpClientErrorKind::TasksUnsupported
        | RmcpClientErrorKind::UnsupportedServerCapability => {
            "use a server that advertises only the supported bounded MCP capabilities"
        }
        RmcpClientErrorKind::IntegrationMismatch => {
            "request a fresh authorization for this exact configured integration"
        }
        RmcpClientErrorKind::InvalidServerResponse
        | RmcpClientErrorKind::UntrustedPayloadTooLarge
        | RmcpClientErrorKind::InvalidToolArguments => {
            "correct the bounded request or server response before retrying"
        }
        RmcpClientErrorKind::ToolListLimitExceeded
        | RmcpClientErrorKind::OptionalCatalogLimitExceeded => {
            "reduce the configured server catalog to Tiber's fixed bound"
        }
        RmcpClientErrorKind::TransportLost => {
            "establish a fresh authorized connection before a bounded retry"
        }
    }
}

/// Establishes the absolute deadline shared by every stage of one operation.
#[expect(
    clippy::implicit_return,
    reason = "the checked deadline construction is clearest as the function tail expression"
)]
fn operation_deadline(
    options: &RequestOptions,
    operation: RmcpClientOperation,
) -> Result<Instant, RmcpClientError> {
    if options.timeout.is_zero() {
        return Err(RmcpClientError::new(
            RmcpClientErrorKind::InvalidTimeout,
            operation,
        ));
    }
    if options.cancellation.is_cancelled() {
        return Err(RmcpClientError::new(
            RmcpClientErrorKind::Cancelled,
            operation,
        ));
    }
    Instant::now()
        .checked_add(options.timeout)
        .ok_or(RmcpClientError::new(
            RmcpClientErrorKind::InvalidTimeout,
            operation,
        ))
}

/// Derives the remaining budget without extending the enclosing operation deadline.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the checked remaining-budget expression remains concise and preserves one deadline"
)]
fn remaining_options(
    options: &RequestOptions,
    deadline: Instant,
    operation: RmcpClientOperation,
) -> Result<RequestOptions, RmcpClientError> {
    if options.cancellation.is_cancelled() {
        return Err(RmcpClientError::new(
            RmcpClientErrorKind::Cancelled,
            operation,
        ));
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(RmcpClientError::new(
            RmcpClientErrorKind::TimedOut,
            operation,
        ))?;
    Ok(RequestOptions::new(remaining, options.cancellation.clone()))
}

/// Builds one tools/call request from already-authorized opaque arguments.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the parsing boundary preserves opaque arguments and its concise error path"
)]
fn call_request(
    tool: &ToolName,
    arguments: &ToolArguments,
    operation: RmcpClientOperation,
) -> Result<ClientRequest, RmcpClientError> {
    let parsed_arguments = serde_json::from_str(arguments.as_json()).map_err(|_source| {
        RmcpClientError::new(RmcpClientErrorKind::InvalidToolArguments, operation)
    })?;
    let params =
        CallToolRequestParams::new(tool.as_str().to_owned()).with_arguments(parsed_arguments);
    Ok(ClientRequest::CallToolRequest(CallToolRequest::new(params)))
}

/// Projects a complete untrusted tool result into one bounded payload.
#[expect(
    clippy::implicit_return,
    reason = "the direct bounded serialization projection is clearest as a tail expression"
)]
fn bounded_call_result(
    result: &CallToolResult,
    operation: RmcpClientOperation,
) -> Result<UntrustedPayload, RmcpClientError> {
    bounded_serialized_payload(serde_json::to_string(result), operation)
}

/// Bounds a serialized untrusted server value before it reaches client callers.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the serialization boundary keeps malformed and oversized mappings local"
)]
fn bounded_serialized_payload(
    serialized: Result<String, serde_json::Error>,
    operation: RmcpClientOperation,
) -> Result<UntrustedPayload, RmcpClientError> {
    let bounded_input = serialized.map_err(|_source| {
        RmcpClientError::new(RmcpClientErrorKind::InvalidServerResponse, operation)
    })?;
    UntrustedPayload::bounded(&bounded_input).map_err(|_source| {
        RmcpClientError::new(RmcpClientErrorKind::UntrustedPayloadTooLarge, operation)
    })
}

/// Sends a bounded best-effort cancellation hint after the connection aborts.
fn cancel_request(handle: RequestHandle<RoleClient>, reason: &str) {
    let cancellation_reason = reason.to_owned();
    let cancellation = async move {
        let request = handle.cancel(Some(cancellation_reason));
        drop(timeout(SHUTDOWN_TIMEOUT, request).await);
    };
    // Closing this terminal connection immediately cancels an in-flight HTTP
    // request. The protocol cancellation is only a bounded best-effort hint.
    drop(tokio::spawn(cancellation));
}

/// Kills and reaps the direct child unless it has already exited.
async fn terminate_child(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) | Err(_) => {}
        Ok(None) => {
            drop(child.kill().await);
        }
    }
}

/// Creates the raw bounded stdio transport instead of RMCP's unbounded reader.
#[cfg(test)]
#[expect(
    clippy::implicit_return,
    reason = "the deterministic fixture transport constructor is clearest as a tail expression"
)]
fn bounded_stdio_transport<R, W>(
    reader: R,
    writer: W,
    health: ConnectionHealth,
    cancellation: CancellationToken,
) -> SinkStreamTransport<
    impl Sink<TxJsonRpcMessage<RoleClient>, Error = io::Error> + Send + Unpin + 'static,
    mpsc::Receiver<RxJsonRpcMessage<RoleClient>>,
>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let (sender, receiver) = mpsc::channel(1);
    let bounded_reader = BoundedStdioReader {
        buffered: Vec::new(),
        discard_until_newline: false,
        maximum_bytes: MAX_STDIO_MESSAGE_BYTES,
        reader,
    };
    drop(tokio::spawn(bounded_reader.run(
        sender,
        health,
        cancellation,
    )));
    SinkStreamTransport::new(BoundedStdioWriter(writer).into_sink(), receiver)
}

/// Applies RMCP-provided protocol headers without granting redirect or retry behavior.
#[expect(
    clippy::implicit_return,
    clippy::iter_over_hash_type,
    reason = "RMCP provides custom headers as a HashMap and their insertion order has no semantic effect"
)]
fn apply_protocol_headers(
    mut request: reqwest::RequestBuilder,
    headers: HashMap<HeaderName, HeaderValue>,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
}

/// Validates and bounds the optional server session identity response header.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the bounded header parser keeps its malformed-header conversion local"
)]
fn response_session_id(
    response: &reqwest::Response,
) -> Result<Option<String>, StreamableHttpError<BoundedHttpClientError>> {
    let Some(header_value) = response.headers().get("mcp-session-id") else {
        return Ok(None);
    };
    let session_id = header_value.to_str().map_err(|_source| {
        StreamableHttpError::UnexpectedServerResponse(
            "invalid bounded HTTP session identity".into(),
        )
    })?;
    if session_id.is_empty()
        || session_id.len() > MAX_SEMANTIC_TEXT_BYTES
        || session_id.chars().any(char::is_control)
    {
        return Err(StreamableHttpError::UnexpectedServerResponse(
            "invalid bounded HTTP session identity".into(),
        ));
    }
    Ok(Some(session_id.to_owned()))
}

/// Classifies a response media type without trusting arbitrary header text.
#[expect(
    clippy::implicit_return,
    reason = "the conservative media-type classifier is clearest as a tail fallback"
)]
fn response_content_type(response: &reqwest::Response) -> ResponseContentType {
    let Some(header_value) = response.headers().get(CONTENT_TYPE) else {
        return ResponseContentType::Other;
    };
    let Ok(media_value) = header_value.to_str() else {
        return ResponseContentType::Other;
    };
    let mime_type = media_value.split(';').next().unwrap_or_default().trim();
    if mime_type.eq_ignore_ascii_case("application/json") {
        return ResponseContentType::Json;
    }
    if mime_type.eq_ignore_ascii_case("text/event-stream") {
        return ResponseContentType::EventStream;
    }
    ResponseContentType::Other
}

/// Retains only a bounded JSON-RPC error response from a non-successful POST.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the RMCP JSON-RPC bounded failure path remains explicit at this protocol boundary"
)]
async fn non_success_response(
    response: reqwest::Response,
    content_type: ResponseContentType,
    session_id: Option<String>,
    cancellation: &CancellationToken,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<BoundedHttpClientError>> {
    if content_type != ResponseContentType::Json {
        return Err(StreamableHttpError::UnexpectedServerResponse(
            "bounded HTTP request failed".into(),
        ));
    }
    let body = read_bounded_body(response, cancellation)
        .await
        .map_err(StreamableHttpError::Client)?;
    match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
        Ok(message @ JsonRpcMessage::Error(_)) => {
            Ok(StreamableHttpPostResponse::Json(message, session_id))
        }
        Ok(_) | Err(_) => Err(StreamableHttpError::UnexpectedServerResponse(
            "bounded HTTP request failed".into(),
        )),
    }
}

/// Reads a response body incrementally while enforcing the Tiber-owned cap.
#[expect(
    clippy::ignored_unit_patterns,
    clippy::implicit_return,
    clippy::integer_division_remainder_used,
    clippy::question_mark_used,
    reason = "the Tokio select macro enforces cancellation precedence while the body loop remains locally bounded"
)]
async fn read_bounded_body(
    response: reqwest::Response,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, BoundedHttpClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTTP_RESPONSE_BYTES_U64)
    {
        return Err(BoundedHttpClientError::BodyTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let next_chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(BoundedHttpClientError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(raw_chunk) = next_chunk else {
            break;
        };
        let body_chunk = raw_chunk.map_err(BoundedHttpClientError::Request)?;
        if body.len().saturating_add(body_chunk.len()) > MAX_HTTP_RESPONSE_BYTES {
            return Err(BoundedHttpClientError::BodyTooLarge);
        }
        body.extend_from_slice(&body_chunk);
    }
    Ok(body)
}

/// Classifies only the bounded host I/O kind retained from process spawn.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "ErrorKind is non-exhaustive; known transient spawn classes are retryable and unknown future classes default safely to permanent"
)]
const fn retryability_for_process_io(kind: io::ErrorKind) -> RmcpClientRetryability {
    match kind {
        io::ErrorKind::Interrupted
        | io::ErrorKind::WouldBlock
        | io::ErrorKind::TimedOut
        | io::ErrorKind::ResourceBusy => RmcpClientRetryability::Retryable,
        _ => RmcpClientRetryability::Permanent,
    }
}

/// Parses bounded SSE while preventing server directives from triggering a resume GET.
#[expect(
    clippy::implicit_return,
    reason = "the named adapter preserves bounded parser termination and clears server retry directives"
)]
fn bounded_sse_stream<S>(
    stream: S,
    max_event_size: usize,
    cancellation: CancellationToken,
) -> BoxStream<'static, Result<Sse, SseError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let bounded_byte_stream = BoundedSseByteStream {
        cancellation,
        failed: false,
        inner: stream.boxed(),
        limiter: SseEventSizeLimiter::new(max_event_size.min(MAX_HTTP_RESPONSE_BYTES)),
    };
    SseStream::from_bytes_stream(bounded_byte_stream)
        .filter_map(|parsed_event| async move {
            match parsed_event {
                Ok(mut sse_event) => {
                    // RMCP's reconnect wrapper treats a server-provided retry
                    // field as stronger than `NeverRetry`. Clear it before the
                    // wrapper sees the event, so EOF stays terminal.
                    sse_event.retry = None;
                    Some(Ok(sse_event))
                }
                // Parser, raw-limit, and source errors must terminate the
                // stream, never invoke RMCP's reconnect-on-error branch.
                Err(_source) => None,
            }
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use core::{
        error::Error as _,
        str,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use std::{env, fs, io::ErrorKind, os::unix::fs::PermissionsExt as _, process};

    use serde_json::{Value, json};
    use tiber_external_tools_core::{
        AbsoluteProgram, AgentRole, AssignmentId, AuthorizationContext, AuthorizedPromptGet,
        AuthorizedReconciliation, AuthorizedResourceListing, AuthorizedResourceRead,
        AuthorizedServerObservation, AuthorizedToolCall, AuthorizedToolListing, ConfiguredTool,
        ExternalToolCapability, ExternalToolError, IdempotencyKey, IntegrationId, LiteralArgument,
        LoopbackEndpoint, MAX_UNTRUSTED_PAYLOAD_BYTES, McpIntegration, McpTransport,
        OwnerApprovalId, PermissionGrant, PolicyDecisionId, PolicyIntersection, PromptArguments,
        PromptGetProposal, PromptName, ReconciliationOutcome, ResourceReadProposal, ResourceUri,
        ScopedPermission, ServerObservationKind, SessionId, TiberOwnedRoot, ToolArguments,
        ToolCallOutcome, ToolCallProposal, ToolClass, ToolName, WorkflowMode, authorize_prompt_get,
        authorize_prompt_listing, authorize_resource_listing, authorize_resource_read,
        authorize_root_declaration, authorize_server_observation, authorize_tool_call,
        authorize_tool_listing,
    };
    use tokio::{
        io::{
            AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
            BufReader,
        },
        net::{TcpListener, TcpStream},
        sync::Notify,
        task,
        time::timeout,
    };
    use tokio_util::sync::CancellationToken;

    use super::{
        BoundedStdioReader, ConnectionHealth, MAX_STDIO_MESSAGE_BYTES, RequestOptions, RmcpClient,
        RmcpClientCause, RmcpClientError, RmcpClientErrorKind, RmcpClientOperation,
        RmcpClientRetryability, ServerObservation, StdioReadError, bounded_stdio_transport,
    };

    struct HttpFixtureRequest {
        body: Value,
        headers: Vec<(String, String)>,
        method: String,
        path: String,
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture accessor uses an idiomatic tail expression"
    )]
    impl HttpFixtureRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|header| header.0 == name)
                .map(|header| header.1.as_str())
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture parsing must fail loudly and uses an idiomatic tail expression"
    )]
    fn text<T>(parse: impl FnOnce(&str) -> Result<T, ExternalToolError>, value: &str) -> T {
        parse(value).expect("fixture semantic text is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test projection keeps result assertions concise"
    )]
    fn error_kind(error: RmcpClientError) -> RmcpClientErrorKind {
        error.kind()
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the test predicate keeps asynchronous health polling concise"
    )]
    fn is_unsupported_error(error: RmcpClientError) -> bool {
        error.kind() == RmcpClientErrorKind::UnsupportedServerCapability
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture wrapper uses an idiomatic tail expression"
    )]
    fn tool(value: &str) -> ToolName {
        text(ToolName::parse, value)
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "trusted fixture construction must fail loudly and uses an idiomatic tail expression"
    )]
    fn integration() -> McpIntegration {
        McpIntegration::new(
            text(IntegrationId::parse, "fixture-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse("/usr/bin/fixture-mcp")
                    .expect("fixture program is absolute"),
                arguments: vec![
                    LiteralArgument::parse("--fixture").expect("fixture argv is literal"),
                ],
            },
            [
                ConfiguredTool::new(tool("inspect"), ToolClass::Observe),
                ConfiguredTool::new(tool("mutate"), ToolClass::Mutate),
                ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
            ],
            Some(tool("mutation_status")),
        )
        .expect("fixture configured integration is valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "trusted loopback fixture construction must fail loudly and uses an idiomatic tail expression"
    )]
    fn http_integration(endpoint: &str) -> McpIntegration {
        McpIntegration::new(
            text(IntegrationId::parse, "fixture-tools"),
            McpTransport::StreamableHttp {
                endpoint: LoopbackEndpoint::parse(endpoint).expect("fixture endpoint is loopback"),
            },
            [
                ConfiguredTool::new(tool("inspect"), ToolClass::Observe),
                ConfiguredTool::new(tool("mutate"), ToolClass::Mutate),
                ConfiguredTool::new(tool("mutation_status"), ToolClass::Observe),
            ],
            Some(tool("mutation_status")),
        )
        .expect("fixture HTTP integration is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test context fixture uses an idiomatic tail expression"
    )]
    fn context() -> AuthorizationContext {
        AuthorizationContext::new(
            text(WorkflowMode::parse, "review"),
            text(AgentRole::parse, "reviewer"),
            text(SessionId::parse, "session-1"),
            text(AssignmentId::parse, "assignment-1"),
            text(PolicyDecisionId::parse, "policy-1"),
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "the test policy fixture uses an idiomatic tail expression"
    )]
    fn policy(_context: &AuthorizationContext) -> PolicyIntersection {
        let integration = integration();
        policy_for(&integration)
    }

    #[expect(
        clippy::implicit_return,
        reason = "the integration-specific policy fixture keeps all six permission layers identical"
    )]
    fn policy_for(integration: &McpIntegration) -> PolicyIntersection {
        let permissions = PermissionGrant::new(
            [tool("inspect"), tool("mutate"), tool("mutation_status")],
            [
                ExternalToolCapability::DiscoverTools,
                ExternalToolCapability::InvokeTools,
                ExternalToolCapability::ReconcileMutations,
                ExternalToolCapability::ObserveToolListChanges,
                ExternalToolCapability::ObserveProgress,
                ExternalToolCapability::ObserveLogging,
                ExternalToolCapability::ObserveResourceChanges,
                ExternalToolCapability::ObservePromptChanges,
                ExternalToolCapability::DeclareRoots,
                ExternalToolCapability::ReadResources,
                ExternalToolCapability::ReadPrompts,
            ],
        );
        PolicyIntersection::new(
            integration,
            permissions.clone(),
            ScopedPermission::new(text(WorkflowMode::parse, "review"), permissions.clone()),
            ScopedPermission::new(text(AgentRole::parse, "reviewer"), permissions.clone()),
            ScopedPermission::new(text(SessionId::parse, "session-1"), permissions.clone()),
            ScopedPermission::new(
                text(AssignmentId::parse, "assignment-1"),
                permissions.clone(),
            ),
            ScopedPermission::new(text(PolicyDecisionId::parse, "policy-1"), permissions),
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "fixture authorization must fail loudly and uses an idiomatic tail expression"
    )]
    fn listing_authorization() -> AuthorizedToolListing {
        let integration = integration();
        listing_authorization_for(&integration)
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the integration-specific fixture authorization must fail loudly"
    )]
    fn listing_authorization_for(integration: &McpIntegration) -> AuthorizedToolListing {
        let context = context();
        authorize_tool_listing(integration, &policy_for(integration), &context)
            .expect("fixture permits tool discovery")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture authorization must fail loudly and uses an idiomatic tail expression"
    )]
    fn resource_listing_authorization() -> AuthorizedResourceListing {
        let integration = integration();
        let context = context();
        authorize_resource_listing(&integration, &policy(&context), &context)
            .expect("fixture permits resource discovery")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture authorization must fail loudly and uses an idiomatic tail expression"
    )]
    fn resource_read_authorization() -> AuthorizedResourceRead {
        let integration = integration();
        let context = context();
        authorize_resource_read(
            &integration,
            &policy(&context),
            &context,
            ResourceReadProposal::new(text(ResourceUri::parse, "file:///workspace/readme.md")),
        )
        .expect("fixture permits one resource read")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture authorization must fail loudly and uses an idiomatic tail expression"
    )]
    fn prompt_get_authorization() -> AuthorizedPromptGet {
        let integration = integration();
        let context = context();
        authorize_prompt_get(
            &integration,
            &policy(&context),
            &context,
            PromptGetProposal::new(
                text(PromptName::parse, "summarize"),
                Some(
                    PromptArguments::parse(r#"{"path":"src/lib.rs"}"#)
                        .expect("fixture prompt arguments are an object"),
                ),
            ),
        )
        .expect("fixture permits one prompt retrieval")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture authorization wrapper uses an idiomatic tail expression"
    )]
    fn observation_authorization(kind: ServerObservationKind) -> AuthorizedServerObservation {
        let integration = integration();
        observation_authorization_for(&integration, kind)
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the integration-specific fixture authorization must fail loudly"
    )]
    fn observation_authorization_for(
        integration: &McpIntegration,
        kind: ServerObservationKind,
    ) -> AuthorizedServerObservation {
        let context = context();
        authorize_server_observation(integration, &policy_for(integration), &context, kind)
            .expect("fixture permits the selected server observation")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture wrapper uses an idiomatic tail expression"
    )]
    fn call_authorization(name: &str, class: ToolClass) -> AuthorizedToolCall {
        let integration = integration();
        call_authorization_for(&integration, name, class)
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture authorization must fail loudly and uses concise closure tails"
    )]
    fn call_authorization_for(
        integration: &McpIntegration,
        name: &str,
        class: ToolClass,
    ) -> AuthorizedToolCall {
        let context = context();
        let idempotency_key =
            (class == ToolClass::Mutate).then(|| text(IdempotencyKey::parse, "mutation-1"));
        let owner_approval =
            (class == ToolClass::Mutate).then(|| text(OwnerApprovalId::parse, "approval-1"));
        authorize_tool_call(
            integration,
            &policy_for(integration),
            &context,
            ToolCallProposal::new(
                tool(name),
                ToolArguments::parse(r#"{"fixture":true}"#).expect("fixture args are JSON"),
                idempotency_key,
            ),
            owner_approval,
        )
        .expect("fixture policy authorizes call")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the fixture options constructor uses an idiomatic tail expression"
    )]
    fn options() -> RequestOptions {
        RequestOptions::new(Duration::from_secs(2), CancellationToken::new())
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the concrete failure-path fixture must fail loudly if invalid options are accepted"
    )]
    async fn assert_invalid_reconciliation_is_provenance_bound(
        client: &mut RmcpClient,
        reconciliation: &AuthorizedReconciliation,
    ) {
        let failure = client
            .reconcile(
                reconciliation,
                RequestOptions::new(Duration::ZERO, CancellationToken::new()),
            )
            .await
            .err()
            .expect("invalid timeout returns bound reconciliation failure");
        assert_eq!(failure.error().kind(), RmcpClientErrorKind::InvalidTimeout);
        assert_eq!(
            failure.provenance().idempotency_key(),
            reconciliation.idempotency_key()
        );
        assert_eq!(failure.provenance().originating_tool().as_str(), "mutate");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the deterministic protocol fixture must fail loudly on malformed input"
    )]
    async fn read_message<R>(reader: &mut BufReader<R>) -> Value
    where
        R: AsyncRead + Unpin,
    {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .expect("fixture reads a protocol line");
        assert_ne!(bytes, 0, "fixture receives a protocol line before EOF");
        serde_json::from_str(&line).expect("client emits JSON-RPC")
    }

    #[expect(
        clippy::expect_used,
        reason = "the deterministic protocol fixture must fail loudly on IO"
    )]
    async fn write_message<W>(writer: &mut W, message: Value)
    where
        W: AsyncWrite + Unpin,
    {
        let serialized_message = serde_json::to_vec(&message).expect("fixture serializes JSON-RPC");
        writer
            .write_all(&serialized_message)
            .await
            .expect("fixture writes JSON-RPC");
        writer
            .write_all(b"\n")
            .await
            .expect("fixture terminates JSON-RPC frame");
        writer.flush().await.expect("fixture flushes JSON-RPC");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the deterministic protocol fixture must fail loudly on malformed input"
    )]
    async fn maybe_read_message<R>(reader: &mut BufReader<R>) -> Option<Value>
    where
        R: AsyncRead + Unpin,
    {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .expect("fixture reads a protocol line or EOF");
        (bytes != 0).then(|| serde_json::from_str(&line).expect("client emits JSON-RPC"))
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the asynchronous fixture waits only for a bounded public observation delivery"
    )]
    async fn next_observation(
        client: &mut RmcpClient,
        authorization: &AuthorizedServerObservation,
    ) -> ServerObservation {
        timeout(Duration::from_secs(1), async {
            loop {
                if let Some(observation) = client
                    .try_next_observation(authorization)
                    .expect("matching fixture observation authorization succeeds")
                {
                    return observation;
                }
                task::yield_now().await;
            }
        })
        .await
        .expect("fixture receives its bounded observation")
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the initialization fixture directly asserts required JSON-RPC transcript fields"
    )]
    async fn respond_to_initialize<R, W>(reader: &mut BufReader<R>, writer: &mut W)
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let initialize = read_message(reader).await;
        assert_eq!(initialize["method"], "initialize");
        write_message(
            writer,
            response(
                &initialize,
                json!({
                    "protocolVersion":"2025-11-25",
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"fixture","version":"0.0.0"}
                }),
            ),
        )
        .await;
        let initialized = read_message(reader).await;
        assert_eq!(initialized["method"], "notifications/initialized");
    }

    #[expect(
        clippy::implicit_return,
        clippy::needless_pass_by_value,
        reason = "the JSON-RPC fixture keeps owned result values to avoid clone-heavy transcripts"
    )]
    fn response(request: &Value, result: Value) -> Value {
        json!({"jsonrpc":"2.0", "id": request["id"].clone(), "result": result})
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the raw HTTP fixture must fail loudly on an incomplete request"
    )]
    async fn read_http_request(stream: &mut TcpStream) -> HttpFixtureRequest {
        maybe_read_http_request(stream)
            .await
            .expect("fixture receives a complete HTTP request")
    }

    #[expect(
        clippy::arithmetic_side_effects,
        clippy::expect_used,
        clippy::implicit_return,
        clippy::indexing_slicing,
        reason = "the bounded raw HTTP fixture deliberately uses direct framing arithmetic and slices"
    )]
    async fn maybe_read_http_request(stream: &mut TcpStream) -> Option<HttpFixtureRequest> {
        let mut raw = Vec::new();
        let header_end = loop {
            let mut chunk: [u8; 4096] = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("fixture reads HTTP bytes");
            if read == 0 {
                return None;
            }
            raw.extend_from_slice(&chunk[..read]);
            assert!(
                raw.len() <= MAX_STDIO_MESSAGE_BYTES,
                "fixture HTTP request stays bounded"
            );
            if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let head = str::from_utf8(&raw[..header_end]).expect("fixture HTTP header is UTF-8");
        let mut lines = head.split("\r\n");
        let mut request_line = lines
            .next()
            .expect("fixture HTTP request has a request line")
            .split_whitespace();
        let method = request_line
            .next()
            .expect("fixture HTTP request has a method")
            .to_owned();
        let path = request_line
            .next()
            .expect("fixture HTTP request has a path")
            .to_owned();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect::<Vec<_>>();
        let content_length = headers
            .iter()
            .find(|header| header.0 == "content-length")
            .map_or(0, |header| {
                header
                    .1
                    .parse::<usize>()
                    .expect("fixture content length is valid")
            });
        while raw.len().saturating_sub(header_end) < content_length {
            let mut chunk: [u8; 4096] = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("fixture reads the HTTP body");
            assert_ne!(read, 0, "fixture receives the complete HTTP body");
            raw.extend_from_slice(&chunk[..read]);
        }
        let body = if content_length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&raw[header_end..header_end + content_length])
                .expect("fixture HTTP body is JSON-RPC")
        };
        Some(HttpFixtureRequest {
            body,
            headers,
            method,
            path,
        })
    }

    #[expect(
        clippy::expect_used,
        reason = "the raw HTTP fixture must fail loudly on response IO"
    )]
    async fn write_http_response(
        stream: &mut TcpStream,
        status: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) {
        let mut head = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for &(name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream
            .write_all(head.as_bytes())
            .await
            .expect("fixture writes HTTP response headers");
        stream
            .write_all(body)
            .await
            .expect("fixture writes HTTP response body");
        stream
            .shutdown()
            .await
            .expect("fixture closes HTTP response");
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::needless_pass_by_value,
        reason = "the fixture retains owned JSON values to build concise response transcripts"
    )]
    fn json_http_body(message: Value) -> Vec<u8> {
        serde_json::to_vec(&message).expect("fixture serializes JSON HTTP response")
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the deterministic stdio fixture uses direct runtime entry points and fails loudly on fixture IO"
    )]
    async fn rejects_an_unterminated_stdio_frame_over_the_tiber_bound() {
        let (mut server_stdout, client_stdin) =
            tokio::io::duplex((MAX_STDIO_MESSAGE_BYTES + 1) * 2);
        let mut reader = BoundedStdioReader::new(client_stdin, MAX_STDIO_MESSAGE_BYTES);

        server_stdout
            .write_all(&vec![b'x'; MAX_STDIO_MESSAGE_BYTES + 1])
            .await
            .expect("fixture writes its hostile frame");

        assert_eq!(
            reader.next_message().await,
            Err(StdioReadError::LineTooLong)
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the deterministic stdio fixture uses direct runtime entry points and fails loudly on fixture IO"
    )]
    async fn discards_the_rest_of_an_oversized_frame_before_reading_the_next_one() {
        let (mut server_stdout, client_stdin) =
            tokio::io::duplex((MAX_STDIO_MESSAGE_BYTES + 32) * 2);
        let mut reader = BoundedStdioReader::new(client_stdin, MAX_STDIO_MESSAGE_BYTES);
        let mut hostile_then_valid = vec![b'x'; MAX_STDIO_MESSAGE_BYTES + 1];
        hostile_then_valid.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\"}\n");
        server_stdout
            .write_all(&hostile_then_valid)
            .await
            .expect("fixture writes its hostile frame and a following valid frame");

        assert_eq!(
            reader.next_message().await,
            Err(StdioReadError::LineTooLong)
        );
        assert_eq!(
            reader.next_message().await,
            Ok(Some(b"{\"jsonrpc\":\"2.0\"}".to_vec()))
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_lines,
        reason = "the end-to-end stdio transcript deliberately uses direct runtime calls, JSON field assertions, and a single readable scenario"
    )]
    async fn accepts_bounded_stdio_initialization_listing_and_an_observed_call() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{"listChanged":true}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "method":"notifications/tools/list_changed"}),
            )
            .await;
            let progress: f64 = 1.0;
            let total: f64 = 2.0;
            write_message(
                &mut server_writer,
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/progress",
                    "params":{"progressToken":"fixture-progress", "progress":progress, "total":total}
                }),
            )
            .await;
            write_message(
                &mut server_writer,
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/message",
                    "params":{"level":"info", "data":"fixture log"}
                }),
            )
            .await;
            let listing = read_message(&mut reader).await;
            assert_eq!(listing["method"], "tools/list");
            write_message(
                &mut server_writer,
                response(
                    &listing,
                    json!({
                        "tools":[
                            {
                                "name":"inspect",
                                "description":"server-provided description",
                                "inputSchema":{"type":"object"}
                            },
                            {"name":"unknown","inputSchema":{"type":"object"}}
                        ]
                    }),
                ),
            )
            .await;
            let call = read_message(&mut reader).await;
            assert_eq!(call["method"], "tools/call");
            assert_eq!(call["params"]["name"], "inspect");
            write_message(
                &mut server_writer,
                response(
                    &call,
                    json!({
                        "content":[{"type":"text","text":"untrusted observation"}],
                        "isError":false
                    }),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");

        let listing = listing_authorization();
        let tools = client
            .list_tools(&listing, options())
            .await
            .expect("authorized tool list succeeds");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name().as_str(), "inspect");
        assert_eq!(
            tools[0]
                .description()
                .expect("fixture supplies a description")
                .as_str(),
            "server-provided description"
        );

        assert_eq!(
            client
                .try_next_observation(&observation_authorization(
                    ServerObservationKind::ToolListChanged,
                ))
                .expect("matching observation authorization succeeds"),
            Some(ServerObservation::ToolListChanged)
        );
        let progress = client
            .try_next_observation(&observation_authorization(ServerObservationKind::Progress))
            .expect("matching observation authorization succeeds")
            .expect("fixture emits a bounded progress observation");
        assert!(matches!(progress, ServerObservation::Progress(_)));
        let logging = client
            .try_next_observation(&observation_authorization(ServerObservationKind::Logging))
            .expect("matching observation authorization succeeds")
            .expect("fixture emits a bounded logging observation");
        assert!(matches!(logging, ServerObservation::Logging(_)));

        let outcome = client
            .call(call_authorization("inspect", ToolClass::Observe), options())
            .await
            .expect("authorized observation succeeds");
        let payload = outcome
            .outcome()
            .observed_payload()
            .expect("read-only call returns an observation");
        assert!(payload.as_str().contains("untrusted observation"));
        client.close().await;
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the optional-capability fixture directly drives the stdio protocol transcript"
    )]
    async fn accepts_advertised_optional_resource_and_prompt_capabilities() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}, "resources":{}, "prompts":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );

        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("tools plus optional resource/prompt capabilities are admitted");
        client.close().await;
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the authorized-roots fixture directly drives and asserts the stdio protocol transcript"
    )]
    async fn authorized_root_declaration_advertises_and_returns_only_tiber_owned_roots() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            assert!(initialize["params"]["capabilities"]["roots"].is_object());
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "id":"roots-1", "method":"roots/list"}),
            )
            .await;
            let roots = read_message(&mut reader).await;
            assert_eq!(roots["id"], "roots-1");
            assert_eq!(
                roots["result"]["roots"],
                json!([{"uri":"file:///workspace"}])
            );
        });
        let root_integration = integration()
            .with_tiber_roots([
                TiberOwnedRoot::from_absolute_path("/workspace").expect("fixture root is absolute")
            ])
            .expect("fixture root catalog is valid");
        let root_context = context();
        let authorization = authorize_root_declaration(
            &root_integration,
            &policy_for(&root_integration),
            &root_context,
        )
        .expect("fixture permits root declaration");
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize_with_roots(
            authorization.integration().clone(),
            Some(authorization),
            transport,
            None,
            health,
            cancellation,
            options(),
            RmcpClientOperation::DeclareRoots,
        )
        .await
        .expect("root-authorized fixture server initializes");
        server
            .await
            .expect("fixture receives the authorized roots response");
        client.close().await;
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the ordinary-connection fixture directly drives and asserts the roots refusal transcript"
    )]
    async fn ordinary_connections_neither_advertise_nor_disclose_roots() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            assert!(initialize["params"]["capabilities"]["roots"].is_null());
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "id":"roots-1", "method":"roots/list"}),
            )
            .await;
            let roots_refusal = read_message(&mut reader).await;
            assert_eq!(roots_refusal["id"], "roots-1");
            let method_not_found: i32 = -32_601;
            assert_eq!(roots_refusal["error"]["code"], method_not_found);
            assert!(roots_refusal["result"].is_null());
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let ordinary_integration = integration()
            .with_tiber_roots([
                TiberOwnedRoot::from_absolute_path("/workspace").expect("fixture root is absolute")
            ])
            .expect("fixture root catalog is valid");
        let mut client = RmcpClient::initialize(
            ordinary_integration,
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("ordinary fixture server initializes before its roots request");
        server
            .await
            .expect("fixture receives the ordinary roots refusal");
        client.close().await;
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_lines,
        reason = "the optional read fixture keeps one complete bounded stdio transcript readable in a single scenario"
    )]
    async fn authorized_resource_and_prompt_reads_are_bounded_and_exact() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}, "resources":{}, "prompts":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");

            let resource_listing = read_message(&mut reader).await;
            assert_eq!(resource_listing["method"], "resources/list");
            write_message(
                &mut server_writer,
                response(
                    &resource_listing,
                    json!({
                        "resources":[{
                            "uri":"file:///workspace/readme.md",
                            "name":"readme",
                            "description":"untrusted resource metadata"
                        }]
                    }),
                ),
            )
            .await;

            let resource_read = read_message(&mut reader).await;
            assert_eq!(resource_read["method"], "resources/read");
            assert_eq!(
                resource_read["params"]["uri"],
                "file:///workspace/readme.md"
            );
            assert!(resource_read["params"]["inputResponses"].is_null());
            assert!(resource_read["params"]["requestState"].is_null());
            write_message(
                &mut server_writer,
                response(
                    &resource_read,
                    json!({
                        "contents":[{
                            "uri":"file:///workspace/readme.md",
                            "text":"untrusted resource contents"
                        }]
                    }),
                ),
            )
            .await;

            let prompt_listing = read_message(&mut reader).await;
            assert_eq!(prompt_listing["method"], "prompts/list");
            write_message(
                &mut server_writer,
                response(
                    &prompt_listing,
                    json!({"prompts":[{"name":"summarize", "description":"untrusted prompt metadata"}]}),
                ),
            )
            .await;

            let prompt_get = read_message(&mut reader).await;
            assert_eq!(prompt_get["method"], "prompts/get");
            assert_eq!(prompt_get["params"]["name"], "summarize");
            assert_eq!(
                prompt_get["params"]["arguments"],
                json!({"path":"src/lib.rs"})
            );
            assert!(prompt_get["params"]["inputResponses"].is_null());
            assert!(prompt_get["params"]["requestState"].is_null());
            write_message(
                &mut server_writer,
                response(
                    &prompt_get,
                    json!({
                        "description":"untrusted prompt result",
                        "messages":[{
                            "role":"user",
                            "content":{"type":"text", "text":"untrusted prompt message"}
                        }]
                    }),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("optional-resource/prompt fixture server initializes");

        let resources = client
            .list_resources(&resource_listing_authorization(), options())
            .await
            .expect("authorized resource list succeeds");
        assert_eq!(resources.len(), 1);
        assert!(
            resources[0]
                .as_str()
                .contains("untrusted resource metadata")
        );
        let resource = client
            .read_resource(&resource_read_authorization(), options())
            .await
            .expect("authorized resource read succeeds");
        assert!(resource.as_str().contains("untrusted resource contents"));
        let prompt_listing_integration = integration();
        let prompt_listing_context = context();
        let prompt_listing = authorize_prompt_listing(
            &prompt_listing_integration,
            &policy(&prompt_listing_context),
            &prompt_listing_context,
        )
        .expect("fixture permits prompt discovery");
        let prompts = client
            .list_prompts(&prompt_listing, options())
            .await
            .expect("authorized prompt list succeeds");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].as_str().contains("untrusted prompt metadata"));
        let prompt = client
            .get_prompt(&prompt_get_authorization(), options())
            .await
            .expect("authorized prompt retrieval succeeds");
        assert!(prompt.as_str().contains("untrusted prompt message"));
        client.close().await;
        server
            .await
            .expect("fixture completes all optional read requests");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the absent-capability fixture directly drives and asserts the no-wire stdio transcript"
    )]
    async fn optional_read_methods_refuse_missing_capabilities_without_a_wire_request() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            let follow_up = maybe_read_message(&mut reader).await;
            assert!(
                follow_up.is_none(),
                "missing optional capability sends no request"
            );
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("tools-only fixture server initializes");

        assert_eq!(
            client
                .list_resources(&resource_listing_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        assert_eq!(
            client
                .get_prompt(&prompt_get_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        client.close().await;
        server
            .await
            .expect("fixture observes no optional wire request");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "the optional-notification fixture directly drives and asserts the stdio protocol transcript"
    )]
    async fn resource_and_prompt_notifications_require_matching_observation_tokens() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{
                            "tools":{},
                            "resources":{"listChanged":true},
                            "prompts":{"listChanged":true}
                        },
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "method":"notifications/resources/list_changed"}),
            )
            .await;
            write_message(
                &mut server_writer,
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/resources/updated",
                    "params":{"uri":"file:///workspace/readme.md"}
                }),
            )
            .await;
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "method":"notifications/prompts/list_changed"}),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("optional notification fixture initializes");

        assert_eq!(
            next_observation(
                &mut client,
                &observation_authorization(ServerObservationKind::ResourceListChanged),
            )
            .await,
            ServerObservation::ResourceListChanged
        );
        let resource_updated = next_observation(
            &mut client,
            &observation_authorization(ServerObservationKind::ResourceUpdated),
        )
        .await;
        let ServerObservation::ResourceUpdated(payload) = resource_updated else {
            panic!("resource token only delivers a bounded resource update");
        };
        assert!(payload.as_str().contains("file:///workspace/readme.md"));
        assert_eq!(
            next_observation(
                &mut client,
                &observation_authorization(ServerObservationKind::PromptListChanged),
            )
            .await,
            ServerObservation::PromptListChanged
        );
        client.close().await;
        server
            .await
            .expect("fixture emits only the token-gated notifications");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the notification transcript proves one saturated observation kind cannot starve another"
    )]
    async fn saturated_logging_ingress_does_not_starve_progress() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            for sequence in 0..super::MAX_PENDING_OBSERVATIONS_PER_KIND {
                write_message(
                    &mut server_writer,
                    json!({
                        "jsonrpc":"2.0",
                        "method":"notifications/message",
                        "params":{"level":"info", "data":{"sequence":sequence}}
                    }),
                )
                .await;
            }
            let completed = u64::from(true);
            write_message(
                &mut server_writer,
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/progress",
                    "params":{"progressToken":"fixture", "progress":completed, "total":completed}
                }),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");

        let progress = next_observation(
            &mut client,
            &observation_authorization(ServerObservationKind::Progress),
        )
        .await;
        assert!(matches!(progress, ServerObservation::Progress(_)));
        client.close().await;
        server
            .await
            .expect("fixture emits the bounded notification burst");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_lines,
        clippy::unreachable,
        reason = "the raw HTTP fixture asserts the resource/prompt request transcript through the public connection constructors"
    )]
    async fn loopback_http_optional_resource_and_prompt_reads_are_admitted() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in [
                "initialize",
                "notifications/initialized",
                "resources/list",
                "initialize",
                "notifications/initialized",
                "prompts/get",
            ] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.path, "/mcp");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}, "resources":{}, "prompts":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    "resources/list" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({"resources":[{
                                "uri":"file:///workspace/readme.md",
                                "name":"readme"
                            }]}),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "prompts/get" => {
                        assert_eq!(request.body["params"]["name"], "summarize");
                        assert_eq!(
                            request.body["params"]["arguments"],
                            json!({"path":"src/lib.rs"})
                        );
                        let body = json_http_body(response(
                            &request.body,
                            json!({"messages":[{
                                "role":"user",
                                "content":{"type":"text", "text":"loopback prompt result"}
                            }]}),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    _ => unreachable!("fixture method list is closed"),
                }
            }
        });
        let integration = http_integration(&endpoint);
        let context = context();
        let integration_policy = policy_for(&integration);
        let resource_listing =
            authorize_resource_listing(&integration, &integration_policy, &context)
                .expect("fixture permits loopback resource discovery");
        let prompt_get = authorize_prompt_get(
            &integration,
            &integration_policy,
            &context,
            PromptGetProposal::new(
                text(PromptName::parse, "summarize"),
                Some(
                    PromptArguments::parse(r#"{"path":"src/lib.rs"}"#)
                        .expect("fixture prompt arguments are an object"),
                ),
            ),
        )
        .expect("fixture permits loopback prompt retrieval");

        let mut resource_client =
            RmcpClient::connect_for_resource_listing(&resource_listing, options())
                .await
                .expect("loopback resource client initializes");
        let resources = resource_client
            .list_resources(&resource_listing, options())
            .await
            .expect("loopback resource list succeeds");
        assert_eq!(resources.len(), 1);
        resource_client.close().await;

        let mut prompt_client = RmcpClient::connect_for_prompt_get(&prompt_get, options())
            .await
            .expect("loopback prompt client initializes");
        let prompt = prompt_client
            .get_prompt(&prompt_get, options())
            .await
            .expect("loopback prompt retrieval succeeds");
        assert!(prompt.as_str().contains("loopback prompt result"));
        prompt_client.close().await;
        server
            .await
            .expect("loopback fixture completes optional read operations");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw HTTP fixture proves the public roots constructor is the only path that advertises roots"
    )]
    async fn loopback_http_root_constructor_advertises_the_roots_capability() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.path, "/mcp");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        assert!(request.body["params"]["capabilities"]["roots"].is_object());
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    _ => unreachable!("fixture method list is closed"),
                }
            }
        });
        let integration = http_integration(&endpoint)
            .with_tiber_roots([
                TiberOwnedRoot::from_absolute_path("/workspace").expect("fixture root is absolute")
            ])
            .expect("fixture root catalog is valid");
        let context = context();
        let authorization =
            authorize_root_declaration(&integration, &policy_for(&integration), &context)
                .expect("fixture permits loopback root declaration");
        let mut client = RmcpClient::connect_for_root_declaration(&authorization, options())
            .await
            .expect("loopback root client initializes");
        client.close().await;
        server
            .await
            .expect("loopback fixture observes the roots capability advertisement");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the input-required fixture directly drives and asserts the terminal stdio transcript"
    )]
    async fn input_required_prompt_result_is_terminal_and_never_sends_a_continuation() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}, "prompts":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            let prompt_get = read_message(&mut reader).await;
            assert_eq!(prompt_get["method"], "prompts/get");
            write_message(
                &mut server_writer,
                response(
                    &prompt_get,
                    json!({
                        "resultType":"input_required",
                        "messages":[],
                        "requestState":"opaque-state"
                    }),
                ),
            )
            .await;
            assert!(
                maybe_read_message(&mut reader).await.is_none(),
                "the adapter closes instead of sending a continuation request"
            );
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("prompt fixture server initializes");

        assert_eq!(
            client
                .get_prompt(&prompt_get_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        server
            .await
            .expect("fixture observes no interactive continuation");
        client.close().await;
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the caching fixture directly drives and asserts the terminal resource-read transcript"
    )]
    async fn resource_cache_directive_is_explicitly_refused_and_not_retained() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let cache_ttl_milliseconds: u64 = 1_000;
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}, "resources":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            let resource_read = read_message(&mut reader).await;
            assert_eq!(resource_read["method"], "resources/read");
            write_message(
                &mut server_writer,
                response(
                    &resource_read,
                    json!({
                        "ttlMs":cache_ttl_milliseconds,
                        "cacheScope":"private",
                        "contents":[{
                            "uri":"file:///workspace/readme.md",
                            "text":"must not be cached"
                        }]
                    }),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("resource fixture server initializes");

        assert_eq!(
            client
                .read_resource(&resource_read_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        client.close().await;
        server
            .await
            .expect("fixture receives the one terminal resource request");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the hostile-metadata fixture directly drives and asserts the bounded resource-list transcript"
    )]
    async fn oversized_resource_metadata_is_rejected_before_callers_can_retain_it() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}, "resources":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            let resource_listing = read_message(&mut reader).await;
            assert_eq!(resource_listing["method"], "resources/list");
            write_message(
                &mut server_writer,
                response(
                    &resource_listing,
                    json!({"resources":[{
                        "uri":"file:///workspace/readme.md",
                        "name":"readme",
                        "description":"x".repeat(MAX_UNTRUSTED_PAYLOAD_BYTES + 1)
                    }]}),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("resource fixture server initializes");

        assert_eq!(
            client
                .list_resources(&resource_listing_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UntrustedPayloadTooLarge)
        );
        server
            .await
            .expect("fixture sends the oversized resource description once");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the hostile-metadata stdio fixture directly drives and asserts the protocol transcript"
    )]
    async fn rejects_oversized_hostile_tool_metadata_without_retaining_it() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            let listing = read_message(&mut reader).await;
            assert_eq!(listing["method"], "tools/list");
            write_message(
                &mut server_writer,
                response(
                    &listing,
                    json!({
                        "tools":[{
                            "name":"inspect",
                            "description":"x".repeat(MAX_UNTRUSTED_PAYLOAD_BYTES + 1),
                            "inputSchema":{"type":"object"}
                        }]
                    }),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");

        assert_eq!(
            client
                .list_tools(&listing_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UntrustedPayloadTooLarge)
        );
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "the cancellation fixture directly drives and asserts its stdio protocol transcript"
    )]
    async fn pre_dispatch_mutation_cancellation_is_typed_and_never_calls_the_server() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let call_count = Arc::new(AtomicUsize::new(0));
        let fixture_call_count = Arc::clone(&call_count);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            while let Some(message) = maybe_read_message(&mut reader).await {
                if message["method"] == "tools/call" {
                    fixture_call_count.fetch_add(1, Ordering::AcqRel);
                }
            }
        });
        let health = ConnectionHealth::new();
        let connection_cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            connection_cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            connection_cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = match client
            .call(
                call_authorization("mutate", ToolClass::Mutate),
                RequestOptions::new(Duration::from_secs(1), cancellation),
            )
            .await
        {
            Ok(_outcome) => panic!("pre-dispatch cancellation must fail"),
            Err(error) => error,
        };
        assert_eq!(error.error().kind(), RmcpClientErrorKind::Cancelled);
        assert_eq!(error.provenance().tool().as_str(), "mutate");
        client.close().await;
        server
            .await
            .expect("fixture server observes the closed client");
        assert_eq!(call_count.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the cancellation fixture directly drives and asserts its stdio protocol transcript"
    )]
    async fn dispatched_mutation_cancellation_returns_outcome_unknown_without_a_retry() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let call_started = Arc::new(Notify::new());
        let fixture_call_started = Arc::clone(&call_started);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            let call = read_message(&mut reader).await;
            assert_eq!(call["method"], "tools/call");
            assert_eq!(call["params"]["name"], "mutate");
            fixture_call_started.notify_one();
            // The terminal adapter close may race the best-effort protocol
            // cancellation hint. Either a cancellation notification or EOF is
            // correct; neither may cause a retry of this mutation.
            if let Some(cancelled) = maybe_read_message(&mut reader).await {
                assert_eq!(cancelled["method"], "notifications/cancelled");
                assert_eq!(cancelled["params"]["requestId"], call["id"]);
            }
        });
        let health = ConnectionHealth::new();
        let connection_cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            connection_cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            connection_cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");
        let cancellation = CancellationToken::new();
        let cancel_after_dispatch = cancellation.clone();
        let cancel_task = tokio::spawn(async move {
            call_started.notified().await;
            cancel_after_dispatch.cancel();
        });

        let outcome = client
            .call(
                call_authorization("mutate", ToolClass::Mutate),
                RequestOptions::new(Duration::from_secs(1), cancellation),
            )
            .await
            .expect("a dispatched mutation becomes explicitly ambiguous on cancellation");
        assert!(matches!(
            outcome.outcome(),
            ToolCallOutcome::OutcomeUnknown(_)
        ));
        cancel_task.await.expect("fixture canceller completes");
        server
            .await
            .expect("fixture server observes cancellation once");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_lines,
        reason = "the transport-loss fixture directly drives a multi-connection stdio transcript and requires a typed reconciliation token"
    )]
    async fn transport_loss_returns_one_reconciliation_token_with_canonical_status_arguments() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let invocation_count = Arc::new(AtomicUsize::new(0));
        let fixture_invocation_count = Arc::clone(&invocation_count);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            let call = read_message(&mut reader).await;
            assert_eq!(call["method"], "tools/call");
            assert_eq!(call["params"]["name"], "mutate");
            assert_eq!(call["params"]["arguments"]["idempotencyKey"], "mutation-1");
            fixture_invocation_count.fetch_add(1, Ordering::AcqRel);
            // Drop the connection after receipt, leaving the side effect
            // explicitly ambiguous rather than issuing any retry.
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");

        let outcome = client
            .call(call_authorization("mutate", ToolClass::Mutate), options())
            .await
            .expect("transport loss makes the mutation outcome explicit");
        let reconciliation = outcome
            .into_reconciliation()
            .expect("mutating transport loss returns a reconciliation token");
        assert_eq!(invocation_count.load(Ordering::Acquire), 1);
        server
            .await
            .expect("fixture server drops the first connection");

        let (status_client_io, status_server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (status_client_reader, status_client_writer) = tokio::io::split(status_client_io);
        let (status_server_reader, mut status_server_writer) = tokio::io::split(status_server_io);
        let status_server = tokio::spawn(async move {
            let mut reader = BufReader::new(status_server_reader);
            respond_to_initialize(&mut reader, &mut status_server_writer).await;
            let status = read_message(&mut reader).await;
            assert_eq!(status["method"], "tools/call");
            assert_eq!(status["params"]["name"], "mutation_status");
            assert_eq!(
                status["params"]["arguments"],
                json!({"idempotencyKey":"mutation-1"})
            );
            write_message(
                &mut status_server_writer,
                response(
                    &status,
                    json!({
                        "content":[],
                        "structuredContent":{"status":"committed"},
                        "isError":false
                    }),
                ),
            )
            .await;
        });
        let status_health = ConnectionHealth::new();
        let status_cancellation = CancellationToken::new();
        let status_transport = bounded_stdio_transport(
            status_client_reader,
            status_client_writer,
            status_health.clone(),
            status_cancellation.clone(),
        );
        let mut status_client = RmcpClient::initialize(
            reconciliation.integration().clone(),
            status_transport,
            None,
            status_health,
            status_cancellation,
            options(),
        )
        .await
        .expect("status fixture initializes");

        let reconciled = status_client
            .reconcile(&reconciliation, options())
            .await
            .expect("status fixture returns a bound reconciliation result");
        assert_eq!(reconciled.outcome(), ReconciliationOutcome::Committed);
        assert_eq!(
            reconciled.idempotency_key().as_str(),
            reconciliation.idempotency_key().as_str()
        );
        assert_eq!(reconciled.originating_tool().as_str(), "mutate");
        assert_eq!(
            reconciled
                .approval()
                .map(tiber_external_tools_core::OwnerApprovalId::as_str),
            Some("approval-1")
        );
        assert_invalid_reconciliation_is_provenance_bound(&mut status_client, &reconciliation)
            .await;
        status_client.close().await;
        status_server
            .await
            .expect("status fixture completes exactly one reconciliation call");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the reconciliation fixture uses exact MCP result construction and fails loudly on malformed test data"
    )]
    async fn errored_or_nonexact_reconciliation_status_never_proves_a_mutation_committed() {
        let errored: rmcp::model::CallToolResult = serde_json::from_value(json!({
            "content":[],
            "structuredContent":{"status":"committed"},
            "isError":true
        }))
        .expect("fixture result is valid MCP JSON");
        let extra_fields: rmcp::model::CallToolResult = serde_json::from_value(json!({
            "content":[],
            "structuredContent":{"status":"committed", "untrusted":"extra"},
            "isError":false
        }))
        .expect("fixture result is valid MCP JSON");

        assert_eq!(
            super::StrictReconciliationStatus::from(&errored),
            super::StrictReconciliationStatus::StillUnknown
        );
        assert_eq!(
            super::StrictReconciliationStatus::from(&extra_fields),
            super::StrictReconciliationStatus::StillUnknown
        );
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the child-process fixture invokes the concrete async runtime command and fails loudly on process control errors"
    )]
    async fn terminate_child_kills_and_reaps_a_direct_process() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("fixture starts a direct child process");
        super::terminate_child(&mut child).await;
        assert!(
            child
                .try_wait()
                .expect("fixture checks the reaped child")
                .is_some()
        );
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::unreachable,
        reason = "the generated real-child fixture proves the public ordinary stdio boundary clears ambient credential roots and cwd"
    )]
    async fn ordinary_stdio_connection_does_not_inherit_parent_home_or_cwd() {
        let parent_home = env::var("HOME").expect("fixture parent has a HOME sentinel");
        let parent_cwd = env::current_dir()
            .expect("fixture resolves its parent cwd")
            .display()
            .to_string();
        assert_ne!(parent_home, "/");
        assert_ne!(parent_cwd, "/");
        let probe_path =
            env::temp_dir().join(format!("tiber-rmcp-ambient-probe-{}.sh", process::id()));
        fs::write(
            &probe_path,
            r#"#!/bin/sh
IFS= read -r initialize
printf '%s\n' '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"probe","version":"0"}}}'
IFS= read -r initialized
printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"ambient-home=%s;cwd=%s"}}\n' "${HOME-unset}" "$PWD"
IFS= read -r hold
"#,
        )
        .expect("fixture writes its executable probe");
        let mut permissions = fs::metadata(&probe_path)
            .expect("fixture reads probe metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&probe_path, permissions).expect("fixture marks probe executable");
        let integration = McpIntegration::new(
            text(IntegrationId::parse, "fixture-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse(
                    probe_path
                        .to_str()
                        .expect("fixture probe path is valid UTF-8"),
                )
                .expect("fixture probe path is absolute"),
                arguments: Vec::new(),
            },
            [ConfiguredTool::new(tool("inspect"), ToolClass::Observe)],
            None,
        )
        .expect("fixture probe integration is valid");
        let authorization =
            observation_authorization_for(&integration, ServerObservationKind::Logging);
        let mut client = RmcpClient::connect_for_observation(&authorization, options())
            .await
            .expect("ordinary stdio probe initializes");

        let observation = next_observation(&mut client, &authorization).await;
        let ServerObservation::Logging(payload) = observation else {
            unreachable!("logging authorization returns a logging observation");
        };
        assert!(payload.as_str().contains("ambient-home=unset;cwd=/"));
        assert!(!payload.as_str().contains(&parent_home));
        assert!(!payload.as_str().contains(&parent_cwd));

        client.close().await;
        fs::remove_file(&probe_path).expect("fixture removes its generated probe");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "the public process-setup failure fixture asserts sanitized owner-facing recovery information"
    )]
    async fn process_setup_error_retains_safe_cause_and_actionable_context() {
        let missing_program = "/definitely-missing/tiber-rmcp-fixture";
        let integration = McpIntegration::new(
            text(IntegrationId::parse, "fixture-tools"),
            McpTransport::Stdio {
                program: AbsoluteProgram::parse(missing_program)
                    .expect("fixture missing program is still an absolute trusted path"),
                arguments: Vec::new(),
            },
            [ConfiguredTool::new(tool("inspect"), ToolClass::Observe)],
            None,
        )
        .expect("fixture missing-program integration is valid");
        let authorization = listing_authorization_for(&integration);

        let error = match RmcpClient::connect_for_listing(&authorization, options()).await {
            Ok(mut client) => {
                client.close().await;
                panic!("missing fixture program cannot initialize")
            }
            Err(error) => error,
        };

        assert_eq!(error.code(), "rmcp_client_process_unavailable");
        assert_eq!(error.kind(), RmcpClientErrorKind::ProcessUnavailable);
        assert_eq!(
            error.context().operation(),
            RmcpClientOperation::StdioProcessSetup
        );
        assert_eq!(
            error.context().action(),
            "check the configured executable and host process availability"
        );
        assert_eq!(error.retryability(), RmcpClientRetryability::Permanent);
        assert_eq!(
            error.retained_cause(),
            Some(RmcpClientCause::ProcessSpawn(ErrorKind::NotFound))
        );
        assert!(error.source().is_some());
        assert_eq!(error.to_string(), "rmcp_client_process_unavailable");
        assert!(!error.to_string().contains(missing_program));
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        reason = "the hostile-notification fixture directly drives the stdio runtime transcript"
    )]
    async fn custom_notification_is_explicitly_refused_after_initialization() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            write_message(
                &mut server_writer,
                json!({"jsonrpc":"2.0", "method":"notifications/custom-server"}),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let observed_health = health.clone();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes before its hostile notification");

        timeout(Duration::from_secs(1), async {
            loop {
                if observed_health
                    .client_error(RmcpClientOperation::ListTools)
                    .is_some_and(is_unsupported_error)
                {
                    break;
                }
                task::yield_now().await;
            }
        })
        .await
        .expect("custom notification reaches the explicit refusal handler");
        assert_eq!(
            client
                .list_tools(&listing_authorization(), options())
                .await
                .map_err(error_kind),
            Err(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the oversized-logging fixture directly drives and asserts the stdio protocol transcript"
    )]
    async fn oversized_logging_during_initialization_returns_the_typed_refusal() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            write_message(
                &mut server_writer,
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/message",
                    "params":{
                        "level":"info",
                        "data":"x".repeat(MAX_UNTRUSTED_PAYLOAD_BYTES + 1)
                    }
                }),
            )
            .await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"tools":{}},
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );

        assert_eq!(
            RmcpClient::initialize(
                integration(),
                transport,
                None,
                health,
                cancellation,
                options(),
            )
            .await
            .err()
            .map(error_kind),
            Some(RmcpClientErrorKind::UnsupportedServerCapability),
        );
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the tasks-refusal fixture directly drives and asserts the stdio protocol transcript"
    )]
    async fn server_tasks_capability_is_refused_before_any_tool_call() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            write_message(
                &mut server_writer,
                response(
                    &initialize,
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{
                            "tools":{},
                            "extensions":{"io.modelcontextprotocol/tasks":{}}
                        },
                        "serverInfo":{"name":"fixture","version":"0.0.0"}
                    }),
                ),
            )
            .await;
            let initialized = read_message(&mut reader).await;
            assert_eq!(initialized["method"], "notifications/initialized");
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );

        assert_eq!(
            RmcpClient::initialize(
                integration(),
                transport,
                None,
                health,
                cancellation,
                options(),
            )
            .await
            .err()
            .map(error_kind),
            Some(RmcpClientErrorKind::TasksUnsupported)
        );
        server.await.expect("fixture server completes");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw HTTP fixture asserts that an unsupported negotiated protocol cannot issue a tool-list request"
    )]
    async fn loopback_http_standard_headers_protocol_is_refused_before_tool_listing() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.path, "/mcp");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2026-07-28",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    _ => unreachable!("fixture method list is closed"),
                }
            }
            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "unsupported protocol must not issue tools/list"
            );
        });
        let integration = http_integration(&endpoint);
        let context = context();
        let authorization =
            authorize_tool_listing(&integration, &policy_for(&integration), &context)
                .expect("fixture permits loopback tool discovery");

        assert_eq!(
            RmcpClient::connect_for_listing(&authorization, options())
                .await
                .err()
                .map(error_kind),
            Some(RmcpClientErrorKind::UnsupportedServerCapability)
        );
        server
            .await
            .expect("unsupported protocol fixture observes no tool-list request");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw HTTP fixture asserts an exact request transcript and treats unexpected requests as test failures"
    )]
    async fn loopback_http_json_initializes_and_returns_a_bounded_observation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized", "tools/call"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.path, "/mcp");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    "tools/call" => {
                        assert_eq!(request.body["params"]["name"], "inspect");
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "content":[{"type":"text","text":"loopback JSON observation"}],
                                "isError":false
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    _ => unreachable!("fixture method list is closed"),
                }
            }
        });
        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "inspect", ToolClass::Observe);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes");

        let outcome = client
            .call(authorization, options())
            .await
            .expect("loopback HTTP call succeeds");
        let payload = outcome
            .outcome()
            .observed_payload()
            .expect("read-only HTTP call returns an observation");
        assert!(payload.as_str().contains("loopback JSON observation"));
        client.close().await;
        server.await.expect("loopback HTTP fixture completes");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw SSE fixture asserts an exact request transcript and treats unexpected requests as test failures"
    )]
    async fn loopback_http_sse_result_is_bounded_and_observed_without_a_retry() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized", "tools/call"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    "tools/call" => {
                        let response = response(
                            &request.body,
                            json!({
                                "content":[{"type":"text","text":"loopback SSE observation"}],
                                "isError":false
                            }),
                        );
                        let body = format!(
                            "data: {}\n\n",
                            serde_json::to_string(&response)
                                .expect("fixture serializes the SSE response")
                        );
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "text/event-stream")],
                            body.as_bytes(),
                        )
                        .await;
                    }
                    _ => unreachable!("fixture method list is closed"),
                }
            }
        });
        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "inspect", ToolClass::Observe);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes");

        let outcome = client
            .call(authorization, options())
            .await
            .expect("bounded SSE result succeeds");
        let payload = outcome
            .outcome()
            .observed_payload()
            .expect("SSE read-only call returns an observation");
        assert!(payload.as_str().contains("loopback SSE observation"));
        client.close().await;
        server.await.expect("loopback SSE fixture completes");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw HTTP drop fixture uses a closed request transcript to prove that no mutation replay occurs"
    )]
    async fn loopback_http_transport_drop_never_replays_a_mutation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    _ => unreachable!("fixture setup methods are closed"),
                }
            }

            let (mut stream, _peer) = listener.accept().await.expect("fixture accepts mutation");
            let request = read_http_request(&mut stream).await;
            assert_eq!(request.method, "POST");
            assert_eq!(request.body["method"], "tools/call");
            assert_eq!(request.body["params"]["name"], "mutate");
            assert_eq!(
                request.body["params"]["arguments"]["idempotencyKey"],
                "mutation-1"
            );
            drop(stream);

            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "the terminal transport drop must not produce a replay connection"
            );
        });

        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes");

        let outcome = client
            .call(authorization, options())
            .await
            .expect("a dispatched mutation becomes explicitly ambiguous");
        assert!(matches!(
            outcome.outcome(),
            ToolCallOutcome::OutcomeUnknown(_)
        ));
        server
            .await
            .expect("fixture observes no retry after the transport drop");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::match_wild_err_arm,
        clippy::panic,
        clippy::unreachable,
        reason = "the stalled raw HTTP fixture treats every unexpected protocol result as a deterministic test failure"
    )]
    async fn stalled_loopback_http_mutation_is_aborted_at_its_operation_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    _ => unreachable!("fixture setup methods are closed"),
                }
            }

            let (mut stream, _peer) = listener.accept().await.expect("fixture accepts mutation");
            let request = read_http_request(&mut stream).await;
            assert_eq!(request.method, "POST");
            assert_eq!(request.body["method"], "tools/call");
            assert_eq!(request.body["params"]["name"], "mutate");
            let mut byte: [u8; 1] = [0; 1];
            match timeout(Duration::from_millis(500), stream.read(&mut byte)).await {
                Ok(Ok(0)) => {}
                Ok(Ok(_)) => panic!("a stalled mutation must not send another request"),
                Ok(Err(error)) => panic!("fixture failed while checking client close: {error}"),
                Err(_) => panic!("client did not abort the stalled HTTP request"),
            }
            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "a cancelled mutation must not open a follow-up connection"
            );
        });

        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes");

        let outcome = timeout(
            Duration::from_millis(400),
            client.call(
                authorization,
                RequestOptions::new(Duration::from_millis(100), CancellationToken::new()),
            ),
        )
        .await
        .expect("operation deadline interrupts a stalled HTTP post")
        .expect("a dispatched mutation becomes explicitly ambiguous");
        assert!(matches!(
            outcome.outcome(),
            ToolCallOutcome::OutcomeUnknown(_)
        ));
        server
            .await
            .expect("fixture observes the aborted HTTP request");
    }

    #[tokio::test]
    #[expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable,
        reason = "the raw SSE retry fixture uses a closed request transcript to prove resume is impossible"
    )]
    async fn loopback_http_sse_retry_directive_cannot_resume_a_mutation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let server = tokio::spawn(async move {
            for expected_method in ["initialize", "notifications/initialized"] {
                let (mut stream, _peer) = listener.accept().await.expect("fixture accepts HTTP");
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.method, "POST");
                assert_eq!(request.body["method"], expected_method);
                match expected_method {
                    "initialize" => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "application/json")],
                            &body,
                        )
                        .await;
                    }
                    "notifications/initialized" => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    _ => unreachable!("fixture setup methods are closed"),
                }
            }

            let (mut stream, _peer) = listener.accept().await.expect("fixture accepts mutation");
            let request = read_http_request(&mut stream).await;
            assert_eq!(request.method, "POST");
            assert_eq!(request.body["method"], "tools/call");
            assert_eq!(request.body["params"]["name"], "mutate");
            write_http_response(
                &mut stream,
                "200 OK",
                &[("Content-Type", "text/event-stream")],
                b"id: event-1\nretry: 0\n\n",
            )
            .await;

            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "a server SSE retry directive must not trigger a resumed GET"
            );
        });

        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes");

        let outcome = client
            .call(authorization, options())
            .await
            .expect("an incomplete mutation stream becomes explicitly ambiguous");
        assert!(matches!(
            outcome.outcome(),
            ToolCallOutcome::OutcomeUnknown(_)
        ));
        server
            .await
            .expect("fixture observes no SSE resume request");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::too_many_lines,
        reason = "the raw HTTP session fixture keeps one complete retry-prevention transcript readable in a single scenario"
    )]
    async fn loopback_http_session_404_never_reinitializes_or_replays_a_mutation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let initialize_count = Arc::new(AtomicUsize::new(0));
        let mutation_count = Arc::new(AtomicUsize::new(0));
        let fixture_initialize_count = Arc::clone(&initialize_count);
        let fixture_mutation_count = Arc::clone(&mutation_count);
        let server = tokio::spawn(async move {
            let mut quiet_until: Option<tokio::time::Instant> = None;
            loop {
                let accepted = match quiet_until {
                    Some(deadline) => {
                        let remaining = deadline
                            .checked_duration_since(tokio::time::Instant::now())
                            .unwrap_or_default();
                        if remaining.is_zero() {
                            break;
                        }
                        match timeout(remaining, listener.accept()).await {
                            Ok(Ok(accepted)) => accepted,
                            Ok(Err(error)) => panic!("fixture listener failed: {error}"),
                            Err(_) => break,
                        }
                    }
                    None => listener.accept().await.expect("fixture accepts HTTP"),
                };
                let (mut stream, _peer) = accepted;
                let request = read_http_request(&mut stream).await;
                assert_eq!(request.path, "/mcp");
                match (request.method.as_str(), request.body["method"].as_str()) {
                    ("POST", Some("initialize")) => {
                        fixture_initialize_count.fetch_add(1, Ordering::AcqRel);
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[
                                ("Content-Type", "application/json"),
                                ("Mcp-Session-Id", "session-1"),
                            ],
                            &body,
                        )
                        .await;
                    }
                    ("POST", Some("notifications/initialized")) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    ("POST", Some("tools/call")) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        assert_eq!(request.body["params"]["name"], "mutate");
                        fixture_mutation_count.fetch_add(1, Ordering::AcqRel);
                        write_http_response(&mut stream, "404 Not Found", &[], &[]).await;
                        quiet_until =
                            Some(tokio::time::Instant::now() + Duration::from_millis(250));
                    }
                    ("POST", Some("notifications/cancelled")) => {
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                    }
                    ("GET", _) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        assert!(
                            request.header("last-event-id").is_none(),
                            "a common SSE stream may start once, but it must not resume"
                        );
                        write_http_response(&mut stream, "405 Method Not Allowed", &[], &[]).await;
                    }
                    ("DELETE", _) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        write_http_response(&mut stream, "200 OK", &[], &[]).await;
                    }
                    _ => panic!("fixture received an unexpected HTTP operation"),
                }
            }
        });

        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "mutate", ToolClass::Mutate);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes with a session");

        let outcome = client
            .call(authorization, options())
            .await
            .expect("a session-expired mutation becomes explicitly ambiguous");
        assert!(matches!(
            outcome.outcome(),
            ToolCallOutcome::OutcomeUnknown(_)
        ));
        server
            .await
            .expect("fixture observes no session reinitialization or mutation replay");
        assert_eq!(initialize_count.load(Ordering::Acquire), 1);
        assert_eq!(mutation_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "the raw HTTP cleanup fixture asserts an exact close transcript and treats deviations as test failures"
    )]
    async fn loopback_http_close_deletes_an_established_session_once() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds a loopback listener");
        let endpoint = format!(
            "http://{}/mcp",
            listener
                .local_addr()
                .expect("fixture listener has an address")
        );
        let delete_count = Arc::new(AtomicUsize::new(0));
        let fixture_delete_count = Arc::clone(&delete_count);
        let server = tokio::spawn(async move {
            let mut quiet_until: Option<tokio::time::Instant> = None;
            loop {
                let accepted = match quiet_until {
                    Some(deadline) => {
                        let remaining = deadline
                            .checked_duration_since(tokio::time::Instant::now())
                            .unwrap_or_default();
                        if remaining.is_zero() {
                            break;
                        }
                        match timeout(remaining, listener.accept()).await {
                            Ok(Ok(accepted)) => accepted,
                            Ok(Err(error)) => panic!("fixture listener failed: {error}"),
                            Err(_) => break,
                        }
                    }
                    None => listener.accept().await.expect("fixture accepts HTTP"),
                };
                let (mut stream, _peer) = accepted;
                let Some(request) = maybe_read_http_request(&mut stream).await else {
                    continue;
                };
                assert_eq!(request.path, "/mcp");
                match (request.method.as_str(), request.body["method"].as_str()) {
                    ("POST", Some("initialize")) => {
                        let body = json_http_body(response(
                            &request.body,
                            json!({
                                "protocolVersion":"2025-11-25",
                                "capabilities":{"tools":{}},
                                "serverInfo":{"name":"fixture","version":"0.0.0"}
                            }),
                        ));
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[
                                ("Content-Type", "application/json"),
                                ("Mcp-Session-Id", "session-1"),
                            ],
                            &body,
                        )
                        .await;
                    }
                    ("POST", Some("notifications/initialized")) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        write_http_response(&mut stream, "202 Accepted", &[], &[]).await;
                        quiet_until =
                            Some(tokio::time::Instant::now() + Duration::from_millis(500));
                    }
                    ("GET", _) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        assert!(request.header("last-event-id").is_none());
                        write_http_response(&mut stream, "405 Method Not Allowed", &[], &[]).await;
                    }
                    ("DELETE", _) => {
                        assert_eq!(request.header("mcp-session-id"), Some("session-1"));
                        fixture_delete_count.fetch_add(1, Ordering::AcqRel);
                        write_http_response(&mut stream, "200 OK", &[], &[]).await;
                        quiet_until =
                            Some(tokio::time::Instant::now() + Duration::from_millis(250));
                    }
                    _ => panic!("fixture received an unexpected HTTP operation"),
                }
            }
        });

        let integration = http_integration(&endpoint);
        let authorization = call_authorization_for(&integration, "inspect", ToolClass::Observe);
        let mut client = RmcpClient::connect_for_call(&authorization, options())
            .await
            .expect("loopback HTTP client initializes with a session");

        timeout(Duration::from_millis(500), client.close())
            .await
            .expect("session close completes within its bounded cleanup period");
        server.await.expect("fixture observes session cleanup");
        assert_eq!(delete_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "the pagination fixture directly drives its timed stdio protocol transcript"
    )]
    async fn paginated_tool_listing_consumes_one_operation_deadline() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            respond_to_initialize(&mut reader, &mut server_writer).await;
            let first_page = read_message(&mut reader).await;
            assert_eq!(first_page["method"], "tools/list");
            tokio::time::sleep(Duration::from_millis(100)).await;
            write_message(
                &mut server_writer,
                response(&first_page, json!({"tools":[], "nextCursor":"next-page"})),
            )
            .await;
            let second_page = read_message(&mut reader).await;
            assert_eq!(second_page["method"], "tools/list");
            assert_eq!(second_page["params"]["cursor"], "next-page");
            // The remaining operation budget must elapse before an independent
            // second-page request timeout could be reset.
            tokio::time::sleep(Duration::from_millis(450)).await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );
        let mut client = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            options(),
        )
        .await
        .expect("fixture server initializes");

        let result = timeout(
            Duration::from_millis(350),
            client.list_tools(
                &listing_authorization(),
                RequestOptions::new(Duration::from_millis(250), CancellationToken::new()),
            ),
        )
        .await
        .expect("the total listing budget does not reset for page two");
        assert_eq!(
            result.map_err(error_kind),
            Err(RmcpClientErrorKind::TimedOut)
        );
        client.close().await;
        server
            .await
            .expect("fixture holds page two past the total deadline");
    }

    #[tokio::test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "the stalled-handshake fixture proves the public initialization deadline classification"
    )]
    async fn initialization_deadline_reports_timed_out() {
        let (client_io, server_io) = tokio::io::duplex(MAX_STDIO_MESSAGE_BYTES * 2);
        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, _server_writer) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let initialize = read_message(&mut reader).await;
            assert_eq!(initialize["method"], "initialize");
            tokio::time::sleep(Duration::from_millis(250)).await;
        });
        let health = ConnectionHealth::new();
        let cancellation = CancellationToken::new();
        let transport = bounded_stdio_transport(
            client_reader,
            client_writer,
            health.clone(),
            cancellation.clone(),
        );

        let result = RmcpClient::initialize(
            integration(),
            transport,
            None,
            health,
            cancellation,
            RequestOptions::new(Duration::from_millis(25), CancellationToken::new()),
        )
        .await;

        let error = match result {
            Ok(mut client) => {
                client.close().await;
                panic!("stalled initialization must time out")
            }
            Err(error) => error,
        };
        assert_eq!(error.kind(), RmcpClientErrorKind::TimedOut);
        assert_eq!(error.context().operation(), RmcpClientOperation::Initialize);
        assert_eq!(error.retryability(), RmcpClientRetryability::Retryable);
        server
            .await
            .expect("fixture holds initialization past deadline");
    }
}
