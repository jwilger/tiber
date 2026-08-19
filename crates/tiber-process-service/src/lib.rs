//! Durable authority boundary for configured process execution.

#![forbid(unsafe_code)]
#![expect(
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    reason = "EventCore derives generate public helpers without item-local lint hooks"
)]

use core::{error::Error, fmt, time::Duration};
use std::path::{Path, PathBuf};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tiber_process_core::{
    AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog, MAX_OUTPUT_BYTES,
    ProcessInvocationId, ProcessRequest,
};
use tiber_workflow_core::EffectId;

/// Maximum durable per-invocation streams admitted for one workflow effect.
pub const MAX_PROCESS_INVOCATION_STREAMS: usize = 64;

/// Stable failures at the durable process-authority boundary.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "service failures remain grouped by admission, authority, modeling, and resource lifecycle"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProcessServiceError {
    /// An effect identity could not form an `EventCore` stream.
    InvalidStream,
    /// The supplied stream is not owned by the request's exact durable effect.
    StreamRequestMismatch,
    /// Retained facts were malformed, conflicting, or from another stream.
    InvalidHistory,
    /// The durable request was refused by trusted configuration.
    RequestRefused,
    /// Signed requested and prepared facts were not both retained.
    PreparedHistoryRequired,
    /// Trusted configuration changed after preparation.
    CatalogChanged,
    /// Checked command logic rejected the transition.
    ModeledCommandFailed,
    /// Checked command logic emitted an invalid event set.
    InvalidModeledEmission,
    /// Captured process output exceeded the configured semantic bound.
    OutputTooLarge,
    /// A distinct invocation would exceed the bounded restart-recovery set.
    InvocationLimitReached,
}

impl ProcessServiceError {
    /// Returns the stable sanitized machine-readable code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidHistory => "process_history_invalid",
            Self::RequestRefused => "process_request_refused",
            Self::PreparedHistoryRequired => "process_prepared_history_required",
            Self::CatalogChanged => "process_catalog_changed",
            Self::StreamRequestMismatch => "process_stream_request_mismatch",
            Self::OutputTooLarge => "process_output_too_large",
            Self::InvocationLimitReached => "process_invocation_limit_reached",
            Self::InvalidStream | Self::ModeledCommandFailed | Self::InvalidModeledEmission => {
                "process_authority_rejected"
            }
        }
    }
}

impl fmt::Display for ProcessServiceError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "sanitized service failures retain no external causal source"
)]
impl Error for ProcessServiceError {}

/// One exact process stream owned by a workflow effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessStream(StreamId);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "restart discovery precedes constructors so the durable stream grammar is stated before it is emitted"
)]
impl ProcessStream {
    /// Selects a verified per-invocation stream owned by one exact effect.
    #[must_use]
    #[inline]
    pub fn from_verified_effect_stream(effect_id: &EffectId, stream: &StreamId) -> Option<Self> {
        let prefix = format!("tiber:process:{}:", effect_id.as_str());
        let value = stream.as_ref();
        let digest = value.strip_prefix(&prefix)?;
        (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| Self(stream.clone()))
    }

    /// Builds the stream for one exact invocation under a durable effect.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessServiceError::InvalidStream`] when `EventCore` rejects
    /// the correlation-derived stream identity.
    #[inline]
    pub fn for_invocation(
        effect_id: &EffectId,
        invocation_id: &ProcessInvocationId,
    ) -> Result<Self, ProcessServiceError> {
        let digest = Sha256::digest(invocation_id.as_str().as_bytes());
        StreamId::try_new(format!("tiber:process:{}:{digest:x}", effect_id.as_str()))
            .map(Self)
            .map_err(|_source| ProcessServiceError::InvalidStream)
    }

    /// Builds the exact stream bound into a semantic process request.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessServiceError::InvalidStream`] when the request identity
    /// cannot form an `EventCore` stream.
    #[inline]
    pub fn for_request(request: &ProcessRequest) -> Result<Self, ProcessServiceError> {
        Self::for_invocation(request.provenance().effect_id(), request.invocation_id())
    }
}

impl StreamIdentity for ProcessStream {
    #[inline]
    fn as_stream_id(&self) -> &StreamId {
        &self.0
    }
}

/// Safe identity proving which semantic request was prepared.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedProcessIdentity {
    /// Opaque identity of the exact trusted catalog entry used for preparation.
    catalog_entry: CatalogEntryIdentity,
    /// Exact semantic request represented by the prepared identity.
    request: ProcessRequest,
}

/// Ephemeral bounded process bytes that preserve arbitrary non-UTF8 output.
#[derive(Clone, Eq, PartialEq)]
pub struct CapturedProcessBytes(Vec<u8>);

impl fmt::Debug for CapturedProcessBytes {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedProcessBytes")
            .field("byte_count", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated construction precedes access to the bounded captured bytes"
)]
impl CapturedProcessBytes {
    /// Constructs one bounded captured output stream.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessServiceError::OutputTooLarge`] when the configured
    /// process-output bound is exceeded.
    #[inline]
    pub fn new(bytes: Vec<u8>) -> Result<Self, ProcessServiceError> {
        if bytes.len() > MAX_OUTPUT_BYTES {
            return Err(ProcessServiceError::OutputTooLarge);
        }
        Ok(Self(bytes))
    }

    /// Returns the exact captured bytes.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Durable content-free identity of one captured output stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapturedProcessIdentity {
    /// Exact captured byte count.
    byte_count: usize,
    /// Domain-separated digest of the captured bytes.
    digest: [u8; 32],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "private digest construction precedes the public content-free byte count inspector"
)]
impl CapturedProcessIdentity {
    /// Derives a durable output identity without retaining raw bytes.
    fn from_bytes(domain: &'static [u8], bytes: &CapturedProcessBytes) -> Self {
        let mut digest = Sha256::new();
        hash_bytes(&mut digest, domain);
        hash_bytes(&mut digest, bytes.as_bytes());
        Self {
            byte_count: bytes.as_bytes().len(),
            digest: digest.finalize().into(),
        }
    }

    /// Returns the exact captured byte count.
    #[must_use]
    #[inline]
    pub const fn byte_count(&self) -> usize {
        self.byte_count
    }
}

/// Semantic terminal exit status independent of platform adapter types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ProcessExitStatus {
    /// The process exited with an exact numeric status code.
    Exited(i32),
}

/// Durable receipt for one definitively completed prepared process.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "receipt fields follow process lifecycle order: identity, exit, stdout, then stderr"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessReceipt {
    /// Exact prepared process identity.
    identity: PreparedProcessIdentity,
    /// Semantic exit status.
    status: ProcessExitStatus,
    /// Content-free captured stdout identity.
    stdout: CapturedProcessIdentity,
    /// Content-free captured stderr identity.
    stderr: CapturedProcessIdentity,
}

/// Stable content-free spawn failure categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ProcessSpawnFailureCode {
    /// The trusted executable was unavailable at dispatch time.
    ExecutableUnavailable,
    /// The operating system denied process creation.
    PermissionDenied,
    /// A transient local resource prevented process creation.
    ResourceUnavailable,
}

/// Durable content-free spawn failure bound to one prepared process.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "spawn failure identity precedes its sanitized failure classification"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessSpawnFailure {
    /// Exact prepared process identity.
    identity: PreparedProcessIdentity,
    /// Stable content-free failure category.
    code: ProcessSpawnFailureCode,
}

/// Durable timeout terminal bound to one prepared process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessTimedOut {
    /// Exact prepared process identity.
    identity: PreparedProcessIdentity,
}

/// Durable cancellation terminal bound to one prepared process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessCancelled {
    /// Exact prepared process identity.
    identity: PreparedProcessIdentity,
}

/// Durable content-free uncertainty bound to one prepared process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessUnknown {
    /// Exact prepared process identity whose external completion is uncertain.
    identity: PreparedProcessIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated unknown-state construction precedes identity inspection"
)]
impl ProcessUnknown {
    /// Constructs an unknown terminal for one prepared process.
    #[must_use]
    #[inline]
    pub const fn new(identity: PreparedProcessIdentity) -> Self {
        Self { identity }
    }

    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }
}

/// Closed content-free result of read-only process reconciliation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ProcessReconciliationOutcome {
    /// A durable completed-process identity was found without redispatch.
    Completed(Box<ProcessReceipt>),
    /// The external process definitely did not complete.
    DefinitelyNotCompleted,
    /// Read-only inspection could not resolve the uncertainty.
    StillUnknown,
}

/// Durable reconciliation result bound to one exact unknown process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessReconciled {
    /// Exact prepared process identity reconciled by read-only inspection.
    identity: PreparedProcessIdentity,
    /// Closed content-free reconciliation outcome.
    outcome: ProcessReconciliationOutcome,
}

impl ProcessReconciled {
    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }

    /// Returns the closed reconciliation outcome.
    #[must_use]
    #[inline]
    pub const fn outcome(&self) -> &ProcessReconciliationOutcome {
        &self.outcome
    }
}

/// Non-cloneable authority for read-only inspection of one unknown process.
pub struct ProcessReconciliationCapability {
    /// Exact prepared identity that may be inspected but never redispatched.
    identity: PreparedProcessIdentity,
}

