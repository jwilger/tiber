//! Native `EventCore` authority for Tiber's durable workflow trampoline.
//!
//! Each public factory below is a closed, command-specific write boundary. The
//! service persists a shell observation in its own transaction before a later
//! command asks the pure core to advance the trampoline.

use core::{error::Error, fmt};

use eventcore::{
    CommandError, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput, ModelState, StreamId,
    mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity as _},
};
use serde::{Deserialize, Serialize};
use tiber_workflow_core::{
    EffectId, EffectObservation, EffectReceiptId, HarnessError, HarnessPhase, HarnessState,
    TiberEffect, TrampolineStep, step,
};

/// Errors raised while converting between the semantic workflow stream and an
/// external stream identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkflowServiceError {
    /// The encoded stream did not have the workflow-session form.
    InvalidStream,
}

impl WorkflowServiceError {
    /// Returns the stable external failure code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the single stable-code match is clearer as the function result"
    )]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidStream => "workflow_invalid_stream",
        }
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::renamed_function_params,
    reason = "the standard display implementation uses a descriptive formatter name"
)]
impl fmt::Display for WorkflowServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "semantic stream errors have no causal source"
)]
impl Error for WorkflowServiceError {}

/// Semantic stream identity for one durable Tiber session workflow.
#[derive(Clone, Debug, Eq, PartialEq, eventcore::StreamIdentity)]
pub struct WorkflowStream(StreamId);

impl WorkflowStream {
    /// Creates the durable workflow stream associated with one session.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowServiceError::InvalidStream`] if `EventCore` rejects the
    /// encoded stream identifier.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic stream constructor directly returns EventCore's parsed result"
    )]
    pub fn for_session(
        session: &tiber_workflow_core::SessionId,
    ) -> Result<Self, WorkflowServiceError> {
        StreamId::try_new(format!("tiber:workflow:{}", session.as_str()))
            .map(Self)
            .map_err(|_source| WorkflowServiceError::InvalidStream)
    }

    /// Recovers the session identity guaranteed by construction.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowServiceError::InvalidStream`] when this stream does
    /// not encode a valid native workflow session.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic stream parser directly returns the typed session result"
    )]
    pub fn session(&self) -> Result<tiber_workflow_core::SessionId, WorkflowServiceError> {
        let raw = self.0.as_ref();
        let Some(session) = raw.strip_prefix("tiber:workflow:") else {
            return Err(WorkflowServiceError::InvalidStream);
        };
        tiber_workflow_core::SessionId::parse(session)
            .map_err(|_source| WorkflowServiceError::InvalidStream)
    }
}

/// Immutable workflow facts emitted by the closed native command surface.
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::large_enum_variant,
    reason = "fact variants follow durable lifecycle order and retain complete serializable checkpoints"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum WorkflowFact {
    /// The immutable initial continuation state was accepted for this stream.
    WorkflowInitialized {
        /// Pure state from which the trampoline begins.
        state: HarnessState,
    },
    /// The core requested exactly one typed effect for the shell to execute.
    EffectRequested {
        /// Checkpoint to resume only after an observation is durable.
        state: HarnessState,
        /// The closed effect envelope carrying complete provenance.
        effect: TiberEffect,
    },
    /// The shell durably recorded its result for one requested effect.
    EffectObserved {
        /// The result that a later request command may feed to the pure core.
        observation: EffectObservation,
    },
    /// The workflow reached a successful terminal state.
    WorkflowCompleted {
        /// Terminal pure state.
        state: HarnessState,
        /// Receipt for the successfully observed effect.
        receipt: EffectReceiptId,
    },
    /// The workflow reached a typed stopped terminal state.
    WorkflowStopped {
        /// Terminal pure state.
        state: HarnessState,
        /// Typed reason the core declined to continue.
        error: HarnessError,
    },
}

/// Durable `EventCore` event for one native workflow stream.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "stream then fact is the durable-envelope order"
)]
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct WorkflowEvent {
    /// Owning workflow stream.
    stream: StreamId,
    /// Immutable business fact.
    fact: WorkflowFact,
}

#[expect(
    clippy::implicit_return,
    reason = "EventCore's Event contract fixes method names while the durable stream accessor is primary"
)]
impl Event for WorkflowEvent {
    fn event_type_name() -> &'static str {
        "TiberWorkflowEvent"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-side projection that consumes every persisted workflow-event field.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the checked-model-only projection keeps stream then fact in durable-envelope order"
)]
#[derive(ModelOutput)]
struct WorkflowEventView {
    /// Projected stream identity.
    stream: StreamId,
    /// Projected immutable fact.
    fact: WorkflowFact,
}

mapping! { WorkflowEventStreamToView: WorkflowEvent.stream => WorkflowEventView.stream using clone; }
mapping! { WorkflowEventFactToView: WorkflowEvent.fact => WorkflowEventView.fact using clone; }

impl WorkflowEventView {
    /// Returns the projected immutable fact.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the checked projection directly builds its complete modeled output"
    )]
    fn fact(&self) -> &WorkflowFact {
        &self.fact
    }

    /// Builds the checked query projection for one persisted workflow fact.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the projection's stream field is consumed as a direct reference"
    )]
    fn from_event(event: &WorkflowEvent) -> Self {
        Self::model_builder()
            .stream(WorkflowEventStreamToView::apply(event))
            .fact(WorkflowEventFactToView::apply(event))
            .build()
            .into_inner()
    }

    /// Returns the projected stream identity.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the projection's fact field is consumed as a direct reference"
    )]
    fn stream(&self) -> &StreamId {
        &self.stream
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore input derives require adjacent origin fields in command data-flow order"
)]
#[derive(ModelInput)]
/// Origin values accepted by initialization.
struct InitializeWorkflowRequest {
    /// Semantic stream to initialize.
    #[model(origin)]
    stream: WorkflowStream,
    /// Ready continuation accepted by the stream.
    #[model(origin)]
    state: HarnessState,
}

/// Initializes one workflow stream exactly once.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore command derives retain stream then initial-state boundary values"
)]
#[derive(ModelCommand)]
struct InitializeWorkflow {
    /// Declared command stream.
    #[stream]
    stream: WorkflowStream,
    /// Candidate initial continuation.
    state: HarnessState,
}

