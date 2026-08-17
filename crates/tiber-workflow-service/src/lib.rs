//! Native `EventCore` authority for Tiber's durable workflow trampoline.
//!
//! Each public factory below is a closed, command-specific write boundary. The
//! service persists a shell observation in its own transaction before a later
//! command asks the pure core to advance the trampoline.

#![expect(
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    reason = "EventCore's ModelEvent derive generates public checked-model helpers at crate scope without an item-local lint hook"
)]

use core::{error::Error, fmt};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledCommand, ModeledEvents, StreamIdentity},
};
use serde::{Deserialize, Serialize, de::Error as _};
use tiber_workflow_core::{
    EffectId, EffectObservation, EffectReceiptId, HarnessError, HarnessPhase, HarnessState,
    TiberEffect, TrampolineStep, continue_after_completion, step,
};

/// Defines opaque checked workflow publication tokens.
macro_rules! workflow_publication {
    ($name:ident) => {
        /// Opaque checked workflow fact accepted by the signed publication adapter.
        pub struct $name {
            /// Exact stream fence returned by the checked command.
            consistency_streams: [StreamId; 1],
            /// Checked durable workflow event.
            event: WorkflowEvent,
        }

        impl $name {
            /// Borrows the checked event for composing a later modeled decision.
            #[must_use]
            #[inline]
            pub const fn event(&self) -> &WorkflowEvent {
                &self.event
            }
            /// Transfers the checked fact and exact consistency stream.
            #[must_use]
            #[inline]
            pub fn into_event_and_consistency_streams(self) -> (WorkflowEvent, [StreamId; 1]) {
                (self.event, self.consistency_streams)
            }
        }
    };
}

/// Errors raised while converting between the semantic workflow stream and an
/// external stream identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkflowServiceError {
    /// A checked workflow command did not emit exactly one fact.
    InvalidModeledEmission,
    /// The encoded stream did not have the workflow-session form.
    InvalidStream,
    /// A checked workflow command rejected the supplied history.
    ModeledCommandFailed,
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
            Self::ModeledCommandFailed => "workflow_modeled_command_failed",
            Self::InvalidModeledEmission => "workflow_invalid_modeled_emission",
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowStream {
    /// Effect identity when this is a bounded per-effect stream.
    effect_id: Option<EffectId>,
    /// Exact durable stream identity.
    id: StreamId,
    /// Session identity encoded by this durable stream.
    session: tiber_workflow_core::SessionId,
}

#[expect(
    clippy::missing_inline_in_public_items,
    reason = "EventCore's stream-identity trait exposes the exact stored stream without an adapter-owned transformation"
)]
impl StreamIdentity for WorkflowStream {
    fn as_stream_id(&self) -> &StreamId {
        &self.id
    }
}

impl WorkflowStream {
    /// Creates the execution stream for one exact inference effect.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowServiceError::InvalidStream`] when `EventCore` rejects
    /// the effect-derived stream identifier.
    #[inline]
    pub fn for_effect(
        effect: &tiber_workflow_core::InferEffect,
    ) -> Result<Self, WorkflowServiceError> {
        StreamId::try_new(format!("tiber:workflow:{}", effect.effect_id().as_str()))
            .map(|id| Self {
                id,
                session: effect.session_id().clone(),
                effect_id: Some(effect.effect_id().clone()),
            })
            .map_err(|_source| WorkflowServiceError::InvalidStream)
    }
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
    #[cfg(test)]
    fn for_session(session: &tiber_workflow_core::SessionId) -> Result<Self, WorkflowServiceError> {
        StreamId::try_new(format!("tiber:workflow:{}", session.as_str()))
            .map(|id| Self {
                id,
                session: session.clone(),
                effect_id: None,
            })
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
        Ok(self.session.clone())
    }

    /// Returns the exact durable stream identifier.
    #[must_use]
    #[inline]
    pub const fn stream_id(&self) -> &StreamId {
        &self.id
    }
}