/// Non-cloneable authority to retire private adapter artifacts for one exact
/// signed closed lifecycle.
pub struct ProcessRetirementCapability {
    /// Exact prepared identity whose private artifacts may be retired.
    identity: PreparedProcessIdentity,
}

impl ProcessRetirementCapability {
    /// Consumes this one-shot authority into the exact identity whose private
    /// artifacts may be retired.
    #[must_use]
    #[inline]
    pub fn into_prepared_identity(self) -> PreparedProcessIdentity {
        self.identity
    }

    /// Returns the exact prepared identity whose private artifacts may be
    /// retired.
    #[must_use]
    #[inline]
    pub const fn prepared_identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }
}

/// Service-owned classification of one exact signed lifecycle at restart.
///
/// This closed decision prevents callers from granting recovery meaning to a
/// tail fact without first validating the complete per-invocation history.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "restart states follow external lifecycle order from preparation through reconciliation and closure"
)]
#[derive(Debug)]
#[non_exhaustive]
pub enum ProcessRestartState {
    /// Dispatch was durably prepared but its external outcome is not recorded.
    Prepared(PreparedProcessIdentity),
    /// An unknown process may be inspected exactly once without redispatch.
    Unknown(ProcessReconciliationCapability),
    /// A completed reconciliation may be projected without another inspection.
    Reconciled(ProcessReconciliationOutcome),
    /// A refusal or definitive terminal requires no restart action.
    Closed,
}

impl fmt::Debug for ProcessReconciliationCapability {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProcessReconciliationCapability(<opaque>)")
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "capability identity inspection precedes its consuming reconciliation transition"
)]
impl ProcessReconciliationCapability {
    /// Returns the exact identity available to read-only reconciliation.
    #[must_use]
    #[inline]
    pub const fn prepared_identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }

    /// Consumes the one-shot read-only capability into a durable result.
    #[must_use]
    #[inline]
    pub fn into_reconciled(self, outcome: ProcessReconciliationOutcome) -> ProcessReconciled {
        ProcessReconciled {
            identity: self.identity,
            outcome,
        }
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated cancellation construction precedes identity inspection"
)]
impl ProcessCancelled {
    /// Constructs a cancellation terminal for one prepared process.
    #[must_use]
    #[inline]
    pub const fn new(identity: PreparedProcessIdentity) -> Self {
        Self { identity }
    }

    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated timeout construction precedes identity inspection"
)]
impl ProcessTimedOut {
    /// Constructs a timeout terminal for one prepared process.
    #[must_use]
    #[inline]
    pub const fn new(identity: PreparedProcessIdentity) -> Self {
        Self { identity }
    }

    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated spawn-failure construction precedes receipt inspectors"
)]
impl ProcessSpawnFailure {
    /// Constructs a typed content-free spawn failure.
    #[must_use]
    #[inline]
    pub const fn new(identity: PreparedProcessIdentity, code: ProcessSpawnFailureCode) -> Self {
        Self { identity, code }
    }

    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }

    /// Returns the stable failure category.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> ProcessSpawnFailureCode {
        self.code
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "validated receipt construction precedes lifecycle-ordered inspectors"
)]
impl ProcessReceipt {
    /// Constructs a completed-process receipt within its prepared output limits.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessServiceError::OutputTooLarge`] when either captured
    /// stream exceeds the exact limit bound into the prepared identity.
    #[inline]
    pub fn new(
        identity: PreparedProcessIdentity,
        status: ProcessExitStatus,
        stdout: &CapturedProcessBytes,
        stderr: &CapturedProcessBytes,
    ) -> Result<Self, ProcessServiceError> {
        if stdout.as_bytes().len() > identity.catalog_entry.stdout_limit_bytes
            || stderr.as_bytes().len() > identity.catalog_entry.stderr_limit_bytes
        {
            return Err(ProcessServiceError::OutputTooLarge);
        }
        Ok(Self {
            identity,
            status,
            stdout: CapturedProcessIdentity::from_bytes(b"stdout", stdout),
            stderr: CapturedProcessIdentity::from_bytes(b"stderr", stderr),
        })
    }

    /// Returns the exact prepared process identity.
    #[must_use]
    #[inline]
    pub const fn identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }

    /// Returns the semantic exit status.
    #[must_use]
    #[inline]
    pub const fn status(&self) -> ProcessExitStatus {
        self.status
    }

    /// Returns the content-free captured stdout identity.
    #[must_use]
    #[inline]
    pub const fn stdout(&self) -> &CapturedProcessIdentity {
        &self.stdout
    }

    /// Returns the content-free captured stderr identity.
    #[must_use]
    #[inline]
    pub const fn stderr(&self) -> &CapturedProcessIdentity {
        &self.stderr
    }
}

/// Opaque, content-free identity of one exact trusted catalog entry.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "catalog identity capture bounds remain in stdout-then-stderr lifecycle order"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogEntryIdentity {
    /// Domain-separated digest of every execution-relevant catalog field.
    digest: [u8; 32],
    /// Prepared stdout capture limit.
    stdout_limit_bytes: usize,
    /// Prepared stderr capture limit.
    stderr_limit_bytes: usize,
}

impl PreparedProcessIdentity {
    /// Returns the exact semantic request prepared for later authorization.
    #[must_use]
    #[inline]
    pub const fn request(&self) -> &ProcessRequest {
        &self.request
    }
}

/// Sanitized immutable process lifecycle facts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ProcessRefusal {
    /// The requested semantic ID was absent from trusted configuration.
    #[serde(rename = "process_policy_unknown_configured_command")]
    UnknownConfiguredCommand,
}

/// Content-free identity of one refused semantic process request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RefusedProcessIdentity {
    /// Opaque digest of the unconfigured command identity.
    command_id_digest: [u8; 32],
    /// Exact invocation retained without exposing the refused command identity.
    invocation_id: ProcessInvocationId,
    /// Trusted workflow provenance retained for exact reconciliation.
    provenance: AssignmentWorkflowProvenance,
}

impl RefusedProcessIdentity {
    /// Builds the safe durable identity for a refused request.
    fn from_request(request: &ProcessRequest) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"tiber-process-refused-command-id-v1");
        digest.update(request.command_id().as_str().as_bytes());
        Self {
            command_id_digest: digest.finalize().into(),
            invocation_id: request.invocation_id().clone(),
            provenance: request.provenance().clone(),
        }
    }

    /// Reports whether this identity belongs to the exact semantic request.
    fn matches(&self, request: &ProcessRequest) -> bool {
        self == &Self::from_request(request)
    }

    /// Reports whether the refusal belongs to the exact effect-owned invocation stream.
    fn matches_stream(&self, stream: &ProcessStream) -> bool {
        ProcessStream::for_invocation(self.provenance.effect_id(), &self.invocation_id).as_ref()
            == Ok(stream)
    }
}

impl ProcessRefusal {
    /// Returns the stable sanitized policy refusal code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownConfiguredCommand => "process_policy_unknown_configured_command",
        }
    }
}

/// Sanitized immutable process lifecycle facts.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "durable facts are ordered by the process lifecycle rather than alphabetically"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ProcessFact {
    /// A semantic request matched trusted configuration.
    Requested(ProcessRequest),
    /// Trusted configuration refused the semantic request.
    Refused {
        /// Content-free refused request identity.
        identity: RefusedProcessIdentity,
        /// Stable sanitized refusal code.
        code: ProcessRefusal,
    },
    /// The exact admitted request was prepared for dispatch authorization.
    Prepared(PreparedProcessIdentity),
    /// The prepared process definitively completed.
    Completed(ProcessReceipt),
    /// The adapter definitively failed to spawn the prepared process.
    SpawnFailed(ProcessSpawnFailure),
    /// The prepared process exceeded its trusted deadline.
    TimedOut(ProcessTimedOut),
    /// The prepared process was explicitly cancelled.
    Cancelled(ProcessCancelled),
    /// External completion became uncertain; dispatch may never be repeated.
    Unknown(ProcessUnknown),
    /// Read-only inspection recorded one closed outcome for an unknown process.
    Reconciled(ProcessReconciled),
}

/// Durable `EventCore` envelope for process authority.
#[derive(Clone, Debug, Deserialize, Eq, ModelEvent, PartialEq, Serialize)]
pub struct ProcessEvent {
    /// Sanitized immutable lifecycle fact.
    fact: ProcessFact,
    /// Exact effect-owned stream.
    stream: StreamId,
}

impl ProcessEvent {
    /// Returns the immutable lifecycle fact.
    #[must_use]
    #[inline]
    pub const fn fact(&self) -> &ProcessFact {
        &self.fact
    }
}

impl Event for ProcessEvent {
    #[inline]
    fn event_type_name() -> &'static str {
        "TiberProcessEvent"
    }

    #[inline]
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the modeled view follows envelope stream then fact projection order"
)]
#[derive(ModelOutput)]
/// Modeled view of one durable process event.
struct ProcessEventView {
    /// Exact effect-owned stream projected from the event.
    stream: StreamId,
    /// Sanitized fact projected from the event.
    fact: ProcessFact,
}

mapping! { ProcessEventStreamToView: ProcessEvent.stream => ProcessEventView.stream using clone; }
mapping! { ProcessEventFactToView: ProcessEvent.fact => ProcessEventView.fact using clone; }