mapping! { InitializeWorkflowRequestToStream: InitializeWorkflowRequest.stream => InitializeWorkflow.stream using clone; }
mapping! { InitializeWorkflowRequestToState: InitializeWorkflowRequest.state => InitializeWorkflow.state using clone; }

#[derive(ModelState)]
/// Minimal initialization replay state.
struct InitializeWorkflowState {
    /// Whether an initialization fact has appeared.
    #[model(default)]
    initialized: bool,
    /// Whether retained facts violate initialization ordering or provenance.
    #[model(default)]
    malformed_history: bool,
}

#[derive(ModelOutput)]
/// Initialization decision projection.
struct InitializeWorkflowDecision {
    /// Folded guards required to decide initialization.
    context: InitializeWorkflowContext,
}

/// Initialization decision guards projected from replay state.
struct InitializeWorkflowContext {
    /// Whether an initialization fact has appeared.
    initialized: bool,
    /// Whether retained history must reject all commands.
    malformed_history: bool,
}

mapping! {
    InitializeWorkflowStateToDecision:
        (InitializeWorkflowState.initialized, InitializeWorkflowState.malformed_history) => InitializeWorkflowDecision.context
        using initialize_workflow_context;
}
mapping! { InitializeWorkflowStreamToEvent: InitializeWorkflow.stream => WorkflowEvent.stream using workflow_stream; }