/// Immutable workflow facts emitted by the closed native command surface.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "fact variants follow durable lifecycle order and retain complete serializable checkpoints"
)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
        /// Workflow-owned ready continuation for the next owner turn.
        successor: HarnessState,
    },
    /// The workflow reached a typed stopped terminal state.
    WorkflowStopped {
        /// Terminal pure state.
        state: HarnessState,
        /// Typed reason the core declined to continue.
        error: HarnessError,
    },
}

/// Deserialization-only compatibility shape for retained workflow facts.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the compatibility wire mirrors the durable fact lifecycle order exactly"
)]
#[derive(Deserialize)]
enum WorkflowFactWire {
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
        /// Successor added after the original durable schema.
        #[serde(default)]
        successor: Option<HarnessState>,
    },
    /// The workflow reached a typed stopped terminal state.
    WorkflowStopped {
        /// Terminal pure state.
        state: HarnessState,
        /// Typed reason the core declined to continue.
        error: HarnessError,
    },
}

#[expect(
    clippy::missing_trait_methods,
    reason = "Serde's default in-place deserialization correctly delegates to this compatibility decoder"
)]
impl<'de> Deserialize<'de> for WorkflowFact {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match WorkflowFactWire::deserialize(deserializer)? {
            WorkflowFactWire::WorkflowInitialized { state } => {
                Ok(Self::WorkflowInitialized { state })
            }
            WorkflowFactWire::EffectRequested { state, effect } => {
                Ok(Self::EffectRequested { state, effect })
            }
            WorkflowFactWire::EffectObserved { observation } => {
                Ok(Self::EffectObserved { observation })
            }
            WorkflowFactWire::WorkflowCompleted {
                state,
                receipt,
                successor: retained_successor,
            } => {
                let successor = retained_successor.map_or_else(
                    || {
                        continue_after_completion(&state)
                            .map_err(|error| D::Error::custom(error.code()))
                    },
                    Ok,
                )?;
                Ok(Self::WorkflowCompleted {
                    state,
                    receipt,
                    successor,
                })
            }
            WorkflowFactWire::WorkflowStopped { state, error } => {
                Ok(Self::WorkflowStopped { state, error })
            }
        }
    }
}

/// Durable `EventCore` event for one native workflow stream.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
#[non_exhaustive]
pub struct WorkflowEvent {
    /// Immutable business fact.
    fact: WorkflowFact,
    /// Owning workflow stream.
    stream: StreamId,
}

impl WorkflowEvent {
    /// Returns the immutable business fact.
    #[must_use]
    #[inline]
    pub const fn fact(&self) -> &WorkflowFact {
        &self.fact
    }