impl ProcessEventView {
    /// Projects one durable event into checked modeled state.
    #[inline]
    fn from_event(event: &ProcessEvent) -> Modeled<Self> {
        Self::model_builder()
            .stream(ProcessEventStreamToView::apply(event))
            .fact(ProcessEventFactToView::apply(event))
            .build()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated request input fields follow stream, request, then catalog-decision lifecycle order"
)]
#[derive(ModelInput)]
/// Modeled input binding for request admission.
struct RequestProcessInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Semantic process intent.
    request: ProcessRequest,
    #[model(origin)]
    /// Trusted catalog entry identity, absent when policy refused the ID.
    catalog_entry: Option<CatalogEntryIdentity>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated request command fields follow stream, request, then catalog-decision lifecycle order"
)]
#[derive(ModelCommand)]
/// Checked request-admission command.
struct RequestProcess {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Semantic process intent.
    request: ProcessRequest,
    /// Trusted catalog entry identity, absent when policy refused the ID.
    catalog_entry: Option<CatalogEntryIdentity>,
}

mapping! { RequestInputToStream: RequestProcessInput.stream => RequestProcess.stream using clone; }
mapping! { RequestInputToRequest: RequestProcessInput.request => RequestProcess.request using clone; }
mapping! { RequestInputToCatalogEntry: RequestProcessInput.catalog_entry => RequestProcess.catalog_entry using clone; }
mapping! { RequestStreamToEvent: RequestProcess.stream => ProcessEvent.stream using stream_id; }
mapping! { RequestToFact: (RequestProcess.request, RequestProcess.catalog_entry) => ProcessEvent.fact using request_fact; }

#[derive(ModelState)]
/// Folded state for request admission.
struct RequestProcessState {
    #[model(default)]
    /// Whether this exact stream already contains a durable fact.
    occupied: bool,
}

#[derive(ModelOutput)]
/// Modeled request-admission decision.
struct RequestProcessDecision {
    /// Whether this exact stream already contains a durable fact.
    occupied: bool,
}

mapping! { RequestStateToOccupied: RequestProcessState.occupied => RequestProcessDecision.occupied using copy; }

#[expect(
    clippy::missing_trait_methods,
    reason = "request admission has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for RequestProcess {
    type Event = ProcessEvent;
    type State = RequestProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = ProcessEventView::from_event(event);
        Modeled::from_built(RequestProcessState {
            occupied: state.as_ref().occupied
                || view.as_ref().stream == *self.stream.as_stream_id(),
        })
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RequestProcessDecision::model_builder()
            .occupied(RequestStateToOccupied::apply(state.as_ref()))
            .build();
        if decision.as_ref().occupied {
            return Err(command_error("process_stream_occupied"));
        }
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(RequestStreamToEvent::apply(self))
                .fact(RequestToFact::apply((self, self)))
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated preparation input fields retain catalog, stream, then request lifecycle framing"
)]
#[derive(ModelInput)]
/// Modeled input binding for dispatch preparation.
struct PrepareProcessInput {
    /// Opaque identity of the exact trusted catalog entry.
    #[model(origin)]
    catalog_entry: CatalogEntryIdentity,
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Semantic process intent.
    request: ProcessRequest,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated preparation command fields retain catalog, stream, then request lifecycle framing"
)]
#[derive(ModelCommand)]
/// Checked dispatch-preparation command.
struct PrepareProcess {
    /// Opaque identity of the exact trusted catalog entry.
    catalog_entry: CatalogEntryIdentity,
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Semantic process intent.
    request: ProcessRequest,
}

mapping! { PrepareInputToStream: PrepareProcessInput.stream => PrepareProcess.stream using clone; }
mapping! { PrepareInputToRequest: PrepareProcessInput.request => PrepareProcess.request using clone; }
mapping! { PrepareInputToCatalogEntry: PrepareProcessInput.catalog_entry => PrepareProcess.catalog_entry using clone; }
mapping! { PrepareStreamToEvent: PrepareProcess.stream => ProcessEvent.stream using stream_id; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "preparation state records requested identity before its malformed-history marker"
)]
#[derive(ModelState)]
/// Folded state for dispatch preparation.
struct PrepareProcessState {
    #[model(default)]
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    #[model(default)]
    /// Whether retained history violates lifecycle or stream identity.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "preparation decision carries request identity before its malformed-history marker"
)]
#[derive(ModelOutput)]
/// Modeled dispatch-preparation decision.
struct PrepareProcessDecision {
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    /// Whether retained history violates lifecycle or stream identity.
    malformed: bool,
}

mapping! { PrepareStateToRequested: PrepareProcessState.requested => PrepareProcessDecision.requested using clone; }
mapping! { PrepareStateToMalformed: PrepareProcessState.malformed => PrepareProcessDecision.malformed using copy; }
mapping! { PrepareToFact: (PrepareProcess.request, PrepareProcess.catalog_entry, PrepareProcessDecision.requested, PrepareProcessDecision.malformed) => ProcessEvent.fact using try prepared_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "preparation has no related streams and folds borrowed closed facts from generated modeled state"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for PrepareProcess {
    type Event = ProcessEvent;
    type State = PrepareProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = ProcessEventView::from_event(event);
        if view.as_ref().stream == *self.stream.as_stream_id() {
            match &view.as_ref().fact {
                ProcessFact::Requested(request) if folded.requested.is_none() => {
                    folded.requested = Some(request.clone());
                }
                ProcessFact::Requested(_)
                | ProcessFact::Refused { .. }
                | ProcessFact::Prepared(_)
                | ProcessFact::Completed(_)
                | ProcessFact::SpawnFailed(_)
                | ProcessFact::TimedOut(_)
                | ProcessFact::Cancelled(_)
                | ProcessFact::Unknown(_)
                | ProcessFact::Reconciled(_) => {
                    folded.malformed = true;
                }
            }
        } else {
            folded.malformed = true;
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = PrepareProcessDecision::model_builder()
            .requested(PrepareStateToRequested::apply(state.as_ref()))
            .malformed(PrepareStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(PrepareStreamToEvent::apply(self))
                .fact(PrepareToFact::apply((
                    self,
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording definitive completion.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated completion input fields follow stream then receipt lifecycle order"
)]
struct CompleteProcessInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact completed-process receipt.
    receipt: ProcessReceipt,
}

#[derive(ModelCommand)]
/// Checked definitive-completion command.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated completion command fields follow stream then receipt lifecycle order"
)]
struct CompleteProcess {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact completed-process receipt.
    receipt: ProcessReceipt,
}

mapping! { CompleteInputToStream: CompleteProcessInput.stream => CompleteProcess.stream using clone; }
mapping! { CompleteInputToReceipt: CompleteProcessInput.receipt => CompleteProcess.receipt using clone; }
mapping! { CompleteStreamToEvent: CompleteProcess.stream => ProcessEvent.stream using stream_id; }

#[derive(ModelState)]
/// Folded state for recording definitive completion.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "completion state follows requested, prepared, terminal, then malformed lifecycle order"
)]
struct CompleteProcessState {
    #[model(default)]
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    #[model(default)]
    /// Exact prepared identity, if retained.
    prepared: Option<PreparedProcessIdentity>,
    #[model(default)]
    /// Whether any terminal fact is already retained.
    terminal: bool,
    #[model(default)]
    /// Whether retained lifecycle or stream identity is malformed.
    malformed: bool,
}

#[derive(ModelOutput)]
/// Modeled definitive-completion decision.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "completion decision follows requested, prepared, terminal, then malformed lifecycle order"
)]
struct CompleteProcessDecision {
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    /// Exact prepared identity, if retained.
    prepared: Option<PreparedProcessIdentity>,
    /// Whether any terminal fact is already retained.
    terminal: bool,
    /// Whether retained lifecycle or stream identity is malformed.
    malformed: bool,
}

mapping! { CompleteStateToRequested: CompleteProcessState.requested => CompleteProcessDecision.requested using clone; }
mapping! { CompleteStateToPrepared: CompleteProcessState.prepared => CompleteProcessDecision.prepared using clone; }
mapping! { CompleteStateToTerminal: CompleteProcessState.terminal => CompleteProcessDecision.terminal using copy; }
mapping! { CompleteStateToMalformed: CompleteProcessState.malformed => CompleteProcessDecision.malformed using copy; }
mapping! { CompleteToFact: (CompleteProcess.receipt, CompleteProcessDecision.requested, CompleteProcessDecision.prepared, CompleteProcessDecision.terminal, CompleteProcessDecision.malformed) => ProcessEvent.fact using try completed_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "completion has no related streams and folds borrowed closed facts from generated modeled state"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for CompleteProcess {
    type Event = ProcessEvent;
    type State = CompleteProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_terminal_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CompleteProcessDecision::model_builder()
            .requested(CompleteStateToRequested::apply(state.as_ref()))
            .prepared(CompleteStateToPrepared::apply(state.as_ref()))
            .terminal(CompleteStateToTerminal::apply(state.as_ref()))
            .malformed(CompleteStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(CompleteStreamToEvent::apply(self))
                .fact(CompleteToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording definitive spawn failure.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated spawn-failure input fields follow stream then failure lifecycle order"
)]
struct FailProcessSpawnInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact content-free spawn failure.
    failure: ProcessSpawnFailure,
}

#[derive(ModelCommand)]
/// Checked definitive spawn-failure command.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated spawn-failure command fields follow stream then failure lifecycle order"
)]
struct FailProcessSpawn {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact content-free spawn failure.
    failure: ProcessSpawnFailure,
}