mapping! {
    InitializeWorkflowToFact:
        (InitializeWorkflow.stream, InitializeWorkflow.state, InitializeWorkflowDecision.context) => WorkflowEvent.fact
        using try initialized_fact, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    reason = "EventCore fixes command-trait method names and generated mapping invocation order"
)]
impl ModelCommandLogic for InitializeWorkflow {
    type Event = WorkflowEvent;
    type State = InitializeWorkflowState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = WorkflowEventView::from_event(event);
        let mut folded = state.into_inner();
        match view.fact() {
            WorkflowFact::WorkflowInitialized {
                state: initialization_state,
            } => {
                let valid = view.stream() == self.stream.as_stream_id()
                    && !folded.malformed_history
                    && !folded.initialized
                    && initialization_is_well_formed(&self.stream, initialization_state);
                folded.malformed_history |= !valid;
                if valid {
                    folded.initialized = true;
                }
            }
            WorkflowFact::EffectRequested { .. }
            | WorkflowFact::EffectObserved { .. }
            | WorkflowFact::WorkflowCompleted { .. }
            | WorkflowFact::WorkflowStopped { .. } => {
                if !folded.initialized {
                    folded.malformed_history = true;
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = InitializeWorkflowDecision::model_builder()
            .context(InitializeWorkflowStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let stream = InitializeWorkflowStreamToEvent::apply(self);
        let fact = InitializeWorkflowToFact::apply((self, self, decision.as_ref()))?;
        Ok(ModeledEvents::one(
            WorkflowEvent::model_builder()
                .stream(stream)
                .fact(fact)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore input derives require adjacent origin fields in command data-flow order"
)]
#[derive(ModelInput)]
/// Origin values accepted by observation recording.
struct RecordObservationRequest {
    /// Semantic stream that owns the observation.
    #[model(origin)]
    stream: WorkflowStream,
    /// Shell outcome to persist.
    #[model(origin)]
    observation: EffectObservation,
}

/// Records exactly one shell observation without advancing the core.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore command derives retain stream then observation boundary values"
)]
#[derive(ModelCommand)]
struct RecordObservation {
    /// Declared command stream.
    #[stream]
    stream: WorkflowStream,
    /// Shell outcome to persist without advancing.
    observation: EffectObservation,
}

mapping! { RecordObservationRequestToStream: RecordObservationRequest.stream => RecordObservation.stream using clone; }
mapping! { RecordObservationRequestToObservation: RecordObservationRequest.observation => RecordObservation.observation using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::struct_excessive_bools,
    reason = "the narrow command fold tracks independent lifecycle guards rather than a broad aggregate"
)]
#[derive(ModelState)]
/// Replay state needed to accept exactly one observation.
struct RecordObservationState {
    /// Whether initialization was accepted.
    #[model(default)]
    initialized: bool,
    /// Ready continuation that binds all later request provenance.
    #[model(default)]
    initial_state: Option<HarnessState>,
    /// Checkpoint attached to the outstanding request.
    #[model(default)]
    pending_state: Option<HarnessState>,
    /// Identity of the outstanding effect.
    #[model(default)]
    pending_effect_id: Option<EffectId>,
    /// Permanent retained-history rejection fence.
    #[model(default)]
    malformed_history: bool,
    /// Persisted observation used to validate a retained terminal fact.
    #[model(default)]
    observation: Option<EffectObservation>,
    /// Whether the pending effect already has an observation.
    #[model(default)]
    observation_recorded: bool,
    /// Whether a terminal fact was encountered.
    #[model(default)]
    terminal: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::struct_excessive_bools,
    reason = "the private decision context preserves each independently folded guard"
)]
#[derive(Clone)]
/// Observation decision guards projected from replay state.
struct RecordObservationContext {
    /// Initialization guard.
    initialized: bool,
    /// Ready continuation that binds the outstanding request.
    initial_state: Option<HarnessState>,
    /// Waiting checkpoint of the outstanding request.
    pending_state: Option<HarnessState>,
    /// Outstanding effect identity.
    pending_effect_id: Option<EffectId>,
    /// Retained-history rejection guard.
    malformed_history: bool,
    /// Previously persisted outcome, if any.
    observation: Option<EffectObservation>,
    /// Duplicate-observation guard.
    observation_recorded: bool,
    /// Terminal lifecycle guard.
    terminal: bool,
}

#[derive(ModelOutput)]
/// Observation decision projection.
struct RecordObservationDecision {
    /// Replay guards needed by the observation mapper.
    context: RecordObservationContext,
}

mapping! {
    RecordObservationStateToDecision:
        (RecordObservationState.initialized, RecordObservationState.initial_state, RecordObservationState.pending_state, RecordObservationState.pending_effect_id, RecordObservationState.malformed_history, RecordObservationState.observation, RecordObservationState.observation_recorded, RecordObservationState.terminal) => RecordObservationDecision.context
        using record_observation_context;
}
mapping! { RecordObservationStreamToEvent: RecordObservation.stream => WorkflowEvent.stream using workflow_stream; }

mapping! {
    RecordObservationToFact:
        (RecordObservation.observation, RecordObservationDecision.context) => WorkflowEvent.fact
        using try observed_fact, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    reason = "EventCore fixes command-trait method signatures and replays borrowed event facts into the narrow fold"
)]
impl ModelCommandLogic for RecordObservation {
    type Event = WorkflowEvent;
    type State = RecordObservationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = WorkflowEventView::from_event(event);
        let event_stream_matches = view.stream() == self.stream.as_stream_id();
        let mut folded = state.into_inner();
        match view.fact() {
            WorkflowFact::WorkflowInitialized { state } => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && !folded.initialized
                    && !folded.terminal
                    && initialization_is_well_formed(&self.stream, state);
                folded.malformed_history |= !valid;
                if valid {
                    folded.initialized = true;
                    folded.initial_state = Some(state.clone());
                }
            }
            WorkflowFact::EffectRequested { state, effect } => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && folded.pending_state.is_none()
                    && folded.pending_effect_id.is_none()
                    && folded.observation.is_none()
                    && !folded.observation_recorded
                    && effect_request_is_well_formed(
                        &self.stream,
                        folded.initial_state.as_ref(),
                        state,
                        effect,
                    );
                folded.malformed_history |= !valid;
                if valid {
                    folded.pending_state = Some(state.clone());
                    folded.pending_effect_id = Some(effect_id(effect));
                    folded.observation = None;
                    folded.observation_recorded = false;
                }
            }
            WorkflowFact::EffectObserved { observation } => {
                let matching_pending = folded
                    .pending_effect_id
                    .as_ref()
                    .is_some_and(|pending| pending == observation.effect_id());
                let valid = !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && !folded.observation_recorded
                    && matching_pending;
                folded.malformed_history |= !event_stream_matches || !valid;
                if valid {
                    folded.observation = Some(observation.clone());
                    folded.observation_recorded = true;
                }
            }
            terminal @ (WorkflowFact::WorkflowCompleted { .. }
            | WorkflowFact::WorkflowStopped { .. }) => {
                folded.malformed_history |= !event_stream_matches
                    || folded.malformed_history
                    || !folded.initialized
                    || folded.terminal
                    || !terminal_fact_is_well_formed(
                        folded.pending_state.as_ref(),
                        folded.observation.as_ref(),
                        terminal,
                    );
                folded.terminal = true;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RecordObservationDecision::model_builder()
            .context(RecordObservationStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let stream = RecordObservationStreamToEvent::apply(self);
        let fact = RecordObservationToFact::apply((self, decision.as_ref()))?;
        Ok(ModeledEvents::one(
            WorkflowEvent::model_builder()
                .stream(stream)
                .fact(fact)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Origin values accepted by a pure-step request.
struct RequestNextEffectRequest {
    /// Semantic stream to advance.
    #[model(origin)]
    stream: WorkflowStream,
}

/// Advances a persisted workflow from one durable checkpoint.
#[derive(ModelCommand)]
struct RequestNextEffect {
    /// Declared command stream.
    #[stream]
    stream: WorkflowStream,
}

mapping! { RequestNextEffectRequestToStream: RequestNextEffectRequest.stream => RequestNextEffect.stream using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow step fold preserves lifecycle checkpoint order rather than alphabetical field order"
)]
#[derive(ModelState)]
/// Replay state needed to step one durable checkpoint.
struct RequestNextEffectState {
    /// Whether initialization was accepted.
    #[model(default)]
    initialized: bool,
    /// Original ready continuation.
    #[model(default)]
    initial_state: Option<HarnessState>,
    /// Waiting checkpoint attached to the outstanding request.
    #[model(default)]
    pending_state: Option<HarnessState>,
    /// Identity of the outstanding effect.
    #[model(default)]
    pending_effect_id: Option<EffectId>,
    /// Permanent retained-history rejection fence.
    #[model(default)]
    malformed_history: bool,
    /// Durable outcome for the outstanding effect.
    #[model(default)]
    observation: Option<EffectObservation>,
    /// Whether a terminal fact was encountered.
    #[model(default)]
    terminal: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the private decision context preserves checkpoint and guard order"
)]
#[derive(Clone)]
/// Step decision context projected from replay state.
struct RequestNextEffectContext {
    /// Initialization guard.
    initialized: bool,
    /// Initial ready continuation.
    initial_state: Option<HarnessState>,
    /// Waiting checkpoint to pass to the core.
    pending_state: Option<HarnessState>,
    /// Outstanding effect identity.
    pending_effect_id: Option<EffectId>,
    /// Retained-history rejection guard.
    malformed_history: bool,
    /// Persisted outcome to pass to the core.
    observation: Option<EffectObservation>,
    /// Terminal lifecycle guard.
    terminal: bool,
}

#[derive(ModelOutput)]
/// Pure-step decision projection.
struct RequestNextEffectDecision {
    /// Replay context needed by the step mapper.
    context: RequestNextEffectContext,
}

mapping! {
    RequestNextEffectStateToDecision:
        (RequestNextEffectState.initialized, RequestNextEffectState.initial_state, RequestNextEffectState.pending_state, RequestNextEffectState.pending_effect_id, RequestNextEffectState.malformed_history, RequestNextEffectState.observation, RequestNextEffectState.terminal) => RequestNextEffectDecision.context
        using request_next_effect_context;
}
mapping! { RequestNextEffectStreamToEvent: RequestNextEffect.stream => WorkflowEvent.stream using workflow_stream; }

mapping! {
    RequestNextEffectToFact:
        (RequestNextEffect.stream, RequestNextEffectDecision.context) => WorkflowEvent.fact
        using try next_effect_fact, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    reason = "EventCore fixes command-trait method signatures and replays borrowed event facts into the narrow checkpoint fold"
)]
impl ModelCommandLogic for RequestNextEffect {
    type Event = WorkflowEvent;
    type State = RequestNextEffectState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = WorkflowEventView::from_event(event);
        let event_stream_matches = view.stream() == self.stream.as_stream_id();
        let mut folded = state.into_inner();
        match view.fact() {
            WorkflowFact::WorkflowInitialized { state } => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && !folded.initialized
                    && !folded.terminal
                    && initialization_is_well_formed(&self.stream, state);
                folded.malformed_history |= !valid;
                if valid {
                    folded.initialized = true;
                    folded.initial_state = Some(state.clone());
                }
            }
            WorkflowFact::EffectRequested { state, effect } => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && folded.pending_state.is_none()
                    && folded.pending_effect_id.is_none()
                    && folded.observation.is_none()
                    && effect_request_is_well_formed(
                        &self.stream,
                        folded.initial_state.as_ref(),
                        state,
                        effect,
                    );
                folded.malformed_history |= !valid;
                if valid {
                    folded.pending_state = Some(state.clone());
                    folded.pending_effect_id = Some(effect_id(effect));
                    folded.observation = None;
                }
            }
            WorkflowFact::EffectObserved { observation } => {
                let matching_pending = folded
                    .pending_effect_id
                    .as_ref()
                    .is_some_and(|pending| pending == observation.effect_id());
                let valid = !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && folded.observation.is_none()
                    && matching_pending;
                folded.malformed_history |= !event_stream_matches || !valid;
                if valid {
                    folded.observation = Some(observation.clone());
                }
            }
            terminal @ (WorkflowFact::WorkflowCompleted { .. }
            | WorkflowFact::WorkflowStopped { .. }) => {
                folded.malformed_history |= !event_stream_matches
                    || folded.malformed_history
                    || !folded.initialized
                    || folded.terminal
                    || !terminal_fact_is_well_formed(
                        folded.pending_state.as_ref(),
                        folded.observation.as_ref(),
                        terminal,
                    );
                folded.terminal = true;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RequestNextEffectDecision::model_builder()
            .context(RequestNextEffectStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let stream = RequestNextEffectStreamToEvent::apply(self);
        let fact = RequestNextEffectToFact::apply((self, decision.as_ref()))?;
        Ok(ModeledEvents::one(
            WorkflowEvent::model_builder()
                .stream(stream)
                .fact(fact)
                .build(),
        ))
    }
}

/// Converts a stable workflow validation code into `EventCore`'s command error.
#[expect(
    clippy::implicit_return,
    reason = "the private error adapter is a direct typed conversion"
)]
fn command_error(code: &'static str) -> CommandError {
    CommandError::ValidationError(code.to_owned())
}