    /// Returns the owning workflow stream.
    #[must_use]
    #[inline]
    #[expect(
        clippy::same_name_method,
        reason = "the public durable-envelope accessor intentionally matches EventCore's required stream_id contract"
    )]
    pub const fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[expect(
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
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

/// Opaque checked initialization accepted by the signed publication adapter.
pub struct WorkflowInitializationPublication {
    /// Exact streams whose versions fence the initialization decision.
    consistency_streams: Vec<StreamId>,
    /// Checked durable workflow event.
    event: WorkflowEvent,
    /// Completed predecessor effect for a successor initialization.
    predecessor_effect_id: Option<EffectId>,
}

impl WorkflowInitializationPublication {
    /// Borrows the checked event for composing its first effect request.
    #[must_use]
    #[inline]
    pub const fn event(&self) -> &WorkflowEvent {
        &self.event
    }

    /// Transfers the checked fact and exact consistency streams.
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (WorkflowEvent, Vec<StreamId>) {
        (self.event, self.consistency_streams)
    }

    /// Borrows the predecessor identity carried by successor authority.
    #[must_use]
    #[inline]
    pub const fn predecessor_effect_id(&self) -> Option<&EffectId> {
        self.predecessor_effect_id.as_ref()
    }
}

workflow_publication!(WorkflowEffectRequestPublication);
workflow_publication!(WorkflowObservationPublication);
workflow_publication!(WorkflowAdvancePublication);

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

#[derive(ModelInput)]
/// Origin streams accepted by successor initialization.
struct InitializeSuccessorWorkflowRequest {
    /// Completed predecessor workflow whose terminal fact grants authority.
    #[model(origin)]
    predecessor: WorkflowStream,
    /// Exact effect-bound stream receiving the derived successor state.
    #[model(origin)]
    successor: WorkflowStream,
}

#[derive(ModelCommand)]
/// Initializes only the exact successor retained by a completed workflow.
struct InitializeSuccessorWorkflow {
    /// Completed predecessor workflow stream read by the decision.
    #[stream]
    predecessor: WorkflowStream,
    /// Exact effect-bound successor stream receiving initialization.
    #[stream]
    successor: WorkflowStream,
}

mapping! {
    InitializeSuccessorRequestToPredecessor:
        InitializeSuccessorWorkflowRequest.predecessor => InitializeSuccessorWorkflow.predecessor
        using clone;
}
mapping! {
    InitializeSuccessorRequestToSuccessor:
        InitializeSuccessorWorkflowRequest.successor => InitializeSuccessorWorkflow.successor
        using clone;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the successor-authority fold follows the predecessor workflow lifecycle"
)]
#[derive(ModelState)]
/// Replay state needed to derive one exact successor initialization.
struct InitializeSuccessorWorkflowState {
    /// Whether predecessor initialization was accepted.
    #[model(default)]
    initialized: bool,
    /// Original ready continuation of the predecessor.
    #[model(default)]
    initial_state: Option<HarnessState>,
    /// Waiting checkpoint attached to the predecessor request.
    #[model(default)]
    pending_state: Option<HarnessState>,
    /// Identity of the predecessor's outstanding effect.
    #[model(default)]
    pending_effect_id: Option<EffectId>,
    /// Durable outcome for the predecessor effect.
    #[model(default)]
    observation: Option<EffectObservation>,
    /// Exact successor retained by a valid completion fact.
    #[model(default)]
    successor_state: Option<HarnessState>,
    /// Permanent retained-history rejection fence.
    #[model(default)]
    malformed_history: bool,
    /// Whether a terminal predecessor fact was encountered.
    #[model(default)]
    terminal: bool,
}

#[derive(Clone)]
/// Closed context consumed by the successor initialization mapper.
struct InitializeSuccessorWorkflowContext {
    /// Permanent retained-history rejection fence.
    malformed_history: bool,
    /// Exact ready successor derived from retained completion.
    successor_state: Option<HarnessState>,
    /// Whether the predecessor reached one valid terminal fact.
    terminal: bool,
}

#[derive(ModelOutput)]
/// Successor initialization decision projection.
struct InitializeSuccessorWorkflowDecision {
    /// Complete folded authority needed to emit initialization.
    context: InitializeSuccessorWorkflowContext,
}

mapping! {
    InitializeSuccessorStateToDecision:
        (InitializeSuccessorWorkflowState.initialized, InitializeSuccessorWorkflowState.initial_state, InitializeSuccessorWorkflowState.pending_state, InitializeSuccessorWorkflowState.pending_effect_id, InitializeSuccessorWorkflowState.observation, InitializeSuccessorWorkflowState.successor_state, InitializeSuccessorWorkflowState.malformed_history, InitializeSuccessorWorkflowState.terminal) => InitializeSuccessorWorkflowDecision.context
        using successor_initialization_context;
}
mapping! {
    InitializeSuccessorStreamToEvent:
        InitializeSuccessorWorkflow.successor => WorkflowEvent.stream
        using workflow_stream;
}
mapping! {
    InitializeSuccessorToFact:
        (InitializeSuccessorWorkflow.predecessor, InitializeSuccessorWorkflow.successor, InitializeSuccessorWorkflowDecision.context) => WorkflowEvent.fact
        using try successor_initialized_fact, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    reason = "EventCore fixes command-trait signatures while the fold validates one exact completed predecessor"
)]
impl ModelCommandLogic for InitializeSuccessorWorkflow {
    type Event = WorkflowEvent;
    type State = InitializeSuccessorWorkflowState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = WorkflowEventView::from_event(event);
        let event_stream_matches = view.stream() == self.predecessor.as_stream_id();
        let mut folded = state.into_inner();
        match view.fact() {
            WorkflowFact::WorkflowInitialized { state } => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && !folded.initialized
                    && !folded.terminal
                    && initialization_is_well_formed(&self.predecessor, state);
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
                        &self.predecessor,
                        folded.initial_state.as_ref(),
                        state,
                        effect,
                    );
                folded.malformed_history |= !valid;
                if valid {
                    folded.pending_state = Some(state.clone());
                    folded.pending_effect_id = Some(effect_id(effect));
                }
            }
            WorkflowFact::EffectObserved { observation } => {
                let matching_pending = folded
                    .pending_effect_id
                    .as_ref()
                    .is_some_and(|pending| pending == observation.effect_id());
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && folded.observation.is_none()
                    && matching_pending;
                folded.malformed_history |= !valid;
                if valid {
                    folded.observation = Some(observation.clone());
                }
            }
            terminal @ (WorkflowFact::WorkflowCompleted { .. }
            | WorkflowFact::WorkflowStopped { .. }) => {
                let valid = event_stream_matches
                    && !folded.malformed_history
                    && folded.initialized
                    && !folded.terminal
                    && terminal_fact_is_well_formed(
                        folded.pending_state.as_ref(),
                        folded.observation.as_ref(),
                        terminal,
                    );
                folded.malformed_history |= !valid;
                folded.terminal = true;
                if valid && let WorkflowFact::WorkflowCompleted { successor, .. } = terminal {
                    folded.successor_state = Some(successor.clone());
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = InitializeSuccessorWorkflowDecision::model_builder()
            .context(InitializeSuccessorStateToDecision::apply((
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
        Ok(ModeledEvents::one(
            WorkflowEvent::model_builder()
                .stream(InitializeSuccessorStreamToEvent::apply(self))
                .fact(InitializeSuccessorToFact::apply((
                    self,
                    self,
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

/// Projects the complete predecessor fold into successor-initialization authority.
#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    reason = "the checked-model mapper consumes every narrow predecessor replay field before projecting successor authority"
)]
fn successor_initialization_context(
    initialized: &bool,
    initial_state: &Option<HarnessState>,
    pending_state: &Option<HarnessState>,
    pending_effect_id: &Option<EffectId>,
    observation: &Option<EffectObservation>,
    successor_state: &Option<HarnessState>,
    malformed_history: &bool,
    terminal: &bool,
) -> InitializeSuccessorWorkflowContext {
    let structurally_complete = *initialized
        && initial_state.is_some()
        && pending_state.is_some()
        && pending_effect_id.is_some()
        && observation.is_some();
    InitializeSuccessorWorkflowContext {
        malformed_history: *malformed_history || !structurally_complete,
        successor_state: successor_state.clone(),
        terminal: *terminal,
    }
}

/// Builds the sole successor initialization fact from validated predecessor authority.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the checked mapper emits only the completed predecessor's exact ready successor"
)]
fn successor_initialized_fact(
    predecessor: &WorkflowStream,
    successor: &WorkflowStream,
    context: &InitializeSuccessorWorkflowContext,
) -> Result<WorkflowFact, CommandError> {
    if context.malformed_history || !context.terminal {
        return Err(command_error("workflow_successor_history_invalid"));
    }
    let state = context
        .successor_state
        .as_ref()
        .ok_or_else(|| command_error("workflow_successor_not_completed"))?;
    let predecessor_effect_id = predecessor
        .effect_id
        .as_ref()
        .ok_or_else(|| command_error("workflow_successor_predecessor_not_effect_bound"))?;
    if state.initial_effect().effect_id() == predecessor_effect_id
        || !initialization_is_well_formed(successor, state)
    {
        return Err(command_error("workflow_successor_mismatch"));
    }
    Ok(WorkflowFact::WorkflowInitialized {
        state: state.clone(),
    })
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
        && stream
            .effect_id
            .as_ref()
            .is_none_or(|effect_id| inference.effect_id() == effect_id)
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
    state.phase() == HarnessPhase::Ready
        && state.initial_effect().session_id() == &stream_session
        && stream
            .effect_id
            .as_ref()
            .is_none_or(|effect_id| state.initial_effect().effect_id() == effect_id)
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
                successor,
            } if state == *fact_state
                && receipt == *fact_receipt
                && continue_after_completion(&state).is_ok_and(|expected| expected == *successor)
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
    if stream
        .effect_id
        .as_ref()
        .is_some_and(|effect_id| state.initial_effect().effect_id() != effect_id)
    {
        return Err(command_error("workflow_initial_state_effect_mismatch"));
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
        TrampolineStep::Complete { state, receipt } => Ok(WorkflowFact::WorkflowCompleted {
            successor: continue_after_completion(&state)
                .map_err(|error| command_error(error.code()))?,
            state,
            receipt,
        }),
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
#[cfg(test)]
fn initialize_workflow(
    stream: WorkflowStream,
    state: HarnessState,
) -> ModeledCommand<InitializeWorkflow> {
    let request = InitializeWorkflowRequest::model_builder()
        .stream(stream)
        .state(state)
        .build();
    InitializeWorkflow::model_builder()
        .stream(InitializeWorkflowRequestToStream::apply(request.as_ref()))
        .state(InitializeWorkflowRequestToState::apply(request.as_ref()))
        .build()
}

/// Builds the checked command that derives an exact completed-workflow successor.
#[must_use]
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the factory maps only its two effect-bound stream origins into the checked command"
)]
#[expect(
    clippy::single_call_fn,
    reason = "the continuation boundary has one successor-command construction site"
)]
fn initialize_successor_workflow(
    predecessor: WorkflowStream,
    successor: WorkflowStream,
) -> ModeledCommand<InitializeSuccessorWorkflow> {
    let request = InitializeSuccessorWorkflowRequest::model_builder()
        .predecessor(predecessor)
        .successor(successor)
        .build();
    InitializeSuccessorWorkflow::model_builder()
        .predecessor(InitializeSuccessorRequestToPredecessor::apply(
            request.as_ref(),
        ))
        .successor(InitializeSuccessorRequestToSuccessor::apply(
            request.as_ref(),
        ))
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
#[cfg(test)]
fn record_observation(
    stream: WorkflowStream,
    observation: EffectObservation,
) -> ModeledCommand<RecordObservation> {
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
fn request_next_effect(stream: WorkflowStream) -> ModeledCommand<RequestNextEffect> {
    let request = RequestNextEffectRequest::model_builder()
        .stream(stream)
        .build();
    RequestNextEffect::model_builder()
        .stream(RequestNextEffectRequestToStream::apply(request.as_ref()))
        .build()
}

/// Decides one workflow initialization fact from empty execution history.
///
/// # Errors
///
/// Returns [`WorkflowServiceError`] when the checked initialization command
/// cannot emit one exact fact.
#[inline]
pub fn decide_initialize_workflow(
    stream: WorkflowStream,
    state: HarnessState,
) -> Result<WorkflowInitializationPublication, WorkflowServiceError> {
    let consistency = stream.id.clone();
    let request = InitializeWorkflowRequest::model_builder()
        .stream(stream)
        .state(state)
        .build();
    let command = InitializeWorkflow::model_builder()
        .stream(InitializeWorkflowRequestToStream::apply(request.as_ref()))
        .state(InitializeWorkflowRequestToState::apply(request.as_ref()))
        .build();
    let events: Vec<WorkflowEvent> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| WorkflowServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| WorkflowServiceError::InvalidModeledEmission)?;
    Ok(WorkflowInitializationPublication {
        event,
        consistency_streams: vec![consistency],
        predecessor_effect_id: None,
    })
}

/// Derives initialization for the exact successor retained by durable completion.
///
/// # Errors
///
/// Returns [`WorkflowServiceError`] when the predecessor history is malformed,
/// stopped, incomplete, or names a different successor effect.
#[inline]
pub fn decide_initialize_successor_workflow(
    history: &[WorkflowEvent],
    predecessor: WorkflowStream,
    successor: WorkflowStream,
) -> Result<WorkflowInitializationPublication, WorkflowServiceError> {
    let predecessor_consistency = predecessor.id.clone();
    let successor_consistency = successor.id.clone();
    let predecessor_effect_id = predecessor.effect_id.clone();
    let command = initialize_successor_workflow(predecessor, successor);
    let mut state: Modeled<InitializeSuccessorWorkflowState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<WorkflowEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| WorkflowServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| WorkflowServiceError::InvalidModeledEmission)?;
    Ok(WorkflowInitializationPublication {
        event,
        consistency_streams: vec![predecessor_consistency, successor_consistency],
        predecessor_effect_id,
    })
}

/// Decides the exact first effect requested by one initialized execution.
///
/// # Errors
///
/// Returns [`WorkflowServiceError`] when retained history cannot authorize one
/// exact next-effect fact.
#[inline]
pub fn decide_request_next_effect(
    history: &[WorkflowEvent],
    stream: WorkflowStream,
) -> Result<WorkflowEffectRequestPublication, WorkflowServiceError> {
    let consistency = stream.id.clone();
    let command = request_next_effect(stream);
    let mut state: Modeled<RequestNextEffectState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<WorkflowEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| WorkflowServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| WorkflowServiceError::InvalidModeledEmission)?;
    Ok(WorkflowEffectRequestPublication {
        event,
        consistency_streams: [consistency],
    })
}

/// Decides one durable shell observation for its pending effect.
///
/// # Errors
///
/// Returns [`WorkflowServiceError`] when retained history does not authorize
/// the supplied observation.
#[inline]
pub fn decide_record_observation(
    history: &[WorkflowEvent],
    stream: WorkflowStream,
    observation: EffectObservation,
) -> Result<WorkflowObservationPublication, WorkflowServiceError> {
    let consistency = stream.id.clone();
    let request = RecordObservationRequest::model_builder()
        .stream(stream)
        .observation(observation)
        .build();
    let command = RecordObservation::model_builder()
        .stream(RecordObservationRequestToStream::apply(request.as_ref()))
        .observation(RecordObservationRequestToObservation::apply(
            request.as_ref(),
        ))
        .build();
    let mut state: Modeled<RecordObservationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<WorkflowEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| WorkflowServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| WorkflowServiceError::InvalidModeledEmission)?;
    Ok(WorkflowObservationPublication {
        event,
        consistency_streams: [consistency],
    })
}

/// Advances an observed execution to its deterministic terminal fact.
///
/// # Errors
///
/// Returns [`WorkflowServiceError`] when retained history cannot authorize one
/// deterministic terminal fact.
#[inline]
pub fn decide_advance_workflow(
    history: &[WorkflowEvent],
    stream: WorkflowStream,
) -> Result<WorkflowAdvancePublication, WorkflowServiceError> {
    let consistency = stream.id.clone();
    let command = request_next_effect(stream);
    let mut state: Modeled<RequestNextEffectState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<WorkflowEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| WorkflowServiceError::ModeledCommandFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| WorkflowServiceError::InvalidModeledEmission)?;
    Ok(WorkflowAdvancePublication {
        event,
        consistency_streams: [consistency],
    })
}