mapping! { FailSpawnInputToStream: FailProcessSpawnInput.stream => FailProcessSpawn.stream using clone; }
mapping! { FailSpawnInputToFailure: FailProcessSpawnInput.failure => FailProcessSpawn.failure using clone; }
mapping! { FailSpawnStreamToEvent: FailProcessSpawn.stream => ProcessEvent.stream using stream_id; }
mapping! { FailSpawnToFact: (FailProcessSpawn.failure, CompleteProcessDecision.requested, CompleteProcessDecision.prepared, CompleteProcessDecision.terminal, CompleteProcessDecision.malformed) => ProcessEvent.fact using try spawn_failed_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "spawn failure has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for FailProcessSpawn {
    type Event = ProcessEvent;
    type State = CompleteProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_terminal_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CompleteProcessDecision::model_builder()
            .requested(CompleteStateToRequested::apply(state.as_ref()))
            .prepared(CompleteStateToPrepared::apply(state.as_ref()))
            .terminal(CompleteStateToTerminal::apply(state.as_ref()))
            .malformed(CompleteStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(FailSpawnStreamToEvent::apply(self))
                .fact(FailSpawnToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording a timeout terminal.
struct TimeOutProcessInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact timeout terminal.
    timed_out: ProcessTimedOut,
}

#[derive(ModelCommand)]
/// Checked timeout-terminal command.
struct TimeOutProcess {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact timeout terminal.
    timed_out: ProcessTimedOut,
}

mapping! { TimeOutInputToStream: TimeOutProcessInput.stream => TimeOutProcess.stream using clone; }
mapping! { TimeOutInputToTerminal: TimeOutProcessInput.timed_out => TimeOutProcess.timed_out using clone; }
mapping! { TimeOutStreamToEvent: TimeOutProcess.stream => ProcessEvent.stream using stream_id; }
mapping! { TimeOutToFact: (TimeOutProcess.timed_out, CompleteProcessDecision.requested, CompleteProcessDecision.prepared, CompleteProcessDecision.terminal, CompleteProcessDecision.malformed) => ProcessEvent.fact using try timed_out_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "timeout has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for TimeOutProcess {
    type Event = ProcessEvent;
    type State = CompleteProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_terminal_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CompleteProcessDecision::model_builder()
            .requested(CompleteStateToRequested::apply(state.as_ref()))
            .prepared(CompleteStateToPrepared::apply(state.as_ref()))
            .terminal(CompleteStateToTerminal::apply(state.as_ref()))
            .malformed(CompleteStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(TimeOutStreamToEvent::apply(self))
                .fact(TimeOutToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording cancellation.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated cancellation input fields follow stream then cancellation lifecycle order"
)]
struct CancelProcessInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact cancellation terminal.
    cancelled: ProcessCancelled,
}

#[derive(ModelCommand)]
/// Checked cancellation-terminal command.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated cancellation command fields follow stream then cancellation lifecycle order"
)]
struct CancelProcess {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact cancellation terminal.
    cancelled: ProcessCancelled,
}

mapping! { CancelInputToStream: CancelProcessInput.stream => CancelProcess.stream using clone; }
mapping! { CancelInputToTerminal: CancelProcessInput.cancelled => CancelProcess.cancelled using clone; }
mapping! { CancelStreamToEvent: CancelProcess.stream => ProcessEvent.stream using stream_id; }
mapping! { CancelToFact: (CancelProcess.cancelled, CompleteProcessDecision.requested, CompleteProcessDecision.prepared, CompleteProcessDecision.terminal, CompleteProcessDecision.malformed) => ProcessEvent.fact using try cancelled_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "cancellation has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for CancelProcess {
    type Event = ProcessEvent;
    type State = CompleteProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_terminal_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CompleteProcessDecision::model_builder()
            .requested(CompleteStateToRequested::apply(state.as_ref()))
            .prepared(CompleteStateToPrepared::apply(state.as_ref()))
            .terminal(CompleteStateToTerminal::apply(state.as_ref()))
            .malformed(CompleteStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(CancelStreamToEvent::apply(self))
                .fact(CancelToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording uncertain external completion.
struct MarkProcessUnknownInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact unknown terminal.
    unknown: ProcessUnknown,
}

#[derive(ModelCommand)]
/// Checked unknown-terminal command.
struct MarkProcessUnknown {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact unknown terminal.
    unknown: ProcessUnknown,
}

mapping! { MarkUnknownInputToStream: MarkProcessUnknownInput.stream => MarkProcessUnknown.stream using clone; }
mapping! { MarkUnknownInputToTerminal: MarkProcessUnknownInput.unknown => MarkProcessUnknown.unknown using clone; }
mapping! { MarkUnknownStreamToEvent: MarkProcessUnknown.stream => ProcessEvent.stream using stream_id; }
mapping! { MarkUnknownToFact: (MarkProcessUnknown.unknown, CompleteProcessDecision.requested, CompleteProcessDecision.prepared, CompleteProcessDecision.terminal, CompleteProcessDecision.malformed) => ProcessEvent.fact using try unknown_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "unknown recording has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for MarkProcessUnknown {
    type Event = ProcessEvent;
    type State = CompleteProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_terminal_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CompleteProcessDecision::model_builder()
            .requested(CompleteStateToRequested::apply(state.as_ref()))
            .prepared(CompleteStateToPrepared::apply(state.as_ref()))
            .terminal(CompleteStateToTerminal::apply(state.as_ref()))
            .malformed(CompleteStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(MarkUnknownStreamToEvent::apply(self))
                .fact(MarkUnknownToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled input binding for recording read-only reconciliation.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated reconciliation input fields follow stream then reconciliation lifecycle order"
)]
struct ReconcileProcessInput {
    #[model(origin)]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    #[model(origin)]
    /// Exact reconciliation result.
    reconciled: ProcessReconciled,
}

#[derive(ModelCommand)]
/// Checked reconciliation-result command.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "generated reconciliation command fields follow stream then reconciliation lifecycle order"
)]
struct ReconcileProcess {
    #[stream]
    /// Exact effect-owned stream.
    stream: ProcessStream,
    /// Exact reconciliation result.
    reconciled: ProcessReconciled,
}

mapping! { ReconcileInputToStream: ReconcileProcessInput.stream => ReconcileProcess.stream using clone; }
mapping! { ReconcileInputToResult: ReconcileProcessInput.reconciled => ReconcileProcess.reconciled using clone; }
mapping! { ReconcileStreamToEvent: ReconcileProcess.stream => ProcessEvent.stream using stream_id; }

#[derive(ModelState)]
/// Folded state for recording one reconciliation result.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "reconciliation state follows requested, prepared, unknown, reconciled, then malformed lifecycle order"
)]
struct ReconcileProcessState {
    #[model(default)]
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    #[model(default)]
    /// Exact prepared identity, if retained after its request.
    prepared: Option<PreparedProcessIdentity>,
    #[model(default)]
    /// Whether one exact unknown terminal was retained.
    unknown: bool,
    #[model(default)]
    /// Whether one exact reconciliation result was retained.
    reconciled: bool,
    #[model(default)]
    /// Whether retained history violates lifecycle or stream identity.
    malformed: bool,
}

#[derive(ModelOutput)]
/// Modeled reconciliation decision.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "reconciliation decision follows requested, prepared, unknown, reconciled, then malformed lifecycle order"
)]
struct ReconcileProcessDecision {
    /// Exact admitted request, if retained.
    requested: Option<ProcessRequest>,
    /// Exact prepared identity, if retained after its request.
    prepared: Option<PreparedProcessIdentity>,
    /// Whether one exact unknown terminal was retained.
    unknown: bool,
    /// Whether one exact reconciliation result was retained.
    reconciled: bool,
    /// Whether retained history violates lifecycle or stream identity.
    malformed: bool,
}