/// Converts the semantic workflow stream into its `EventCore` stream identity.
#[expect(
    clippy::implicit_return,
    reason = "the private stream adapter preserves the semantic stream identity"
)]
fn workflow_stream(stream: &WorkflowStream) -> StreamId {
    stream.as_stream_id().clone()
}

/// Returns the stable identity carried by a closed workflow effect.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the closed effect vocabulary has one inference provenance carrier"
)]
fn effect_id(effect: &TiberEffect) -> EffectId {
    match effect {
        TiberEffect::Infer(inference) => inference.effect_id().clone(),
    }
}

/// Validates a retained requested effect against stream and initial provenance.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the private history validator matches the closed inference effect envelope"
)]
fn effect_request_is_well_formed(
    stream: &WorkflowStream,
    initial_state: Option<&HarnessState>,
    state: &HarnessState,
    effect: &TiberEffect,
) -> bool {
    let Ok(stream_session) = stream.session() else {
        return false;
    };
    let Some(bound_initial_state) = initial_state else {
        return false;
    };
    let TiberEffect::Infer(inference) = effect;
    state.phase() == HarnessPhase::WaitingForInference
        && inference.session_id() == &stream_session
        && bound_initial_state.initial_effect() == state.initial_effect()
        && inference == state.initial_effect()
}

/// Validates that an initialization checkpoint is ready for its stream.
#[expect(
    clippy::implicit_return,
    reason = "the private initialization validator only compares parsed semantic identity"
)]
fn initialization_is_well_formed(stream: &WorkflowStream, state: &HarnessState) -> bool {
    let Ok(stream_session) = stream.session() else {
        return false;
    };
    state.phase() == HarnessPhase::Ready && state.initial_effect().session_id() == &stream_session
}

/// Validates that a retained terminal fact exactly matches the pure core step.
#[expect(
    clippy::implicit_return,
    reason = "retained terminal facts are compared with the core's closed deterministic step algebra"
)]
fn terminal_fact_is_well_formed(
    checkpoint_state: Option<&HarnessState>,
    recorded_observation: Option<&EffectObservation>,
    fact: &WorkflowFact,
) -> bool {
    let (Some(step_state), Some(effect_observation)) = (checkpoint_state, recorded_observation)
    else {
        return false;
    };
    match step(step_state, Some(effect_observation)) {
        TrampolineStep::Complete { state, receipt } => matches!(
            fact,
            WorkflowFact::WorkflowCompleted {
                state: fact_state,
                receipt: fact_receipt,
            } if state == *fact_state && receipt == *fact_receipt
        ),
        TrampolineStep::Stop { state, error } => matches!(
            fact,
            WorkflowFact::WorkflowStopped {
                state: fact_state,
                error: fact_error,
            } if state == *fact_state && error == *fact_error
        ),
        TrampolineStep::Continue { .. } => false,
    }
}

/// Builds initialization decision guards from the narrow replay state.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the checked-model mapper receives borrowed scalar state and returns one copied guard context"
)]
fn initialize_workflow_context(
    initialized: &bool,
    malformed_history: &bool,
) -> InitializeWorkflowContext {
    InitializeWorkflowContext {
        initialized: *initialized,
        malformed_history: *malformed_history,
    }
}

/// Produces the only initialization fact after replay guards are checked.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the checked-model mapping signature borrows fields and returns its typed validation result directly"
)]
fn initialized_fact(
    stream: &WorkflowStream,
    state: &HarnessState,
    context: &InitializeWorkflowContext,
) -> Result<WorkflowFact, CommandError> {
    if context.malformed_history {
        return Err(command_error("workflow_history_invalid"));
    }
    if context.initialized {
        return Err(command_error("workflow_already_initialized"));
    }
    if state.phase() != HarnessPhase::Ready {
        return Err(command_error("workflow_initial_state_not_ready"));
    }
    let stream_session = stream
        .session()
        .map_err(|_source| command_error("workflow_invalid_stream"))?;
    if state.initial_effect().session_id() != &stream_session {
        return Err(command_error("workflow_initial_state_session_mismatch"));
    }
    Ok(WorkflowFact::WorkflowInitialized {
        state: state.clone(),
    })
}