#[cfg(test)]
mod tests {
    use core::{fmt, slice};

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

    #[test]
    #[expect(
        clippy::absolute_paths,
        clippy::expect_used,
        clippy::panic,
        clippy::pattern_type_mismatch,
        reason = "the closed-publication test uses explicit fixture setup and verifies its one modeled terminal fact"
    )]
    fn closed_publications_drive_one_complete_effect_execution() {
        let stream = workflow_stream();
        let initialized = decide_initialize_workflow(stream.clone(), workflow_state())
            .expect("initialization modeled")
            .into_event_and_consistency_streams()
            .0;
        let requested =
            decide_request_next_effect(core::slice::from_ref(&initialized), stream.clone())
                .expect("request modeled")
                .into_event_and_consistency_streams()
                .0;
        assert!(matches!(
            requested.fact(),
            WorkflowFact::EffectRequested { .. }
        ));
        let observed = decide_record_observation(
            &[initialized.clone(), requested.clone()],
            stream.clone(),
            success_observation("effect-1"),
        )
        .expect("observation modeled")
        .into_event_and_consistency_streams()
        .0;
        let completed = decide_advance_workflow(&[initialized, requested, observed], stream)
            .expect("advance modeled")
            .into_event_and_consistency_streams()
            .0;
        let WorkflowFact::WorkflowCompleted { successor, .. } = completed.fact() else {
            panic!("successful advance must preserve its workflow-owned successor");
        };
        assert_eq!(successor.phase(), HarnessPhase::Ready);
        assert_ne!(
            successor.initial_effect().effect_id(),
            workflow_state().initial_effect().effect_id()
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        clippy::pattern_type_mismatch,
        reason = "the baseline compatibility fixture must expose deserialization or replay rejection exactly"
    )]
    fn baseline_completion_without_successor_replays_with_derived_authority() {
        let predecessor =
            WorkflowStream::for_effect(&initial_effect()).expect("predecessor stream");
        let initialized = decide_initialize_workflow(predecessor.clone(), workflow_state())
            .expect("initialization modeled")
            .into_event_and_consistency_streams()
            .0;
        let requested =
            decide_request_next_effect(slice::from_ref(&initialized), predecessor.clone())
                .expect("request modeled")
                .into_event_and_consistency_streams()
                .0;
        let observed = decide_record_observation(
            &[initialized.clone(), requested.clone()],
            predecessor.clone(),
            success_observation("effect-1"),
        )
        .expect("observation modeled")
        .into_event_and_consistency_streams()
        .0;
        let completed = decide_advance_workflow(
            &[initialized.clone(), requested.clone(), observed.clone()],
            predecessor.clone(),
        )
        .expect("advance modeled")
        .into_event_and_consistency_streams()
        .0;
        let WorkflowFact::WorkflowCompleted { successor, .. } = completed.fact() else {
            panic!("successful advance must produce a successor");
        };
        let expected_successor = successor.clone();
        let current_completion = serde_json::to_value(&completed).expect("completion serializes");
        let serialized_successor = current_completion
            .get("fact")
            .and_then(|fact| fact.get("WorkflowCompleted"))
            .and_then(|fact| fact.get("successor"));
        assert!(
            serialized_successor.is_some_and(serde_json::Value::is_object),
            "new completion facts must persist workflow-owned successor authority"
        );
        let baseline_fact = serde_json::from_str(
            r#"{"WorkflowCompleted":{"state":{"initial_effect":{"agent_id":"agent-1","assignment_epoch":1,"assignment_id":"assignment-1","assignment_scope":"scope-1","attempt_number":1,"context_receipt_id":"context-1","deadline_milliseconds":1000,"effect_id":"effect-1","idempotency_key":"idempotency-1","policy_decision_id":"policy-1","session_id":"session-1","workflow_id":"workflow-1"},"phase":"Completed"},"receipt":"receipt-1"}}"#,
        )
        .expect("the c692eeba completion fact schema remains readable");
        let baseline_completion = WorkflowEvent {
            fact: baseline_fact,
            stream: predecessor.as_stream_id().clone(),
        };
        let history = [initialized, requested, observed, baseline_completion];
        let successor_stream = WorkflowStream::for_effect(expected_successor.initial_effect())
            .expect("successor stream");

        let publication =
            decide_initialize_successor_workflow(&history, predecessor, successor_stream)
                .expect("baseline completion authorizes its deterministic successor");
        assert_eq!(
            publication.event().fact(),
            &WorkflowFact::WorkflowInitialized {
                state: expected_successor,
            }
        );
    }

    #[test]
    fn per_effect_stream_rejects_a_different_same_session_effect() {
        let stream = WorkflowStream::for_effect(&effect_two()).expect("effect stream");

        assert!(decide_initialize_workflow(stream, workflow_state()).is_err());
    }

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
        clippy::single_call_fn,
        reason = "the one-purpose alternate-effect fixture isolates same-session stream rejection"
    )]
    fn effect_two() -> InferEffect {
        let base = initial_effect();
        InferEffect::new(
            base.session_id().clone(),
            base.agent_id().clone(),
            base.workflow_id().clone(),
            base.assignment_id().clone(),
            base.assignment_scope().clone(),
            base.assignment_epoch(),
            base.attempt_number(),
            base.context_receipt_id().clone(),
            base.policy_decision_id().clone(),
            parsed("effect-2", EffectId::parse),
            parsed("idempotency-2", IdempotencyKey::parse),
            base.deadline_milliseconds(),
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
                    successor: HarnessState::new(initial_effect()),
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
                    successor: HarnessState::new(initial_effect()),
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