mapping! { ReconcileStateToRequested: ReconcileProcessState.requested => ReconcileProcessDecision.requested using clone; }
mapping! { ReconcileStateToPrepared: ReconcileProcessState.prepared => ReconcileProcessDecision.prepared using clone; }
mapping! { ReconcileStateToUnknown: ReconcileProcessState.unknown => ReconcileProcessDecision.unknown using copy; }
mapping! { ReconcileStateToReconciled: ReconcileProcessState.reconciled => ReconcileProcessDecision.reconciled using copy; }
mapping! { ReconcileStateToMalformed: ReconcileProcessState.malformed => ReconcileProcessDecision.malformed using copy; }
mapping! { ReconcileToFact: (ReconcileProcess.reconciled, ReconcileProcessDecision.requested, ReconcileProcessDecision.prepared, ReconcileProcessDecision.unknown, ReconcileProcessDecision.reconciled, ReconcileProcessDecision.malformed) => ProcessEvent.fact using try reconciled_fact, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "reconciliation has no related streams beyond its exact effect stream"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore lifecycle implementations present state evolution before transition decisions"
)]
impl ModelCommandLogic for ReconcileProcess {
    type Event = ProcessEvent;
    type State = ReconcileProcessState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        fold_reconciliation_state(state, event, &self.stream)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ReconcileProcessDecision::model_builder()
            .requested(ReconcileStateToRequested::apply(state.as_ref()))
            .prepared(ReconcileStateToPrepared::apply(state.as_ref()))
            .unknown(ReconcileStateToUnknown::apply(state.as_ref()))
            .reconciled(ReconcileStateToReconciled::apply(state.as_ref()))
            .malformed(ReconcileStateToMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ProcessEvent::model_builder()
                .stream(ReconcileStreamToEvent::apply(self))
                .fact(ReconcileToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

/// Closed publication batch for one process policy decision.
#[derive(Debug)]
pub struct ProcessPublication {
    /// Ordered closed event batch selected by modeled commands.
    events: Vec<ProcessEvent>,
    /// Exact consistency stream for the batch.
    stream: ProcessStream,
}

impl ProcessPublication {
    /// Consumes the publication into ordered events and its exact consistency fence.
    #[must_use]
    #[inline]
    pub fn into_events_and_consistency_streams(self) -> (Vec<ProcessEvent>, [ProcessStream; 1]) {
        (self.events, [self.stream])
    }
}

/// Opaque, non-cloneable process authority consumable by an adapter.
pub struct AuthorizedProcess {
    /// Fixed adapter plan concealed until the one-shot authority is consumed.
    plan: AdapterExecutionPlan,
}

impl fmt::Debug for AuthorizedProcess {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizedProcess(<opaque>)")
    }
}

impl AuthorizedProcess {
    /// Consumes this authority and reveals the fixed adapter plan.
    #[must_use]
    #[inline]
    pub fn into_adapter_execution_plan(self) -> AdapterExecutionPlan {
        self.plan
    }
}

/// Fixed execution data exposed only after consuming verified authority.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "adapter plan fields follow dispatch framing: identity, executable, argv, cwd, environment, bounds, provenance"
)]
pub struct AdapterExecutionPlan {
    /// Exact durable identity prepared for this one-shot execution.
    identity: PreparedProcessIdentity,
    /// Trusted absolute executable path.
    program: PathBuf,
    /// Literal direct argv.
    argv: Vec<String>,
    /// Repository-relative working directory.
    cwd: PathBuf,
    /// Exact fixed child environment.
    environment: Vec<(String, String)>,
    /// Nonzero execution deadline.
    timeout: Duration,
    /// Stdout capture bound.
    stdout_bytes: usize,
    /// Stderr capture bound.
    stderr_bytes: usize,
    /// Workflow and assignment provenance bound to authority.
    provenance: AssignmentWorkflowProvenance,
}

impl fmt::Debug for AdapterExecutionPlan {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdapterExecutionPlan(<redacted>)")
    }
}

#[expect(
    clippy::pattern_type_mismatch,
    reason = "the environment iterator destructures borrowed configured pairs without copying secrets"
)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "adapter plan inspectors follow the same dispatch framing as the immutable plan"
)]
impl AdapterExecutionPlan {
    /// Returns the exact durable identity prepared for this execution.
    #[must_use]
    #[inline]
    pub const fn prepared_identity(&self) -> &PreparedProcessIdentity {
        &self.identity
    }

    /// Returns the trusted absolute executable path.
    #[must_use]
    #[inline]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Returns literal direct argv in order.
    #[must_use]
    #[inline]
    pub fn argv(&self) -> impl ExactSizeIterator<Item = &str> {
        self.argv.iter().map(String::as_str)
    }

    /// Returns the repository-relative working directory.
    #[must_use]
    #[inline]
    pub fn repository_relative_cwd(&self) -> &Path {
        &self.cwd
    }

    /// Returns the exact fixed environment.
    #[must_use]
    #[inline]
    pub fn fixed_environment(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.environment
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Reports the closed network policy.
    #[must_use]
    #[inline]
    pub const fn network_is_denied(&self) -> bool {
        true
    }

    /// Returns the execution deadline.
    #[must_use]
    #[inline]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns the stdout bound.
    #[must_use]
    #[inline]
    pub const fn stdout_limit_bytes(&self) -> usize {
        self.stdout_bytes
    }

    /// Returns the stderr bound.
    #[must_use]
    #[inline]
    pub const fn stderr_limit_bytes(&self) -> usize {
        self.stderr_bytes
    }

    /// Returns the workflow provenance bound to this authorization.
    #[must_use]
    #[inline]
    pub const fn provenance(&self) -> &AssignmentWorkflowProvenance {
        &self.provenance
    }
}

#[derive(Debug)]
/// Private adapter preserving a static checked-command rejection.
struct ProcessCommandError(&'static str);

impl fmt::Display for ProcessCommandError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the private modeled error has no nested source"
)]
impl Error for ProcessCommandError {}

/// Folds the common exact Requested/Prepared/terminal lifecycle.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the lifecycle fold intentionally matches borrowed closed facts without cloning durable payloads"
)]
fn fold_terminal_state(
    state: Modeled<CompleteProcessState>,
    event: &ProcessEvent,
    stream: &ProcessStream,
) -> Modeled<CompleteProcessState> {
    let mut folded = state.into_inner();
    let view = ProcessEventView::from_event(event);
    if view.as_ref().stream != *stream.as_stream_id() {
        folded.malformed = true;
        return Modeled::from_built(folded);
    }
    match &view.as_ref().fact {
        ProcessFact::Requested(request)
            if folded.requested.is_none() && folded.prepared.is_none() && !folded.terminal =>
        {
            folded.requested = Some(request.clone());
        }
        ProcessFact::Prepared(identity)
            if folded.requested.as_ref() == Some(identity.request())
                && folded.prepared.is_none()
                && !folded.terminal =>
        {
            folded.prepared = Some(identity.clone());
        }
        ProcessFact::Completed(receipt)
            if folded.prepared.as_ref() == Some(receipt.identity()) && !folded.terminal =>
        {
            folded.terminal = true;
        }
        ProcessFact::SpawnFailed(failure)
            if folded.prepared.as_ref() == Some(failure.identity()) && !folded.terminal =>
        {
            folded.terminal = true;
        }
        ProcessFact::TimedOut(timed_out)
            if folded.prepared.as_ref() == Some(timed_out.identity()) && !folded.terminal =>
        {
            folded.terminal = true;
        }
        ProcessFact::Cancelled(cancelled)
            if folded.prepared.as_ref() == Some(cancelled.identity()) && !folded.terminal =>
        {
            folded.terminal = true;
        }
        ProcessFact::Unknown(unknown)
            if folded.prepared.as_ref() == Some(unknown.identity()) && !folded.terminal =>
        {
            folded.terminal = true;
        }
        ProcessFact::Requested(_)
        | ProcessFact::Refused { .. }
        | ProcessFact::Prepared(_)
        | ProcessFact::Completed(_)
        | ProcessFact::SpawnFailed(_)
        | ProcessFact::TimedOut(_)
        | ProcessFact::Cancelled(_)
        | ProcessFact::Unknown(_)
        | ProcessFact::Reconciled(_) => folded.malformed = true,
    }
    Modeled::from_built(folded)
}

/// Folds the exact Requested/Prepared/Unknown/Reconciled lifecycle.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the reconciliation fold matches borrowed closed facts without cloning payloads unnecessarily"
)]
fn fold_reconciliation_state(
    state: Modeled<ReconcileProcessState>,
    event: &ProcessEvent,
    stream: &ProcessStream,
) -> Modeled<ReconcileProcessState> {
    let mut folded = state.into_inner();
    let view = ProcessEventView::from_event(event);
    if view.as_ref().stream != *stream.as_stream_id() {
        folded.malformed = true;
        return Modeled::from_built(folded);
    }
    match &view.as_ref().fact {
        ProcessFact::Requested(request)
            if folded.requested.is_none()
                && folded.prepared.is_none()
                && !folded.unknown
                && !folded.reconciled =>
        {
            folded.requested = Some(request.clone());
        }
        ProcessFact::Prepared(identity)
            if folded.requested.as_ref() == Some(identity.request())
                && folded.prepared.is_none()
                && !folded.unknown
                && !folded.reconciled =>
        {
            folded.prepared = Some(identity.clone());
        }
        ProcessFact::Unknown(unknown)
            if folded.prepared.as_ref() == Some(unknown.identity())
                && !folded.unknown
                && !folded.reconciled =>
        {
            folded.unknown = true;
        }
        ProcessFact::Reconciled(reconciled)
            if folded.prepared.as_ref() == Some(reconciled.identity())
                && folded.unknown
                && !folded.reconciled
                && reconciliation_outcome_matches(reconciled) =>
        {
            folded.reconciled = true;
        }
        ProcessFact::Requested(_)
        | ProcessFact::Refused { .. }
        | ProcessFact::Prepared(_)
        | ProcessFact::Completed(_)
        | ProcessFact::SpawnFailed(_)
        | ProcessFact::TimedOut(_)
        | ProcessFact::Cancelled(_)
        | ProcessFact::Unknown(_)
        | ProcessFact::Reconciled(_) => folded.malformed = true,
    }
    Modeled::from_built(folded)
}