/// Copies the exact observation replay fields into its decision projection.
#[expect(
    clippy::implicit_return,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping functions receive borrowed modeled fields and build a cloned decision context"
)]
fn record_observation_context(
    initialized: &bool,
    initial_state: &Option<HarnessState>,
    pending_state: &Option<HarnessState>,
    pending_effect_id: &Option<EffectId>,
    malformed_history: &bool,
    recorded_observation: &Option<EffectObservation>,
    observation_recorded: &bool,
    terminal: &bool,
) -> RecordObservationContext {
    RecordObservationContext {
        initialized: *initialized,
        initial_state: initial_state.clone(),
        pending_state: pending_state.clone(),
        pending_effect_id: pending_effect_id.clone(),
        malformed_history: *malformed_history,
        observation: recorded_observation.clone(),
        observation_recorded: *observation_recorded,
        terminal: *terminal,
    }
}

/// Produces one observation fact after all retained provenance checks pass.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the mapping validates the borrowed pending effect before returning its one durable fact"
)]
fn observed_fact(
    observation: &EffectObservation,
    context: &RecordObservationContext,
) -> Result<WorkflowFact, CommandError> {
    if context.malformed_history {
        return Err(command_error("workflow_history_invalid"));
    }
    if !context.initialized {
        return Err(command_error("workflow_not_initialized"));
    }
    if context.terminal {
        return Err(command_error("workflow_terminal"));
    }
    let (Some(initial_state), Some(pending_state)) =
        (&context.initial_state, &context.pending_state)
    else {
        return Err(command_error("workflow_history_invalid"));
    };
    if initial_state.initial_effect() != pending_state.initial_effect()
        || context.observation.is_some() != context.observation_recorded
    {
        return Err(command_error("workflow_history_invalid"));
    }
    let Some(pending_effect_id) = &context.pending_effect_id else {
        return Err(command_error("workflow_effect_not_requested"));
    };
    if context.observation_recorded {
        return Err(command_error("workflow_observation_already_recorded"));
    }
    if observation.effect_id() != pending_effect_id {
        return Err(command_error("workflow_observation_effect_mismatch"));
    }
    Ok(WorkflowFact::EffectObserved {
        observation: observation.clone(),
    })
}

/// Copies the exact stepping replay fields into its decision projection.
#[expect(
    clippy::implicit_return,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping functions receive borrowed modeled fields and build a cloned decision context"
)]
fn request_next_effect_context(
    initialized: &bool,
    initial_state: &Option<HarnessState>,
    pending_state: &Option<HarnessState>,
    pending_effect_id: &Option<EffectId>,
    malformed_history: &bool,
    observation: &Option<EffectObservation>,
    terminal: &bool,
) -> RequestNextEffectContext {
    RequestNextEffectContext {
        initialized: *initialized,
        initial_state: initial_state.clone(),
        pending_state: pending_state.clone(),
        pending_effect_id: pending_effect_id.clone(),
        malformed_history: *malformed_history,
        observation: observation.clone(),
        terminal: *terminal,
    }
}

/// Converts a valid core continuation request into its durable fact.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the private mapper returns one validated durable request fact"
)]
fn checked_effect_request(
    stream: &WorkflowStream,
    state: HarnessState,
    effect: TiberEffect,
) -> Result<WorkflowFact, CommandError> {
    if !effect_request_is_well_formed(stream, Some(&state), &state, &effect) {
        return Err(command_error("workflow_effect_provenance_mismatch"));
    }
    Ok(WorkflowFact::EffectRequested { state, effect })
}

/// Converts every closed pure-core step outcome into one durable fact.
#[expect(
    clippy::implicit_return,
    reason = "the private mapper converts the closed core step algebra into one durable fact"
)]
fn fact_from_step(
    stream: &WorkflowStream,
    step: TrampolineStep,
) -> Result<WorkflowFact, CommandError> {
    match step {
        TrampolineStep::Continue { state, effect } => checked_effect_request(stream, state, effect),
        TrampolineStep::Complete { state, receipt } => {
            Ok(WorkflowFact::WorkflowCompleted { state, receipt })
        }
        TrampolineStep::Stop { state, error } => Ok(WorkflowFact::WorkflowStopped { state, error }),
    }
}

/// Produces the one successor fact allowed by a validated durable checkpoint.
#[expect(
    clippy::implicit_return,
    clippy::match_same_arms,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the private decision explicitly covers every durable checkpoint combination"
)]
fn next_effect_fact(
    stream: &WorkflowStream,
    context: &RequestNextEffectContext,
) -> Result<WorkflowFact, CommandError> {
    if context.malformed_history {
        return Err(command_error("workflow_history_invalid"));
    }
    if !context.initialized {
        return Err(command_error("workflow_not_initialized"));
    }
    if context.terminal {
        return Err(command_error("workflow_terminal"));
    }
    match (
        &context.initial_state,
        &context.pending_state,
        &context.pending_effect_id,
        &context.observation,
    ) {
        (_, Some(_), Some(_), None) => Err(command_error("workflow_effect_observation_missing")),
        (_, Some(state), Some(pending_effect_id), Some(observation)) => {
            if observation.effect_id() != pending_effect_id {
                return Err(command_error("workflow_observation_effect_mismatch"));
            }
            fact_from_step(stream, step(state, Some(observation)))
        }
        (_, Some(_), None, _) => Err(command_error("workflow_effect_not_requested")),
        (Some(initial_state), None, None, None) => {
            fact_from_step(stream, step(initial_state, None))
        }
        (Some(_), None, None, Some(_)) => Err(command_error("workflow_effect_not_requested")),
        (None, None, None, None) => Err(command_error("workflow_not_initialized")),
        (None, None, None, Some(_)) => Err(command_error("workflow_not_initialized")),
        (_, None, Some(_), _) => Err(command_error("workflow_effect_not_requested")),
    }
}

/// Builds the closed command that initializes one stream's pure continuation.
#[must_use]
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the public factory directly builds the checked command from its parsed request"
)]
pub fn initialize_workflow(
    stream: WorkflowStream,
    state: HarnessState,
) -> impl eventcore::CommandLogic {
    let request = InitializeWorkflowRequest::model_builder()
        .stream(stream)
        .state(state)
        .build();
    InitializeWorkflow::model_builder()
        .stream(InitializeWorkflowRequestToStream::apply(request.as_ref()))
        .state(InitializeWorkflowRequestToState::apply(request.as_ref()))
        .build()
}