/// Checks whether one invocation can enter the durable per-effect recovery set.
///
/// Existing terminal, refused, and nonterminal invocation streams all consume
/// the same bounded recovery catalog. Retrying an already-admitted stream is
/// allowed at the bound; a distinct stream is not.
///
/// # Errors
///
/// Returns [`ProcessServiceError::StreamRequestMismatch`] when `candidate` is
/// not owned by `effect_id`, and
/// [`ProcessServiceError::InvocationLimitReached`] when verified history is
/// already over the bound or a distinct candidate would exceed it.
#[inline]
pub fn admit_process_invocation(
    effect_id: &EffectId,
    verified_stream_ids: &[StreamId],
    candidate: &ProcessStream,
) -> Result<(), ProcessServiceError> {
    if ProcessStream::from_verified_effect_stream(effect_id, candidate.as_stream_id()).as_ref()
        != Some(candidate)
    {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    let mut admitted: usize = 0;
    let mut candidate_admitted = false;
    for stream_id in verified_stream_ids {
        let Some(stream) = ProcessStream::from_verified_effect_stream(effect_id, stream_id) else {
            continue;
        };
        admitted = admitted.saturating_add(1);
        candidate_admitted |= stream == *candidate;
        if admitted > MAX_PROCESS_INVOCATION_STREAMS {
            return Err(ProcessServiceError::InvocationLimitReached);
        }
    }
    if candidate_admitted || admitted < MAX_PROCESS_INVOCATION_STREAMS {
        Ok(())
    } else {
        Err(ProcessServiceError::InvocationLimitReached)
    }
}

/// Decides the durable requested/prepared or refused publication for one semantic intent.
///
/// # Errors
///
/// Returns a stable service failure for conflicting, malformed, or cross-stream history.
#[inline]
pub fn decide_process_request(
    history: &[ProcessEvent],
    stream: ProcessStream,
    request: ProcessRequest,
    catalog: &ConfiguredCommandCatalog,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(&request)? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    if !history.is_empty() {
        return reconcile_existing(history, stream, &request);
    }

    let catalog_entry_decision = catalog
        .resolve(request.command_id())
        .ok()
        .map(catalog_entry_identity);
    let request_input = RequestProcessInput::model_builder()
        .stream(stream.clone())
        .request(request.clone())
        .catalog_entry(catalog_entry_decision.clone())
        .build();
    let request_command = RequestProcess::model_builder()
        .stream(RequestInputToStream::apply(request_input.as_ref()))
        .request(RequestInputToRequest::apply(request_input.as_ref()))
        .catalog_entry(RequestInputToCatalogEntry::apply(request_input.as_ref()))
        .build();
    let requested = exactly_one(CommandLogic::handle(&request_command, Modeled::default()))?;
    let Some(catalog_entry) = catalog_entry_decision else {
        return Ok(ProcessPublication {
            events: vec![requested],
            stream,
        });
    };

    let prepare_input = PrepareProcessInput::model_builder()
        .catalog_entry(catalog_entry)
        .stream(stream.clone())
        .request(request)
        .build();
    let prepare_command = PrepareProcess::model_builder()
        .catalog_entry(PrepareInputToCatalogEntry::apply(prepare_input.as_ref()))
        .stream(PrepareInputToStream::apply(prepare_input.as_ref()))
        .request(PrepareInputToRequest::apply(prepare_input.as_ref()))
        .build();
    let state = ModelCommandLogic::evolve(prepare_command.as_ref(), Modeled::default(), &requested);
    let prepared = exactly_one(CommandLogic::handle(&prepare_command, state))?;
    Ok(ProcessPublication {
        events: vec![requested, prepared],
        stream,
    })
}

/// Records definitive completion for the exact prepared process.
///
/// # Errors
///
/// Returns a stable failure when preparation is missing or mismatched, retained
/// history is malformed, or another terminal outcome already exists.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the one terminal receipt even though modeled provenance requires internal clones"
)]
pub fn decide_record_completed(
    history: &[ProcessEvent],
    stream: ProcessStream,
    receipt: ProcessReceipt,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(receipt.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(history.last(), Some(event) if event.fact() == &ProcessFact::Completed(receipt.clone()));
    let input = CompleteProcessInput::model_builder()
        .stream(stream.clone())
        .receipt(receipt.clone())
        .build();
    let command = CompleteProcess::model_builder()
        .stream(CompleteInputToStream::apply(input.as_ref()))
        .receipt(CompleteInputToReceipt::apply(input.as_ref()))
        .build();
    let mut state: Modeled<CompleteProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().terminal
            || state.as_ref().requested.as_ref() != Some(receipt.identity().request())
            || state.as_ref().prepared.as_ref() != Some(receipt.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let completed = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![completed],
        stream,
    })
}

/// Records a definitive content-free spawn failure for the exact prepared process.
///
/// # Errors
///
/// Returns a stable failure when preparation is missing or mismatched, retained
/// history is malformed, or another terminal outcome already exists.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the one terminal failure even though modeled provenance requires internal clones"
)]
pub fn decide_record_spawn_failed(
    history: &[ProcessEvent],
    stream: ProcessStream,
    failure: ProcessSpawnFailure,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(failure.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(
        history.last(),
        Some(event) if event.fact() == &ProcessFact::SpawnFailed(failure.clone())
    );
    let input = FailProcessSpawnInput::model_builder()
        .stream(stream.clone())
        .failure(failure.clone())
        .build();
    let command = FailProcessSpawn::model_builder()
        .stream(FailSpawnInputToStream::apply(input.as_ref()))
        .failure(FailSpawnInputToFailure::apply(input.as_ref()))
        .build();
    let mut state: Modeled<CompleteProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().terminal
            || state.as_ref().requested.as_ref() != Some(failure.identity().request())
            || state.as_ref().prepared.as_ref() != Some(failure.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let failed = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![failed],
        stream,
    })
}

/// Records a timeout terminal for the exact prepared process.
///
/// # Errors
///
/// Returns a stable failure when preparation is missing or mismatched, retained
/// history is malformed, or another terminal outcome already exists.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the one timeout terminal even though modeled provenance requires internal clones"
)]
pub fn decide_record_timed_out(
    history: &[ProcessEvent],
    stream: ProcessStream,
    timed_out: ProcessTimedOut,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(timed_out.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(
        history.last(),
        Some(event) if event.fact() == &ProcessFact::TimedOut(timed_out.clone())
    );
    let input = TimeOutProcessInput::model_builder()
        .stream(stream.clone())
        .timed_out(timed_out.clone())
        .build();
    let command = TimeOutProcess::model_builder()
        .stream(TimeOutInputToStream::apply(input.as_ref()))
        .timed_out(TimeOutInputToTerminal::apply(input.as_ref()))
        .build();
    let mut state: Modeled<CompleteProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().terminal
            || state.as_ref().requested.as_ref() != Some(timed_out.identity().request())
            || state.as_ref().prepared.as_ref() != Some(timed_out.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let event = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![event],
        stream,
    })
}

/// Records cancellation for the exact prepared process.
///
/// # Errors
///
/// Returns a stable failure when preparation is missing or mismatched, retained
/// history is malformed, or another terminal outcome already exists.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the one cancellation terminal even though modeled provenance requires internal clones"
)]
pub fn decide_record_cancelled(
    history: &[ProcessEvent],
    stream: ProcessStream,
    cancelled: ProcessCancelled,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(cancelled.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(
        history.last(),
        Some(event) if event.fact() == &ProcessFact::Cancelled(cancelled.clone())
    );
    let input = CancelProcessInput::model_builder()
        .stream(stream.clone())
        .cancelled(cancelled.clone())
        .build();
    let command = CancelProcess::model_builder()
        .stream(CancelInputToStream::apply(input.as_ref()))
        .cancelled(CancelInputToTerminal::apply(input.as_ref()))
        .build();
    let mut state: Modeled<CompleteProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().terminal
            || state.as_ref().requested.as_ref() != Some(cancelled.identity().request())
            || state.as_ref().prepared.as_ref() != Some(cancelled.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let event = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![event],
        stream,
    })
}

/// Records content-free uncertainty for the exact prepared process.
///
/// # Errors
///
/// Returns a stable failure when preparation is missing or mismatched, retained
/// history is malformed, or any terminal outcome already exists.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the terminal even though modeled provenance requires internal clones"
)]
pub fn decide_record_unknown(
    history: &[ProcessEvent],
    stream: ProcessStream,
    unknown: ProcessUnknown,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(unknown.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(
        history.last(),
        Some(event) if event.fact() == &ProcessFact::Unknown(unknown.clone())
    );
    let input = MarkProcessUnknownInput::model_builder()
        .stream(stream.clone())
        .unknown(unknown.clone())
        .build();
    let command = MarkProcessUnknown::model_builder()
        .stream(MarkUnknownInputToStream::apply(input.as_ref()))
        .unknown(MarkUnknownInputToTerminal::apply(input.as_ref()))
        .build();
    let mut state: Modeled<CompleteProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().terminal
            || state.as_ref().requested.as_ref() != Some(unknown.identity().request())
            || state.as_ref().prepared.as_ref() != Some(unknown.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let event = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![event],
        stream,
    })
}