/// Builds the closed command that persists a shell observation only.
///
/// It deliberately emits [`WorkflowFact::EffectObserved`] and nothing else;
/// a separate [`request_next_effect`] command advances the pure core after this
/// event is durable.
#[must_use]
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the public factory directly builds the checked command from its parsed request"
)]
pub fn record_observation(
    stream: WorkflowStream,
    observation: EffectObservation,
) -> impl eventcore::CommandLogic {
    let request = RecordObservationRequest::model_builder()
        .stream(stream)
        .observation(observation)
        .build();
    RecordObservation::model_builder()
        .stream(RecordObservationRequestToStream::apply(request.as_ref()))
        .observation(RecordObservationRequestToObservation::apply(
            request.as_ref(),
        ))
        .build()
}

/// Builds the closed command that advances the core after durable history is
/// folded. A pending request without an observation is rejected and never
/// re-emitted.
#[must_use]
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the public factory directly builds the checked stream-only command"
)]
pub fn request_next_effect(stream: WorkflowStream) -> impl eventcore::CommandLogic {
    let request = RequestNextEffectRequest::model_builder()
        .stream(stream)
        .build();
    RequestNextEffect::model_builder()
        .stream(RequestNextEffectRequestToStream::apply(request.as_ref()))
        .build()
}

#[cfg(test)]
mod tests {
    use core::fmt;

    use eventcore::model::{CheckStatus, StreamIdentity as _, check};
    use eventcore::{RetryPolicy, execute};
    use eventcore_memory::InMemoryEventStore;
    use eventcore_types::{EventStore as _, StreamVersion, StreamWrites};
    use futures::{StreamExt as _, executor::block_on};
    use tiber_workflow_core::{
        AgentId, AssignmentEpoch, AssignmentId, AssignmentScope, AttemptNumber, ContextReceiptId,
        DeadlineMilliseconds, EffectFailureCode, EffectId, EffectObservation, EffectReceiptId,
        HarnessPhase, IdempotencyKey, InferEffect, MAX_SESSION_ID_CHARACTERS, PolicyDecisionId,
        Retryability, SessionId, WorkflowId,
    };

    use super::*;