/// Validates and classifies one complete signed process lifecycle for restart.
///
/// The caller is responsible for supplying history already verified by its
/// signed event-store read boundary. Every accepted state is bound to the
/// exact stream, request, preparation, and terminal or reconciliation identity.
///
/// # Errors
///
/// Returns [`ProcessServiceError::InvalidHistory`] for incomplete, malformed,
/// conflicting, cross-stream, or identity-mismatched lifecycle facts.
#[inline]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "restart classification borrows closed durable facts while returning only the exact recovery capability or projection"
)]
pub fn classify_process_restart(
    history: &[ProcessEvent],
    stream: &ProcessStream,
) -> Result<ProcessRestartState, ProcessServiceError> {
    validate_stream_history(history, stream)?;
    if let [refused] = history {
        let ProcessFact::Refused { identity, .. } = refused.fact() else {
            return Err(ProcessServiceError::InvalidHistory);
        };
        return identity
            .matches_stream(stream)
            .then_some(ProcessRestartState::Closed)
            .ok_or(ProcessServiceError::InvalidHistory);
    }

    let [requested, prepared, tail @ ..] = history else {
        return Err(ProcessServiceError::InvalidHistory);
    };
    let ProcessFact::Requested(request) = requested.fact() else {
        return Err(ProcessServiceError::InvalidHistory);
    };
    if ProcessStream::for_request(request).as_ref() != Ok(stream) {
        return Err(ProcessServiceError::InvalidHistory);
    }
    let ProcessFact::Prepared(identity) = prepared.fact() else {
        return Err(ProcessServiceError::InvalidHistory);
    };
    if identity.request() != request {
        return Err(ProcessServiceError::InvalidHistory);
    }

    match tail {
        [] => Ok(ProcessRestartState::Prepared(identity.clone())),
        [terminal]
            if matches!(
                terminal.fact(),
                ProcessFact::Completed(receipt) if receipt.identity() == identity
            ) || matches!(
                terminal.fact(),
                ProcessFact::SpawnFailed(failure) if failure.identity() == identity
            ) || matches!(
                terminal.fact(),
                ProcessFact::TimedOut(timed_out) if timed_out.identity() == identity
            ) || matches!(
                terminal.fact(),
                ProcessFact::Cancelled(cancelled) if cancelled.identity() == identity
            ) =>
        {
            Ok(ProcessRestartState::Closed)
        }
        [unknown] if matches!(unknown.fact(), ProcessFact::Unknown(recorded) if recorded.identity() == identity) => {
            Ok(ProcessRestartState::Unknown(
                ProcessReconciliationCapability {
                    identity: identity.clone(),
                },
            ))
        }
        [unknown, reconciled]
            if matches!(unknown.fact(), ProcessFact::Unknown(recorded) if recorded.identity() == identity)
                && matches!(reconciled.fact(), ProcessFact::Reconciled(result)
                    if result.identity() == identity && reconciliation_outcome_matches(result)) =>
        {
            let ProcessFact::Reconciled(result) = reconciled.fact() else {
                return Err(ProcessServiceError::InvalidHistory);
            };
            Ok(ProcessRestartState::Reconciled(result.outcome().clone()))
        }
        _ => Err(ProcessServiceError::InvalidHistory),
    }
}

/// Mints exact private-artifact retirement authority only from a complete
/// signed lifecycle that is already definitively terminal or reconciled.
///
/// Refusal-only, prepared, and unreconciled unknown histories mint no
/// retirement authority. The caller is responsible for supplying history
/// already verified by its signed event-store read boundary.
///
/// # Errors
///
/// Returns [`ProcessServiceError::InvalidHistory`] for any lifecycle rejected
/// by the canonical full-history restart classifier.
#[inline]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "retirement authority borrows the validated Prepared identity after canonical full-history classification"
)]
pub fn authorize_process_retirement(
    history: &[ProcessEvent],
    stream: &ProcessStream,
) -> Result<Option<ProcessRetirementCapability>, ProcessServiceError> {
    let retireable = matches!(
        classify_process_restart(history, stream)?,
        ProcessRestartState::Closed | ProcessRestartState::Reconciled(_)
    );
    if !retireable {
        return Ok(None);
    }
    let Some(ProcessFact::Prepared(identity)) = history.get(1).map(ProcessEvent::fact) else {
        return Ok(None);
    };
    Ok(Some(ProcessRetirementCapability {
        identity: identity.clone(),
    }))
}

/// Folds exact verified history into read-only reconciliation authority.
///
/// The caller is responsible for supplying history already verified by its
/// signed event-store read boundary. A completed reconciliation returns no
/// capability, so restart cannot repeat inspection or mint process authority.
///
/// # Errors
///
/// Returns a stable failure for missing, malformed, duplicate, conflicting, or
/// cross-stream lifecycle facts.
#[inline]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the compatibility error mapping borrows the retained Requested fact before delegating full lifecycle validation"
)]
pub fn recover_process_reconciliation(
    history: &[ProcessEvent],
    stream: &ProcessStream,
) -> Result<Option<ProcessReconciliationCapability>, ProcessServiceError> {
    if let Some(ProcessFact::Requested(request)) = history.first().map(ProcessEvent::fact)
        && ProcessStream::for_request(request).as_ref() != Ok(stream)
    {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    match classify_process_restart(history, stream)? {
        ProcessRestartState::Unknown(capability) => Ok(Some(capability)),
        ProcessRestartState::Reconciled(_) => Ok(None),
        ProcessRestartState::Prepared(_) | ProcessRestartState::Closed => {
            Err(ProcessServiceError::InvalidHistory)
        }
    }
}

/// Records one closed result produced by read-only reconciliation.
///
/// # Errors
///
/// Returns a stable failure for a mismatched result, malformed lifecycle, or
/// conflicting/duplicate retained reconciliation.
#[inline]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the public command boundary consumes the one-shot reconciliation result"
)]
pub fn decide_record_reconciled(
    history: &[ProcessEvent],
    stream: ProcessStream,
    reconciled: ProcessReconciled,
) -> Result<ProcessPublication, ProcessServiceError> {
    if stream != ProcessStream::for_request(reconciled.identity().request())? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, &stream)?;
    let exact_duplicate = matches!(
        history.last(),
        Some(event) if event.fact() == &ProcessFact::Reconciled(reconciled.clone())
    );
    let input = ReconcileProcessInput::model_builder()
        .stream(stream.clone())
        .reconciled(reconciled.clone())
        .build();
    let command = ReconcileProcess::model_builder()
        .stream(ReconcileInputToStream::apply(input.as_ref()))
        .reconciled(ReconcileInputToResult::apply(input.as_ref()))
        .build();
    let mut state: Modeled<ReconcileProcessState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if exact_duplicate {
        if state.as_ref().malformed
            || !state.as_ref().reconciled
            || state.as_ref().requested.as_ref() != Some(reconciled.identity().request())
            || state.as_ref().prepared.as_ref() != Some(reconciled.identity())
        {
            return Err(ProcessServiceError::InvalidHistory);
        }
        return Ok(ProcessPublication {
            events: Vec::new(),
            stream,
        });
    }
    let event = exactly_one(CommandLogic::handle(&command, state))?;
    Ok(ProcessPublication {
        events: vec![event],
        stream,
    })
}

/// Mints non-cloneable adapter authority only from matching requested/prepared history.
///
/// The caller is responsible for supplying history already verified by its signed
/// event-store read boundary.
///
/// # Errors
///
/// Returns a stable failure when history, identity, stream, or current trusted
/// configuration does not exactly match.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "authorization matches borrowed durable facts without cloning history"
)]
#[inline]
pub fn authorize_prepared_process(
    history: &[ProcessEvent],
    stream: &ProcessStream,
    request: &ProcessRequest,
    catalog: &ConfiguredCommandCatalog,
) -> Result<AuthorizedProcess, ProcessServiceError> {
    if stream != &ProcessStream::for_request(request)? {
        return Err(ProcessServiceError::StreamRequestMismatch);
    }
    validate_stream_history(history, stream)?;
    let prepared_identity = match history {
        [requested, prepared]
            if matches!(requested.fact(), ProcessFact::Requested(recorded) if recorded == request)
                && matches!(prepared.fact(), ProcessFact::Prepared(identity) if identity.request() == request) =>
        {
            let ProcessFact::Prepared(identity) = prepared.fact() else {
                return Err(ProcessServiceError::InvalidHistory);
            };
            identity
        }
        [event] if matches!(event.fact(), ProcessFact::Refused { identity, .. } if identity.matches(request)) =>
        {
            return Err(ProcessServiceError::RequestRefused);
        }
        [] => return Err(ProcessServiceError::PreparedHistoryRequired),
        _ => return Err(ProcessServiceError::InvalidHistory),
    };
    let command = catalog
        .resolve(request.command_id())
        .map_err(|_source| ProcessServiceError::CatalogChanged)?;
    if catalog_entry_identity(command) != prepared_identity.catalog_entry {
        return Err(ProcessServiceError::CatalogChanged);
    }
    Ok(AuthorizedProcess {
        plan: AdapterExecutionPlan {
            identity: prepared_identity.clone(),
            program: command.program().to_path_buf(),
            argv: command.argv().map(str::to_owned).collect(),
            cwd: command.repository_relative_cwd().to_path_buf(),
            environment: command
                .fixed_environment()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            timeout: command.timeout(),
            stdout_bytes: command.stdout_limit_bytes(),
            stderr_bytes: command.stderr_limit_bytes(),
            provenance: request.provenance().clone(),
        },
    })
}