    #[expect(
        clippy::implicit_return,
        clippy::panic,
        reason = "invalid local fixtures must fail immediately"
    )]
    fn parsed<T>(value: &str, parser: impl FnOnce(&str) -> Result<T, HarnessError>) -> T {
        parser(value).unwrap_or_else(|error| panic!("valid fixture required: {error}"))
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixed local deadline fixture is required to parse"
    )]
    fn initial_effect() -> InferEffect {
        InferEffect::new(
            parsed("session-1", SessionId::parse),
            parsed("agent-1", AgentId::parse),
            parsed("workflow-1", WorkflowId::parse),
            parsed("assignment-1", AssignmentId::parse),
            parsed("scope-1", AssignmentScope::parse),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            parsed("context-1", ContextReceiptId::parse),
            parsed("policy-1", PolicyDecisionId::parse),
            parsed("effect-1", EffectId::parse),
            parsed("idempotency-1", IdempotencyKey::parse),
            DeadlineMilliseconds::parse(1_000).expect("fixture deadline is valid"),
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "the state fixture is a direct construction from the canonical initial effect"
    )]
    fn workflow_state() -> HarnessState {
        HarnessState::new(initial_effect())
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixed local workflow stream fixture is required to parse"
    )]
    fn workflow_stream() -> WorkflowStream {
        WorkflowStream::for_session(&parsed("session-1", SessionId::parse))
            .expect("fixture stream is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the observation fixture is a direct closed outcome construction"
    )]
    fn success_observation(effect_id: &str) -> EffectObservation {
        EffectObservation::Succeeded {
            effect_id: parsed(effect_id, EffectId::parse),
            receipt_id: parsed("receipt-1", EffectReceiptId::parse),
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::panic,
        reason = "the alternate same-session fixture uses fixed valid provenance values"
    )]
    fn foreign_waiting_state() -> HarnessState {
        let foreign = InferEffect::new(
            parsed("session-1", SessionId::parse),
            parsed("agent-foreign", AgentId::parse),
            parsed("workflow-foreign", WorkflowId::parse),
            parsed("assignment-foreign", AssignmentId::parse),
            parsed("scope-foreign", AssignmentScope::parse),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            parsed("context-foreign", ContextReceiptId::parse),
            parsed("policy-foreign", PolicyDecisionId::parse),
            parsed("effect-foreign", EffectId::parse),
            parsed("idempotency-foreign", IdempotencyKey::parse),
            DeadlineMilliseconds::parse(1_000).expect("fixture deadline is valid"),
        );
        let ready = HarnessState::new(foreign);
        let TrampolineStep::Continue { state, .. } = step(&ready, None) else {
            panic!("foreign ready fixture must request one effect");
        };
        state
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "test fixture reads must fail immediately on storage corruption"
    )]
    fn recorded_facts(store: &InMemoryEventStore, stream: &WorkflowStream) -> Vec<WorkflowFact> {
        block_on(async {
            let mut events = store
                .read_stream::<WorkflowEvent>(stream.as_stream_id().clone())
                .await
                .expect("workflow event stream is readable");
            let mut facts = Vec::new();
            while let Some(event) = events.next().await {
                facts.push(event.expect("stored workflow event decodes").fact.clone());
            }
            facts
        })
    }

    #[expect(
        clippy::expect_used,
        reason = "the assertion helper requires a rejected command"
    )]
    fn command_fails_with<T: fmt::Debug, E: fmt::Display>(result: Result<T, E>, code: &str) {
        let error = result.expect_err("command must be rejected");
        assert!(
            error.to_string().contains(code),
            "unexpected error: {error}"
        );
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the retained-history fixture must be assembled atomically"
    )]
    fn append_facts(
        store: &InMemoryEventStore,
        stream: &WorkflowStream,
        facts: impl IntoIterator<Item = WorkflowFact>,
    ) {
        let writes = facts.into_iter().fold(
            StreamWrites::new()
                .register_stream(stream.as_stream_id().clone(), StreamVersion::new(0))
                .expect("empty workflow stream can be registered"),
            |writes, fact| {
                writes
                    .append(WorkflowEvent {
                        stream: stream.as_stream_id().clone(),
                        fact,
                    })
                    .expect("workflow fixture event belongs to its registered stream")
            },
        );
        block_on(store.append_events(writes))
            .expect("retained workflow fixture facts append atomically");
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the checked-model report is the test's required assertion"
    )]
    fn every_registered_workflow_command_has_checked_provenance() {
        let report = check().expect("complete native workflow model");
        assert_eq!(report.status, CheckStatus::Verified);
        assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    }

    #[test]
    fn rejects_observation_or_trampoline_work_before_initialization() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_not_initialized",
        );
        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("effect-1")),
                RetryPolicy::new(),
            )),
            "workflow_not_initialized",
        );
        assert!(recorded_facts(&store, &stream).is_empty());
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the exact stream-boundary regression requires successful semantic stream construction"
    )]
    fn maximum_session_id_round_trips_through_workflow_stream() {
        let maximum = "s".repeat(MAX_SESSION_ID_CHARACTERS);
        let session = parsed(&maximum, SessionId::parse);
        let stream = WorkflowStream::for_session(&session).expect(
            "maximum valid session id must produce an EventCore-compatible workflow stream",
        );
        assert_eq!(
            stream
                .session()
                .expect("workflow stream must recover its maximum valid session id"),
            session
        );

        assert_eq!(
            SessionId::parse(&format!("{maximum}s")),
            Err(HarnessError::InvalidSessionId)
        );
        for glob_character in ["*", "?", "[", "]"] {
            assert_eq!(
                SessionId::parse(&format!("session{glob_character}id")),
                Err(HarnessError::InvalidSessionId)
            );
        }
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the retained-history fixture proves malformed facts permanently fence commands"
    )]
    fn malformed_history_remains_rejected_after_later_well_formed_facts() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let step_result = step(&ready, None);
        let TrampolineStep::Continue { state: waiting, .. } = step_result else {
            panic!("ready fixture must request one effect");
        };
        append_facts(
            &store,
            &stream,
            [
                WorkflowFact::WorkflowInitialized { state: ready },
                // This retained observation precedes all pending work.
                WorkflowFact::EffectObserved {
                    observation: success_observation("effect-1"),
                },
                // A valid-looking later request cannot erase the bad order.
                WorkflowFact::EffectRequested {
                    state: waiting,
                    effect: TiberEffect::Infer(initial_effect()),
                },
            ],
        );

        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("effect-1")),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 3);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the seeded retained-history fixture must append before command rejection is asserted"
    )]
    fn preinitialization_fact_permanently_rejects_initialization() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let waiting = match step(&ready, None) {
            TrampolineStep::Continue { state, .. } => state,
            TrampolineStep::Complete { .. } | TrampolineStep::Stop { .. } => {
                panic!("ready fixture must request one effect")
            }
        };
        append_facts(
            &store,
            &stream,
            [WorkflowFact::EffectRequested {
                state: waiting,
                effect: TiberEffect::Infer(initial_effect()),
            }],
        );

        command_fails_with(
            block_on(execute(
                &store,
                initialize_workflow(stream.clone(), ready),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 1);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the seeded retained-history fixture must append before command rejection is asserted"
    )]
    fn duplicate_or_foreign_effect_requests_permanently_reject_commands() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let waiting = match step(&ready, None) {
            TrampolineStep::Continue { state, .. } => state,
            TrampolineStep::Complete { .. } | TrampolineStep::Stop { .. } => {
                panic!("ready fixture must request one effect")
            }
        };
        let foreign_waiting = foreign_waiting_state();
        append_facts(
            &store,
            &stream,
            [
                WorkflowFact::WorkflowInitialized { state: ready },
                WorkflowFact::EffectRequested {
                    state: waiting,
                    effect: TiberEffect::Infer(initial_effect()),
                },
                // A second request and a same-session foreign checkpoint both
                // violate the one-pending-effect provenance fence.
                WorkflowFact::EffectRequested {
                    state: foreign_waiting.clone(),
                    effect: TiberEffect::Infer(foreign_waiting.initial_effect().clone()),
                },
            ],
        );

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("effect-1")),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 3);
    }

    #[test]
    fn first_same_session_foreign_request_permanently_rejects_commands() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let foreign_waiting = foreign_waiting_state();
        append_facts(
            &store,
            &stream,
            [
                WorkflowFact::WorkflowInitialized { state: ready },
                // This is the first request, but it carries different full
                // immutable provenance under the same semantic session.
                WorkflowFact::EffectRequested {
                    state: foreign_waiting.clone(),
                    effect: TiberEffect::Infer(foreign_waiting.initial_effect().clone()),
                },
            ],
        );

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("effect-foreign")),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 2);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the seeded retained-history fixture must append before command rejection is asserted"
    )]
    fn malformed_terminal_facts_permanently_reject_commands() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let waiting = match step(&ready, None) {
            TrampolineStep::Continue { state, .. } => state,
            TrampolineStep::Complete { .. } | TrampolineStep::Stop { .. } => {
                panic!("ready fixture must request one effect")
            }
        };
        append_facts(
            &store,
            &stream,
            [
                WorkflowFact::WorkflowInitialized { state: ready },
                WorkflowFact::EffectRequested {
                    state: waiting.clone(),
                    effect: TiberEffect::Infer(initial_effect()),
                },
                // A completed fact cannot precede the observation that would
                // deterministically produce its receipt and terminal state.
                WorkflowFact::WorkflowCompleted {
                    state: waiting,
                    receipt: parsed("bogus-receipt", EffectReceiptId::parse),
                },
            ],
        );

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 3);
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the seeded retained-history fixture derives a valid terminal state before corrupting only its receipt"
    )]
    fn terminal_with_wrong_receipt_permanently_rejects_commands() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let ready = workflow_state();
        let waiting = match step(&ready, None) {
            TrampolineStep::Continue { state, .. } => state,
            TrampolineStep::Complete { .. } | TrampolineStep::Stop { .. } => {
                panic!("ready fixture must request one effect")
            }
        };
        let observed = success_observation("effect-1");
        let complete_state = match step(&waiting, Some(&observed)) {
            TrampolineStep::Complete { state, .. } => state,
            TrampolineStep::Continue { .. } | TrampolineStep::Stop { .. } => {
                panic!("successful observation fixture must complete")
            }
        };
        append_facts(
            &store,
            &stream,
            [
                WorkflowFact::WorkflowInitialized { state: ready },
                WorkflowFact::EffectRequested {
                    state: waiting,
                    effect: TiberEffect::Infer(initial_effect()),
                },
                WorkflowFact::EffectObserved {
                    observation: observed,
                },
                WorkflowFact::WorkflowCompleted {
                    state: complete_state,
                    receipt: parsed("wrong-receipt", EffectReceiptId::parse),
                },
            ],
        );

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_history_invalid",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 4);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "workflow integration fixtures fail fast when a required command does not succeed"
    )]
    fn initializes_once_and_requests_the_exact_inference_effect() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        let state = workflow_state();

        block_on(execute(
            &store,
            initialize_workflow(stream.clone(), state.clone()),
            RetryPolicy::new(),
        ))
        .expect("initialization succeeds");
        command_fails_with(
            block_on(execute(
                &store,
                initialize_workflow(stream.clone(), state),
                RetryPolicy::new(),
            )),
            "workflow_already_initialized",
        );
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("first request asks the pure core for work");
        // Initialization remains refused after ordinary lifecycle facts; the
        // initialization fold does not mistake valid post-init history for a
        // pre-initialization corruption fence.
        command_fails_with(
            block_on(execute(
                &store,
                initialize_workflow(stream.clone(), workflow_state()),
                RetryPolicy::new(),
            )),
            "workflow_already_initialized",
        );

        let facts = recorded_facts(&store, &stream);
        assert_eq!(facts.len(), 2);
        let Some(WorkflowFact::EffectRequested {
            state: requested_state,
            effect,
        }) = facts.get(1).cloned()
        else {
            panic!("expected second fact to be an effect request");
        };
        assert_eq!(requested_state.phase(), HarnessPhase::WaitingForInference);
        assert_eq!(effect, TiberEffect::Infer(initial_effect()));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "workflow integration fixtures fail fast when setup cannot reach the pending-effect boundary"
    )]
    fn refuses_to_reissue_a_pending_effect_before_an_observation() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        block_on(execute(
            &store,
            initialize_workflow(stream.clone(), workflow_state()),
            RetryPolicy::new(),
        ))
        .expect("initialization succeeds");
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("first request succeeds");

        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_effect_observation_missing",
        );
        let facts = recorded_facts(&store, &stream);
        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(fact, WorkflowFact::EffectRequested { .. }))
                .count(),
            1
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "workflow integration fixtures fail fast when setup cannot reach observation persistence"
    )]
    fn observation_must_match_one_outstanding_effect_and_cannot_repeat() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        block_on(execute(
            &store,
            initialize_workflow(stream.clone(), workflow_state()),
            RetryPolicy::new(),
        ))
        .expect("initialization succeeds");
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("effect request succeeds");

        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("other-effect")),
                RetryPolicy::new(),
            )),
            "workflow_observation_effect_mismatch",
        );
        let observation = success_observation("effect-1");
        block_on(execute(
            &store,
            record_observation(stream.clone(), observation.clone()),
            RetryPolicy::new(),
        ))
        .expect("matching observation is persisted");
        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), observation),
                RetryPolicy::new(),
            )),
            "workflow_observation_already_recorded",
        );
        let facts = recorded_facts(&store, &stream);
        assert_eq!(facts.len(), 3);
        assert!(matches!(
            facts.get(2),
            Some(WorkflowFact::EffectObserved { .. })
        ));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "workflow integration fixtures fail fast when the intended durable transition does not execute"
    )]
    fn persists_the_observation_before_a_fresh_command_completes_the_workflow() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        block_on(execute(
            &store,
            initialize_workflow(stream.clone(), workflow_state()),
            RetryPolicy::new(),
        ))
        .expect("initialization succeeds");
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("effect request succeeds");
        block_on(execute(
            &store,
            record_observation(stream.clone(), success_observation("effect-1")),
            RetryPolicy::new(),
        ))
        .expect("observation is its own durable transaction");

        // This is deliberately a fresh command value: recovery reconstructs
        // authority from durable facts, not a closure continuation.
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("fresh request completes from persisted observation");

        let facts = recorded_facts(&store, &stream);
        assert!(matches!(
            facts.as_slice(),
            [
                WorkflowFact::WorkflowInitialized { .. },
                WorkflowFact::EffectRequested { .. },
                WorkflowFact::EffectObserved { .. },
                WorkflowFact::WorkflowCompleted { .. },
            ]
        ));
        command_fails_with(
            block_on(execute(
                &store,
                request_next_effect(stream.clone()),
                RetryPolicy::new(),
            )),
            "workflow_terminal",
        );
        command_fails_with(
            block_on(execute(
                &store,
                record_observation(stream.clone(), success_observation("effect-1")),
                RetryPolicy::new(),
            )),
            "workflow_terminal",
        );
        assert_eq!(recorded_facts(&store, &stream).len(), 4);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "workflow integration fixtures fail fast when the expected stopped path cannot be reached"
    )]
    fn failed_or_unknown_observations_stop_only_after_their_durable_checkpoint() {
        let store = InMemoryEventStore::new();
        let stream = workflow_stream();
        block_on(execute(
            &store,
            initialize_workflow(stream.clone(), workflow_state()),
            RetryPolicy::new(),
        ))
        .expect("initialization succeeds");
        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("effect request succeeds");
        let failure = EffectObservation::Failed {
            effect_id: parsed("effect-1", EffectId::parse),
            code: parsed("shell-timeout", EffectFailureCode::parse),
            retryability: Retryability::Retryable,
        };
        block_on(execute(
            &store,
            record_observation(stream.clone(), failure),
            RetryPolicy::new(),
        ))
        .expect("failure observation becomes durable first");
        let before_stop = recorded_facts(&store, &stream);
        assert!(matches!(
            before_stop.as_slice(),
            [
                WorkflowFact::WorkflowInitialized { .. },
                WorkflowFact::EffectRequested { .. },
                WorkflowFact::EffectObserved { .. },
            ]
        ));

        block_on(execute(
            &store,
            request_next_effect(stream.clone()),
            RetryPolicy::new(),
        ))
        .expect("later trampoline request records the stop");
        let facts = recorded_facts(&store, &stream);
        assert!(matches!(
            facts.last(),
            Some(WorkflowFact::WorkflowStopped {
                error: HarnessError::EffectFailed,
                ..
            })
        ));
    }
}