/// Reconciles an identical terminal policy decision without a duplicate publication.
#[expect(
    clippy::single_call_fn,
    reason = "named reconciliation isolates retained-history validation from first-admission command construction"
)]
#[inline]
fn reconcile_existing(
    history: &[ProcessEvent],
    stream: ProcessStream,
    request: &ProcessRequest,
) -> Result<ProcessPublication, ProcessServiceError> {
    validate_retained_process_request(history, &stream, request)?;
    Ok(ProcessPublication {
        events: Vec::new(),
        stream,
    })
}

/// Validates that retained lifecycle facts belong to this exact semantic request.
///
/// This boundary admits an exact retry after either preparation or a terminal
/// receipt while rejecting a different request that happens to share the same
/// effect-owned stream.
///
/// # Errors
///
/// Returns [`ProcessServiceError::InvalidHistory`] unless the history is one
/// complete, structurally valid lifecycle for `request`.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "retained lifecycle validation intentionally destructures a borrowed event slice"
)]
#[inline]
pub fn validate_retained_process_request(
    history: &[ProcessEvent],
    stream: &ProcessStream,
    request: &ProcessRequest,
) -> Result<(), ProcessServiceError> {
    classify_process_restart(history, stream)?;
    let exact = match history {
        [refused] if matches!(refused.fact(), ProcessFact::Refused { identity, .. } if identity.matches(request)) => {
            true
        }
        [requested, ..] if matches!(requested.fact(), ProcessFact::Requested(retained) if retained == request) => {
            true
        }
        _ => false,
    };
    exact
        .then_some(())
        .ok_or(ProcessServiceError::InvalidHistory)
}

/// Verifies that every supplied fact belongs to the exact effect stream.
fn validate_stream_history(
    history: &[ProcessEvent],
    stream: &ProcessStream,
) -> Result<(), ProcessServiceError> {
    if history
        .iter()
        .all(|event| event.stream_id() == stream.as_stream_id())
    {
        Ok(())
    } else {
        Err(ProcessServiceError::InvalidHistory)
    }
}

/// Extracts the single event required from a checked command decision.
fn exactly_one(
    result: Result<eventcore::NewEvents<ProcessEvent>, CommandError>,
) -> Result<ProcessEvent, ProcessServiceError> {
    let events: Vec<ProcessEvent> = result
        .map_err(|_source| ProcessServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| ProcessServiceError::InvalidModeledEmission)?;
    Ok(event)
}

/// Clones the exact stream identity consumed by generated command mappings.
fn stream_id(stream: &ProcessStream) -> StreamId {
    stream.as_stream_id().clone()
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects the sanitized requested or refused fact from trusted catalog membership.
fn request_fact(
    request: &ProcessRequest,
    catalog_entry: &Option<CatalogEntryIdentity>,
) -> ProcessFact {
    if catalog_entry.is_some() {
        ProcessFact::Requested(request.clone())
    } else {
        ProcessFact::Refused {
            identity: RefusedProcessIdentity::from_request(request),
            code: ProcessRefusal::UnknownConfiguredCommand,
        }
    }
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects preparation only for the exact durable admitted request.
fn prepared_fact(
    request: &ProcessRequest,
    catalog_entry: &CatalogEntryIdentity,
    durable_request: &Option<ProcessRequest>,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed || durable_request.as_ref() != Some(request) {
        return Err(command_error("process_request_history_invalid"));
    }
    Ok(ProcessFact::Prepared(PreparedProcessIdentity {
        catalog_entry: catalog_entry.clone(),
        request: request.clone(),
    }))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects definitive completion only for the exact prepared identity.
fn completed_fact(
    receipt: &ProcessReceipt,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || *terminal
        || requested.as_ref() != Some(receipt.identity().request())
        || prepared.as_ref() != Some(receipt.identity())
    {
        return Err(command_error("process_completion_history_invalid"));
    }
    Ok(ProcessFact::Completed(receipt.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects spawn failure only for the exact prepared identity.
fn spawn_failed_fact(
    failure: &ProcessSpawnFailure,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || *terminal
        || requested.as_ref() != Some(failure.identity().request())
        || prepared.as_ref() != Some(failure.identity())
    {
        return Err(command_error("process_spawn_failure_history_invalid"));
    }
    Ok(ProcessFact::SpawnFailed(failure.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects timeout only for the exact prepared identity.
fn timed_out_fact(
    timed_out: &ProcessTimedOut,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || *terminal
        || requested.as_ref() != Some(timed_out.identity().request())
        || prepared.as_ref() != Some(timed_out.identity())
    {
        return Err(command_error("process_timeout_history_invalid"));
    }
    Ok(ProcessFact::TimedOut(timed_out.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects cancellation only for the exact prepared identity.
fn cancelled_fact(
    cancelled: &ProcessCancelled,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || *terminal
        || requested.as_ref() != Some(cancelled.identity().request())
        || prepared.as_ref() != Some(cancelled.identity())
    {
        return Err(command_error("process_cancellation_history_invalid"));
    }
    Ok(ProcessFact::Cancelled(cancelled.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects uncertainty only for the exact prepared identity before any terminal.
fn unknown_fact(
    unknown: &ProcessUnknown,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || *terminal
        || requested.as_ref() != Some(unknown.identity().request())
        || prepared.as_ref() != Some(unknown.identity())
    {
        return Err(command_error("process_unknown_history_invalid"));
    }
    Ok(ProcessFact::Unknown(unknown.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs"
)]
/// Selects one exact closed result after uncertainty and never process authority.
fn reconciled_fact(
    result: &ProcessReconciled,
    requested: &Option<ProcessRequest>,
    prepared: &Option<PreparedProcessIdentity>,
    unknown: &bool,
    reconciled: &bool,
    malformed: &bool,
) -> Result<ProcessFact, CommandError> {
    if *malformed
        || !*unknown
        || *reconciled
        || requested.as_ref() != Some(result.identity().request())
        || prepared.as_ref() != Some(result.identity())
        || !reconciliation_outcome_matches(result)
    {
        return Err(command_error("process_reconciliation_history_invalid"));
    }
    Ok(ProcessFact::Reconciled(result.clone()))
}

/// Checks that any completed outcome belongs to the exact reconciled process.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the closed outcome check matches a borrowed durable result without cloning it"
)]
fn reconciliation_outcome_matches(result: &ProcessReconciled) -> bool {
    match result.outcome() {
        ProcessReconciliationOutcome::Completed(receipt) => receipt.identity() == result.identity(),
        ProcessReconciliationOutcome::DefinitelyNotCompleted
        | ProcessReconciliationOutcome::StillUnknown => true,
    }
}

/// Derives a content-free identity from every execution-relevant catalog field.
#[expect(
    clippy::big_endian_bytes,
    reason = "the catalog identity requires a canonical cross-platform byte order"
)]
fn catalog_entry_identity(command: &ConfiguredCommand) -> CatalogEntryIdentity {
    let mut digest = Sha256::new();
    hash_bytes(&mut digest, b"program");
    hash_bytes(
        &mut digest,
        command.program().as_os_str().as_encoded_bytes(),
    );
    hash_bytes(&mut digest, b"argv");
    hash_count(&mut digest, command.argv().len());
    for argument in command.argv() {
        hash_bytes(&mut digest, argument.as_bytes());
    }
    hash_bytes(&mut digest, b"cwd");
    hash_bytes(
        &mut digest,
        command
            .repository_relative_cwd()
            .as_os_str()
            .as_encoded_bytes(),
    );
    hash_bytes(&mut digest, b"environment");
    hash_count(&mut digest, command.fixed_environment().len());
    for (key, value) in command.fixed_environment() {
        hash_bytes(&mut digest, b"environment-key");
        hash_bytes(&mut digest, key.as_bytes());
        hash_bytes(&mut digest, b"environment-value");
        hash_bytes(&mut digest, value.as_bytes());
    }
    hash_bytes(&mut digest, b"timeout-nanoseconds");
    digest.update(command.timeout().as_nanos().to_be_bytes());
    hash_bytes(&mut digest, b"stdout-limit-bytes");
    hash_count(&mut digest, command.stdout_limit_bytes());
    hash_bytes(&mut digest, b"stderr-limit-bytes");
    hash_count(&mut digest, command.stderr_limit_bytes());
    CatalogEntryIdentity {
        digest: digest.finalize().into(),
        stdout_limit_bytes: command.stdout_limit_bytes(),
        stderr_limit_bytes: command.stderr_limit_bytes(),
    }
}

/// Adds one length-delimited field to a catalog-entry identity.
fn hash_bytes(digest: &mut Sha256, bytes: &[u8]) {
    hash_count(digest, bytes.len());
    digest.update(bytes);
}

/// Adds one delimiter-framed canonical decimal count to a catalog-entry identity.
fn hash_count(digest: &mut Sha256, count: usize) {
    let decimal = count.to_string();
    digest.update(decimal.as_bytes());
    digest.update([0]);
}

/// Builds a typed checked-command business-rule rejection.
#[inline]
fn command_error(message: &'static str) -> CommandError {
    CommandError::BusinessRuleViolation(Box::new(ProcessCommandError(message)))
}
