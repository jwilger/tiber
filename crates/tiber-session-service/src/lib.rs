//! Durable task-bound conversation authority.
//!
//! Public decisions mint closed publication values from checked `EventCore`
//! commands. Adapters may publish those values but cannot construct arbitrary
//! conversation facts.

#![forbid(unsafe_code)]
#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    reason = "EventCore's ModelEvent derive generates public checked-model helpers at crate scope without an item-local lint hook"
)]
use core::{error::Error, fmt};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tiber_tasks_core::TaskId;
use tiber_workflow_core::{EffectObservation, HarnessState};

/// The single active task-bound conversation stream in one repository authority.
const ACTIVE_SESSION_STREAM: &str = "tiber:session:active";
/// Immutable task and workflow provenance selected when a conversation starts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionBinding {
    /// Native task selected for this conversation.
    task_id: TaskId,
    /// Workflow state whose inference envelope owns session and assignment IDs.
    workflow_state: HarnessState,
}

/// Owner-authored prompt preserved exactly at the durable inference boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptText(String);

/// Exact assistant text returned by one completed inference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AssistantText(String);

impl AssistantText {
    /// Maximum assistant bytes admitted from the app-server protocol.
    pub const MAX_BYTES: usize = 256 * 1024;

    /// Returns the exact accepted assistant text.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses one bounded app-server response while rejecting terminal controls.
    ///
    /// # Errors
    ///
    /// Returns [`AssistantTextError`] when the response is oversized or contains
    /// a control character other than a line feed or tab.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, AssistantTextError> {
        if value.len() > Self::MAX_BYTES {
            return Err(AssistantTextError::TooLarge);
        }
        if value
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
        {
            return Err(AssistantTextError::ControlCharacter);
        }
        Ok(Self(value.to_owned()))
    }
}

#[expect(
    clippy::absolute_paths,
    clippy::missing_inline_in_public_items,
    clippy::missing_trait_methods,
    reason = "Serde's required deserialization signature maps untrusted text through the semantic parser"
)]
impl<'de> Deserialize<'de> for AssistantText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AssistantTextError {
    ControlCharacter,
    TooLarge,
}

impl AssistantTextError {
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ControlCharacter => "session_assistant_control_character",
            Self::TooLarge => "session_assistant_too_large",
        }
    }
}

#[expect(
    clippy::missing_inline_in_public_items,
    clippy::renamed_function_params,
    reason = "the standard display implementation keeps its descriptive formatter parameter"
)]
impl fmt::Display for AssistantTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "semantic assistant-text errors carry no lower-level cause"
)]
impl Error for AssistantTextError {}

impl PromptText {
    /// Returns the exact accepted owner-authored text.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses owner-authored prompt text without changing its contents.
    ///
    /// # Errors
    ///
    /// Returns [`PromptTextError`] when the prompt is empty, oversized, or
    /// contains a terminal control character.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, PromptTextError> {
        if value.is_empty() {
            return Err(PromptTextError::Empty);
        }
        if value.len() > 16 * 1024 {
            return Err(PromptTextError::TooLarge);
        }
        if value.chars().any(char::is_control) {
            return Err(PromptTextError::ControlCharacter);
        }
        Ok(Self(value.to_owned()))
    }
}

#[expect(
    clippy::absolute_paths,
    clippy::missing_inline_in_public_items,
    clippy::missing_trait_methods,
    reason = "Serde's required deserialization signature maps owner text through the semantic parser"
)]
impl<'de> Deserialize<'de> for PromptText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Stable prompt boundary failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PromptTextError {
    ControlCharacter,
    Empty,
    TooLarge,
}

impl PromptTextError {
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ControlCharacter => "session_prompt_control_character",
            Self::Empty => "session_prompt_empty",
            Self::TooLarge => "session_prompt_too_large",
        }
    }
}

#[expect(
    clippy::missing_inline_in_public_items,
    clippy::renamed_function_params,
    reason = "the standard display implementation keeps its descriptive formatter parameter"
)]
impl fmt::Display for PromptTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "semantic prompt errors carry no lower-level cause"
)]
impl Error for PromptTextError {}

impl SessionBinding {
    /// Binds a native task to one ready workflow continuation.
    #[must_use]
    #[inline]
    pub const fn new(task_id: TaskId, workflow_state: HarnessState) -> Self {
        Self {
            task_id,
            workflow_state,
        }
    }

    /// Returns the selected native task.
    #[must_use]
    #[inline]
    pub const fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// Returns the ready workflow continuation carrying complete provenance.
    #[must_use]
    #[inline]
    pub const fn workflow_state(&self) -> &HarnessState {
        &self.workflow_state
    }
}

/// Immutable durable facts for one task-bound conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum SessionFact {
    /// A requested inference was durably closed without fabricating assistant output.
    InferenceInterrupted {
        /// Sanitized typed shell observation for the exact pending effect.
        observation: EffectObservation,
    },
    /// A requested inference completed with observable assistant text.
    InferenceObserved {
        /// Stable effect identity paired with the request.
        effect_id: tiber_workflow_core::EffectId,
        /// Complete assistant text assembled from protocol deltas.
        assistant: AssistantText,
    },
    /// A prompt was durably paired with the workflow's next inference effect.
    InferenceRequested {
        /// Exact active binding for planning mode; absent for ordinary and legacy turns.
        #[serde(default)]
        planning_binding: Option<SessionBinding>,
        /// Complete workflow-owned effect provenance for this inference.
        effect: tiber_workflow_core::InferEffect,
        /// Immediately preceding effect whose completed workflow authorized this turn.
        #[serde(default)]
        predecessor_effect_id: Option<tiber_workflow_core::EffectId>,
        /// Conversational purpose of this inference; absent legacy data is ordinary.
        #[serde(default)]
        mode: InferenceMode,
        /// Exact owner-authored prompt.
        prompt: PromptText,
    },
    /// The owner accepted or cancelled the pending proposed plan.
    PlanDecided {
        /// Exact terminal owner decision.
        decision: PlanDecision,
        /// Effect whose proposed plan was decided.
        effect_id: tiber_workflow_core::EffectId,
    },
    /// The task, session, workflow, and assignment were bound atomically.
    SessionStarted {
        /// Complete immutable binding selected by the application shell.
        binding: SessionBinding,
    },
    /// The prior task-bound conversation ended and a new active task took ownership.
    SessionSucceeded {
        /// Exact binding that previously owned the active-session stream.
        predecessor: SessionBinding,
        /// Complete binding for the successor task conversation.
        binding: SessionBinding,
    },
}

/// Typed conversational purpose for a durable inference request.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "inference mode is a closed durable vocabulary"
)]
pub enum InferenceMode {
    /// Ordinary conversational inference, including all retained legacy requests.
    #[default]
    Ordinary,
    /// Planning-only inference whose text cannot grant mutation or process authority.
    Planning,
}

/// Owner decision for one durable proposed plan.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "plan decisions are a closed owner-authored vocabulary"
)]
pub enum PlanDecision {
    /// The proposal was accepted as conversational guidance only.
    Accepted,
    /// The proposal was cancelled without granting any effect authority.
    Cancelled,
}

/// Read-only restart projection for the active session's latest planning lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PlanRestartState {
    /// A proposed plan awaits an owner decision.
    AwaitingDecision {
        binding: SessionBinding,
        effect: tiber_workflow_core::InferEffect,
        prompt: PromptText,
        proposal: AssistantText,
    },
    /// A planning inference is durably pending.
    AwaitingProposal {
        binding: SessionBinding,
        effect: tiber_workflow_core::InferEffect,
        prompt: PromptText,
    },
    /// The proposed plan reached a terminal owner decision.
    Decided {
        proposal: AssistantText,
        decision: PlanDecision,
    },
    /// No planning lifecycle is retained.
    None,
}

/// Stable identity for one isolated conversational branch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct IsolatedTurnId(String);

impl IsolatedTurnId {
    /// Parses one bounded, nonempty branch identity.
    ///
    /// # Errors
    ///
    /// Returns a stable failure for empty, oversized, or control-bearing text.
    #[inline]
    pub fn parse(value: &str) -> Result<Self, SessionServiceError> {
        if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(SessionServiceError::InvalidIsolatedTurn);
        }
        Ok(Self(value.to_owned()))
    }
}

/// Supported isolated conversational surfaces.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "isolated turn kinds are a closed product vocabulary"
)]
pub enum IsolatedTurnKind {
    /// A brief `/btw` conversational branch.
    Btw,
    /// A `/side` conversational branch.
    Side,
}

/// Branch-unique inference binding anchored to the active session owner.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IsolatedTurnBinding {
    /// Active-session owner anchoring the branch.
    parent: SessionBinding,
    /// Branch-unique inference continuation.
    workflow_state: HarnessState,
}

impl IsolatedTurnBinding {
    /// Returns the exact active-session binding that owns this child branch.
    #[must_use]
    #[inline]
    pub const fn parent(&self) -> &SessionBinding {
        &self.parent
    }

    /// Creates a branch binding whose first effect cannot collide with its parent.
    ///
    /// # Errors
    ///
    /// Returns a stable failure when provenance differs or identities collide.
    #[inline]
    pub fn new(
        parent: SessionBinding,
        workflow_state: HarnessState,
    ) -> Result<Self, SessionServiceError> {
        let candidate = Self {
            parent,
            workflow_state,
        };
        if !isolated_binding_is_valid(&candidate) {
            return Err(SessionServiceError::InvalidIsolatedTurn);
        }
        Ok(candidate)
    }

    /// Returns the branch-local workflow state.
    #[must_use]
    #[inline]
    pub const fn workflow_state(&self) -> &HarnessState {
        &self.workflow_state
    }
}

/// Immutable facts retained only on one isolated child stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "durable isolated facts retain complete typed provenance"
)]
pub enum IsolatedTurnFact {
    /// The isolated branch reached a terminal conversational boundary.
    Closed,
    /// The branch inference ended without fabricated assistant text.
    InferenceInterrupted { observation: EffectObservation },
    /// The branch inference returned bounded assistant text.
    InferenceObserved {
        effect_id: tiber_workflow_core::EffectId,
        assistant: AssistantText,
    },
    /// The branch requested one inference-only effect.
    InferenceRequested {
        effect: tiber_workflow_core::InferEffect,
        prompt: PromptText,
    },
    /// The child stream was bound to one branch-unique workflow.
    Opened {
        binding: IsolatedTurnBinding,
        kind: IsolatedTurnKind,
        turn_id: IsolatedTurnId,
    },
}

/// Durable `EventCore` envelope for one isolated child stream.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
#[non_exhaustive]
pub struct IsolatedTurnEvent {
    /// Immutable isolated child-stream fact.
    fact: IsolatedTurnFact,
    /// Exact child stream.
    stream: StreamId,
}

impl IsolatedTurnEvent {
    #[must_use]
    #[inline]
    pub const fn fact(&self) -> &IsolatedTurnFact {
        &self.fact
    }
    #[must_use]
    #[inline]
    #[expect(
        clippy::same_name_method,
        reason = "the public accessor mirrors EventCore vocabulary"
    )]
    pub const fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

impl Event for IsolatedTurnEvent {
    #[inline]
    fn event_type_name() -> &'static str {
        "TiberIsolatedTurnEvent"
    }
    #[inline]
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[derive(ModelOutput)]
/// Checked read projection consuming every isolated event field.
struct IsolatedTurnEventView {
    /// Projected fact.
    fact: IsolatedTurnFact,
    /// Projected stream.
    stream: StreamId,
}

mapping! { IsolatedEventToViewFact: IsolatedTurnEvent.fact => IsolatedTurnEventView.fact using clone; }
mapping! { IsolatedEventToViewStream: IsolatedTurnEvent.stream => IsolatedTurnEventView.stream using clone; }

impl IsolatedTurnEventView {
    /// Projects one event without granting authority.
    fn from_event(event: &IsolatedTurnEvent) -> Self {
        Self::model_builder()
            .fact(IsolatedEventToViewFact::apply(event))
            .stream(IsolatedEventToViewStream::apply(event))
            .build()
            .into_inner()
    }
}

/// Read-only restart projection for one isolated child stream.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IsolatedTurnRestartState {
    /// An inference request lacks a durable terminal observation.
    AwaitingInference,
    /// The child lifecycle is terminal.
    Closed,
    /// The child is open without a request.
    Open,
    /// A resolved child may be closed.
    ReadyToClose,
}

/// Durable `EventCore` envelope for one conversation fact.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
#[non_exhaustive]
pub struct SessionEvent {
    /// Immutable conversation fact.
    fact: SessionFact,
    /// Repository-local active-session stream.
    stream: StreamId,
}

impl SessionEvent {
    /// Returns the immutable conversation fact.
    #[must_use]
    #[inline]
    pub const fn fact(&self) -> &SessionFact {
        &self.fact
    }

    /// Returns this fact's durable conversation stream.
    #[must_use]
    #[inline]
    #[expect(
        clippy::same_name_method,
        reason = "the public event accessor intentionally mirrors EventCore's trait vocabulary"
    )]
    pub const fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

impl Event for SessionEvent {
    #[inline]
    fn event_type_name() -> &'static str {
        "TiberSessionEvent"
    }

    #[inline]
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Read-side modeled view that consumes every durable event field.
#[derive(ModelOutput)]
struct SessionEventView {
    /// Projected immutable fact.
    fact: SessionFact,
    /// Projected durable stream identity.
    stream: StreamId,
}

mapping! { SessionEventToViewFact: SessionEvent.fact => SessionEventView.fact using clone; }
mapping! { SessionEventToViewStream: SessionEvent.stream => SessionEventView.stream using clone; }

impl SessionEventView {
    /// Returns the projected immutable fact.
    #[must_use]
    const fn fact(&self) -> &SessionFact {
        &self.fact
    }

    /// Projects one durable event without granting write authority.
    #[must_use]
    fn from_event(event: &SessionEvent) -> Self {
        Self::model_builder()
            .fact(SessionEventToViewFact::apply(event))
            .stream(SessionEventToViewStream::apply(event))
            .build()
            .into_inner()
    }

    /// Returns the projected durable stream.
    #[must_use]
    const fn stream(&self) -> &StreamId {
        &self.stream
    }
}

/// Opaque checked fact accepted by the signed Git publication adapter.
pub struct SessionStartPublication {
    /// Exact repository-local consistency stream.
    consistency_streams: [StreamId; 1],
    /// Sole modeled start fact.
    event: SessionEvent,
}

/// Opaque checked successor fact accepted by the signed publication adapter.
pub struct SessionSuccessorPublication {
    /// Exact repository-local consistency stream.
    consistency_streams: [StreamId; 1],
    /// Checked ownership-transfer event.
    event: SessionEvent,
}

impl SessionSuccessorPublication {
    /// Borrows the checked fact for a post-publication immutable projection.
    #[must_use]
    #[inline]
    pub const fn event(&self) -> &SessionEvent {
        &self.event
    }

    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (SessionEvent, [StreamId; 1]) {
        (self.event, self.consistency_streams)
    }
}

/// Opaque checked inference-request fact accepted by the publication adapter.
pub struct InferenceRequestPublication {
    /// Exact repository-local consistency stream.
    consistency_streams: [StreamId; 1],
    /// Checked durable prompt request.
    event: SessionEvent,
}

/// Opaque checked inference-resolution fact accepted by the publication adapter.
pub struct InferenceObservationPublication {
    /// Exact repository-local consistency stream.
    consistency_streams: [StreamId; 1],
    /// Checked durable assistant or interruption observation.
    event: SessionEvent,
}

/// Opaque checked terminal plan decision accepted by the publication adapter.
pub struct PlanDecisionPublication {
    /// Exact singleton stream fenced by the checked command.
    consistency_streams: [StreamId; 1],
    /// Sole modeled terminal plan fact.
    event: SessionEvent,
}

/// Opaque checked acceptance plus ordinary inference request for atomic publication.
pub struct AcceptedPlanInferencePublication {
    /// Exact active-session fence shared by both checked facts.
    consistency_streams: [StreamId; 1],
    /// Accepted decision followed by its ordinary inference request.
    events: [SessionEvent; 2],
}

impl AcceptedPlanInferencePublication {
    /// Transfers both ordered checked facts and their exact stream fence.
    #[must_use]
    #[inline]
    pub fn into_events_and_consistency_streams(self) -> ([SessionEvent; 2], [StreamId; 1]) {
        (self.events, self.consistency_streams)
    }
}

impl PlanDecisionPublication {
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (SessionEvent, [StreamId; 1]) {
        (self.event, self.consistency_streams)
    }
}

/// Defines one command-specific opaque isolated publication token.
macro_rules! isolated_publication {
    ($name:ident) => {
        /// Opaque checked isolated-turn fact accepted by the signed publisher.
        pub struct $name {
            /// Exact child-stream fence.
            consistency_streams: [StreamId; 1],
            /// Sole checked event.
            event: IsolatedTurnEvent,
        }
        impl $name {
            /// Borrows the checked child-stream event without granting write authority.
            #[must_use]
            #[inline]
            pub const fn event(&self) -> &IsolatedTurnEvent {
                &self.event
            }

            #[must_use]
            #[inline]
            pub fn into_event_and_consistency_streams(self) -> (IsolatedTurnEvent, [StreamId; 1]) {
                let view = IsolatedTurnEventView::from_event(&self.event);
                let _complete_projection = (&view.fact, &view.stream);
                (self.event, self.consistency_streams)
            }
        }
    };
}

isolated_publication!(IsolatedTurnOpenPublication);
isolated_publication!(IsolatedTurnRequestPublication);
isolated_publication!(IsolatedTurnObservationPublication);
isolated_publication!(IsolatedTurnClosePublication);

impl InferenceObservationPublication {
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (SessionEvent, [StreamId; 1]) {
        (self.event, self.consistency_streams)
    }
}

impl InferenceRequestPublication {
    /// Transfers the checked event and its exact consistency boundary.
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (SessionEvent, [StreamId; 1]) {
        (self.event, self.consistency_streams)
    }
}

impl SessionStartPublication {
    /// Borrows the checked fact for a post-publication immutable projection.
    #[must_use]
    #[inline]
    pub const fn event(&self) -> &SessionEvent {
        &self.event
    }

    /// Transfers the checked event and its exact consistency boundary.
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(self) -> (SessionEvent, [StreamId; 1]) {
        let projected = SessionEventView::from_event(&self.event);
        let _complete_projection = (projected.fact(), projected.stream());
        (self.event, self.consistency_streams)
    }
}

#[derive(ModelInput)]
/// Modeled origins for transferring the active conversation binding.
struct SucceedSessionIntent {
    /// Binding that currently owns the conversation.
    #[model(origin)]
    predecessor: SessionBinding,
    /// Singleton durable session stream.
    #[model(origin)]
    stream: StreamId,
    /// Binding that will own the conversation next.
    #[model(origin)]
    successor: SessionBinding,
}

#[derive(ModelCommand)]
/// Checked command that transfers one completed binding to its successor.
struct SucceedSession {
    /// Binding expected to own the conversation now.
    predecessor: SessionBinding,
    /// Singleton durable session stream.
    #[stream]
    stream: StreamId,
    /// Binding selected as the successor.
    successor: SessionBinding,
}

mapping! { SucceedIntentToPredecessor: SucceedSessionIntent.predecessor => SucceedSession.predecessor using clone; }
mapping! { SucceedIntentToSuccessor: SucceedSessionIntent.successor => SucceedSession.successor using clone; }
mapping! { SucceedIntentToStream: SucceedSessionIntent.stream => SucceedSession.stream using clone; }

#[derive(ModelState)]
/// Folded history relevant to a session-successor decision.
struct SucceedSessionState {
    /// Current binding reconstructed from retained authority.
    #[model(default)]
    current: Option<SessionBinding>,
    /// Whether retained authority violates the successor chain.
    #[model(default)]
    malformed: bool,
    /// Effect that remains unresolved for the current binding.
    #[model(default)]
    pending_effect: Option<tiber_workflow_core::EffectId>,
    /// Effect identities already consumed by the conversation.
    #[model(default)]
    seen_effect_ids: Vec<tiber_workflow_core::EffectId>,
    /// Idempotency identities already consumed by the conversation.
    #[model(default)]
    seen_idempotency_keys: Vec<tiber_workflow_core::IdempotencyKey>,
}

#[derive(ModelOutput)]
/// Model output consumed to construct a closed successor publication.
struct SucceedSessionDecision {
    /// Binding relinquishing ownership.
    predecessor: SessionBinding,
    /// Binding receiving ownership.
    successor: SessionBinding,
}

mapping! {
    SucceedToDecisionPredecessor:
        (SucceedSession.predecessor, SucceedSession.successor, SucceedSessionState.current, SucceedSessionState.malformed, SucceedSessionState.pending_effect, SucceedSessionState.seen_effect_ids, SucceedSessionState.seen_idempotency_keys) => SucceedSessionDecision.predecessor
        using try validate_predecessor, error = CommandError;
}
mapping! {
    SucceedToDecisionSuccessor: SucceedSession.successor => SucceedSessionDecision.successor
    using try validate_task_binding, error = CommandError;
}
mapping! { SucceedToEventStream: SucceedSession.stream => SessionEvent.stream using clone; }
mapping! {
    SucceedToEventFact:
        (SucceedSessionDecision.predecessor, SucceedSessionDecision.successor) => SessionEvent.fact
        using session_succeeded;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    reason = "EventCore's command trait requires this closed fold/decision shape and supplies no related-stream implementation"
)]
impl ModelCommandLogic for SucceedSession {
    type Event = SessionEvent;
    type State = SucceedSessionState;

    #[expect(
        clippy::pattern_type_mismatch,
        reason = "EventCore supplies borrowed modeled events while the fold deliberately matches their owned semantic facts"
    )]
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        if event.stream_id() != &self.stream {
            next.malformed = true;
        }
        match event.fact() {
            SessionFact::SessionStarted { binding } => {
                if !task_binding_is_valid(binding) || next.current.is_some() {
                    next.malformed = true;
                } else {
                    next.current = Some(binding.clone());
                }
            }
            SessionFact::SessionSucceeded {
                predecessor,
                binding,
            } => {
                if next.pending_effect.is_none()
                    && task_binding_is_valid(predecessor)
                    && task_binding_is_valid(binding)
                    && next.current.as_ref() == Some(predecessor)
                {
                    next.current = Some(binding.clone());
                } else {
                    next.malformed = true;
                }
            }
            SessionFact::InferenceRequested { effect, .. } => {
                if next.pending_effect.is_some()
                    || next.seen_effect_ids.contains(effect.effect_id())
                    || next
                        .seen_idempotency_keys
                        .contains(effect.idempotency_key())
                    || next
                        .current
                        .as_ref()
                        .is_none_or(|binding| !effect_has_binding_provenance(effect, binding))
                {
                    next.malformed = true;
                } else {
                    next.pending_effect = Some(effect.effect_id().clone());
                    next.seen_effect_ids.push(effect.effect_id().clone());
                    next.seen_idempotency_keys
                        .push(effect.idempotency_key().clone());
                }
            }
            SessionFact::InferenceObserved { effect_id, .. } => {
                if next.pending_effect.as_ref() == Some(effect_id) {
                    next.pending_effect = None;
                } else {
                    next.malformed = true;
                }
            }
            SessionFact::InferenceInterrupted { observation } => {
                if next.pending_effect.as_ref() == Some(observation.effect_id()) {
                    next.pending_effect = None;
                } else {
                    next.malformed = true;
                }
            }
            SessionFact::PlanDecided { .. } => {}
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = SucceedSessionDecision::model_builder()
            .predecessor(SucceedToDecisionPredecessor::apply((
                self,
                self,
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            ))?)
            .successor(SucceedToDecisionSuccessor::apply(self)?)
            .build();
        Ok(ModeledEvents::one(
            SessionEvent::model_builder()
                .stream(SucceedToEventStream::apply(self))
                .fact(SucceedToEventFact::apply((
                    decision.as_ref(),
                    decision.as_ref(),
                )))
                .build(),
        ))
    }
}

/// Stable failure produced if a checked `EventCore` session model cannot emit its fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionServiceError {
    /// The isolated turn identity, binding, history, or transition is invalid.
    InvalidIsolatedTurn,
    /// The checked observation model did not emit exactly one fact.
    InvalidModeledInferenceObservation,
    /// The checked model emitted a shape other than one inference request.
    InvalidModeledInferenceRequest,
    /// The checked isolated-turn command emitted an invalid shape.
    InvalidModeledIsolatedTurn,
    /// The checked plan command emitted an invalid shape.
    InvalidModeledPlan,
    /// The successor command emitted an invalid durable shape.
    InvalidModeledSessionSuccessor,
    /// The checked model emitted a shape other than one start fact.
    InvalidModeledStart,
    /// The fixed active-session stream was rejected by `EventCore`.
    InvalidSessionStream,
    /// The checked observation model could not decide from supplied facts.
    ModeledInferenceObservationFailed,
    /// The checked inference-request model could not decide from the supplied facts.
    ModeledInferenceRequestFailed,
    /// Retained history could not authorize the requested planning transition.
    ModeledPlanFailed,
    /// The successor command did not match the current durable binding.
    ModeledSessionSuccessorFailed,
    /// The checked model rejected its single start emission.
    ModeledStartFailed,
    /// A different immutable binding already owns the active-session stream.
    SessionAlreadyStarted,
}

impl SessionServiceError {
    /// Returns the stable owner-facing failure code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidIsolatedTurn => "session_invalid_isolated_turn",
            Self::InvalidModeledIsolatedTurn => "session_invalid_modeled_isolated_turn",
            Self::InvalidModeledPlan => "session_invalid_modeled_plan",
            Self::InvalidModeledInferenceObservation => {
                "session_invalid_modeled_inference_observation"
            }
            Self::InvalidModeledInferenceRequest => "session_invalid_modeled_inference_request",
            Self::InvalidModeledStart => "session_invalid_modeled_start",
            Self::InvalidModeledSessionSuccessor => "session_invalid_modeled_successor",
            Self::InvalidSessionStream => "session_invalid_stream",
            Self::ModeledInferenceObservationFailed => {
                "session_modeled_inference_observation_failed"
            }
            Self::ModeledInferenceRequestFailed => "session_modeled_inference_request_failed",
            Self::ModeledPlanFailed => "session_modeled_plan_failed",
            Self::ModeledSessionSuccessorFailed => "session_modeled_successor_failed",
            Self::ModeledStartFailed => "session_modeled_start_failed",
            Self::SessionAlreadyStarted => "session_already_started",
        }
    }
}

impl fmt::Display for SessionServiceError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the modeled session failure has no lower-level causal source"
)]
impl Error for SessionServiceError {}

/// Typed mismatch at the start-fact projection boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionProjectionError {
    /// Retained planning facts do not form one valid lifecycle.
    InvalidPlanHistory,
    /// The projected fact does not establish a session binding.
    NotStarted,
}

impl SessionProjectionError {
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidPlanHistory => "session_invalid_plan_history",
            Self::NotStarted => "session_fact_not_started",
        }
    }
}

impl fmt::Display for SessionProjectionError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the projection mismatch has no lower-level causal source"
)]
impl Error for SessionProjectionError {}

#[derive(ModelInput)]
/// Modeled origins for one owner-authored inference request.
struct RequestInferenceIntent {
    /// Workflow-owned effect authorized for this turn.
    #[model(origin)]
    effect: tiber_workflow_core::InferEffect,
    /// Typed conversational purpose.
    #[model(origin)]
    mode: InferenceMode,
    /// Exact requested planning owner, absent for ordinary inference.
    #[model(origin)]
    planning_binding: Option<SessionBinding>,
    /// Owner-authored prompt for this turn.
    #[model(origin)]
    prompt: PromptText,
    /// Singleton durable session stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(ModelCommand)]
/// Checked command that pairs the next workflow effect with a prompt.
struct RequestInference {
    /// Workflow-owned effect authorized for this turn.
    effect: tiber_workflow_core::InferEffect,
    /// Typed conversational purpose.
    mode: InferenceMode,
    /// Exact requested planning owner, absent for ordinary inference.
    planning_binding: Option<SessionBinding>,
    /// Owner-authored prompt for this turn.
    prompt: PromptText,
    /// Singleton durable session stream.
    #[stream]
    stream: StreamId,
}

mapping! { RequestInferenceIntentToPrompt: RequestInferenceIntent.prompt => RequestInference.prompt using clone; }
mapping! { RequestInferenceIntentToEffect: RequestInferenceIntent.effect => RequestInference.effect using clone; }
mapping! { RequestInferenceIntentToMode: RequestInferenceIntent.mode => RequestInference.mode using copy; }
mapping! { RequestInferenceIntentToPlanningBinding: RequestInferenceIntent.planning_binding => RequestInference.planning_binding using clone; }
mapping! { RequestInferenceIntentToStream: RequestInferenceIntent.stream => RequestInference.stream using clone; }

#[derive(ModelState)]
/// Folded history relevant to one pending inference request.
struct RequestInferenceState {
    /// Effect from the current session binding.
    #[model(default)]
    base_effect: Option<tiber_workflow_core::InferEffect>,
    /// Current immutable session binding.
    #[model(default)]
    current_binding: Option<SessionBinding>,
    /// Whether retained request authority is malformed.
    #[model(default)]
    malformed: bool,
    /// Whether the current turn still awaits an observation.
    #[model(default)]
    pending: bool,
    /// Identity of the pending effect, when present.
    #[model(default)]
    pending_effect: Option<tiber_workflow_core::EffectId>,
    /// Most recently requested effect, retained after its observation.
    #[model(default)]
    predecessor_effect_id: Option<tiber_workflow_core::EffectId>,
    /// Effect identities already used by the conversation.
    #[model(default)]
    seen_effect_ids: Vec<tiber_workflow_core::EffectId>,
    /// Idempotency identities already used by the conversation.
    #[model(default)]
    seen_idempotency_keys: Vec<tiber_workflow_core::IdempotencyKey>,
}

#[derive(ModelOutput)]
/// Model output consumed to construct a closed inference-request publication.
struct RequestInferenceDecision {
    /// Validated workflow effect paired with the prompt.
    effect: tiber_workflow_core::InferEffect,
    /// Exact preceding effect for every non-initial turn.
    predecessor_effect_id: Option<tiber_workflow_core::EffectId>,
}

mapping! {
    RequestInferenceToDecision:
        (RequestInference.effect, RequestInferenceState.base_effect, RequestInferenceState.seen_effect_ids, RequestInferenceState.seen_idempotency_keys, RequestInferenceState.pending, RequestInferenceState.malformed, RequestInferenceState.current_binding, RequestInferenceState.pending_effect, RequestInference.mode, RequestInference.planning_binding) => RequestInferenceDecision.effect
        using try validate_next_effect, error = CommandError;
}
mapping! {
    RequestInferencePredecessorToDecision:
        RequestInferenceState.predecessor_effect_id => RequestInferenceDecision.predecessor_effect_id
        using clone;
}

mapping! { RequestInferenceToEventStream: RequestInference.stream => SessionEvent.stream using clone; }
mapping! {
    RequestInferenceToEventFact:
        (RequestInference.prompt, RequestInferenceDecision.effect, RequestInferenceDecision.predecessor_effect_id, RequestInference.mode, RequestInference.planning_binding) => SessionEvent.fact
        using inference_requested;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    reason = "EventCore's command trait requires this closed fold/decision shape and supplies no related-stream implementation"
)]
impl ModelCommandLogic for RequestInference {
    type Event = SessionEvent;
    type State = RequestInferenceState;

    #[expect(
        clippy::pattern_type_mismatch,
        reason = "EventCore supplies borrowed modeled events while the fold deliberately matches their owned semantic facts"
    )]
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        if event.stream_id() != &self.stream {
            next.malformed = true;
        }
        if let SessionFact::SessionStarted { binding } = event.fact() {
            if next.current_binding.is_some() || !task_binding_is_valid(binding) {
                next.malformed = true;
            } else {
                next.current_binding = Some(binding.clone());
                next.base_effect = Some(binding.workflow_state().initial_effect().clone());
            }
        }
        if let SessionFact::SessionSucceeded {
            predecessor,
            binding,
        } = event.fact()
        {
            if predecessor == binding
                || next.current_binding.as_ref() != Some(predecessor)
                || !task_binding_is_valid(predecessor)
                || !task_binding_is_valid(binding)
            {
                next.malformed = true;
            } else {
                next.current_binding = Some(binding.clone());
                next.base_effect = Some(binding.workflow_state().initial_effect().clone());
                next.seen_effect_ids.clear();
                next.seen_idempotency_keys.clear();
                next.pending = false;
                next.pending_effect = None;
                next.predecessor_effect_id = None;
            }
        }
        if let SessionFact::InferenceRequested {
            effect,
            predecessor_effect_id,
            ..
        } = event.fact()
        {
            if next.pending
                || predecessor_effect_id != &next.predecessor_effect_id
                || next
                    .current_binding
                    .as_ref()
                    .is_none_or(|binding| !effect_has_binding_provenance(effect, binding))
                || next.seen_effect_ids.contains(effect.effect_id())
                || next
                    .seen_idempotency_keys
                    .contains(effect.idempotency_key())
            {
                next.malformed = true;
            } else {
                next.seen_effect_ids.push(effect.effect_id().clone());
                next.seen_idempotency_keys
                    .push(effect.idempotency_key().clone());
                next.pending = true;
                next.pending_effect = Some(effect.effect_id().clone());
                next.predecessor_effect_id = Some(effect.effect_id().clone());
            }
        }
        if let SessionFact::InferenceObserved { effect_id, .. } = event.fact() {
            if !next.pending || next.pending_effect.as_ref() != Some(effect_id) {
                next.malformed = true;
            } else {
                next.pending = false;
                next.pending_effect = None;
            }
        }
        if let SessionFact::InferenceInterrupted { observation } = event.fact() {
            if !next.pending || next.pending_effect.as_ref() != Some(observation.effect_id()) {
                next.malformed = true;
            } else {
                next.pending = false;
                next.pending_effect = None;
            }
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RequestInferenceDecision::model_builder()
            .effect(RequestInferenceToDecision::apply((
                self,
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                self,
                self,
            ))?)
            .predecessor_effect_id(RequestInferencePredecessorToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            SessionEvent::model_builder()
                .stream(RequestInferenceToEventStream::apply(self))
                .fact(RequestInferenceToEventFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    self,
                    self,
                )))
                .build(),
        ))
    }
}

#[derive(Clone)]
/// Closed representation of one completed or interrupted inference turn.
enum InferenceResolution {
    /// Genuine assistant output returned by the inference provider.
    Completed(AssistantText),
    /// Sanitized typed failure recorded without assistant output.
    Interrupted(EffectObservation),
}

#[derive(ModelInput)]
/// Modeled origins for recording one terminal inference resolution.
struct ObserveInferenceIntent {
    /// Validated terminal response for the pending effect.
    #[model(origin)]
    resolution: InferenceResolution,
    /// Singleton durable session stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(ModelCommand)]
/// Checked command that records one terminal inference resolution.
struct ObserveInference {
    /// Validated terminal response for the pending effect.
    resolution: InferenceResolution,
    /// Singleton durable session stream.
    #[stream]
    stream: StreamId,
}

mapping! { ObserveIntentToResolution: ObserveInferenceIntent.resolution => ObserveInference.resolution using clone; }
mapping! { ObserveIntentToStream: ObserveInferenceIntent.stream => ObserveInference.stream using clone; }

#[derive(ModelState)]
/// Folded history relevant to one pending inference observation.
struct ObserveInferenceState {
    /// Current immutable session binding.
    #[model(default)]
    current_binding: Option<SessionBinding>,
    /// Identity of the pending effect.
    #[model(default)]
    effect_id: Option<tiber_workflow_core::EffectId>,
    /// Whether retained observation authority is malformed.
    #[model(default)]
    malformed: bool,
    /// Whether the pending effect has already been observed.
    #[model(default)]
    observed: bool,
}

#[derive(ModelOutput)]
/// Model output consumed to construct a closed inference-observation publication.
struct ObserveInferenceDecision {
    /// Validated identity of the observed effect.
    effect_id: tiber_workflow_core::EffectId,
}

mapping! { ObserveStateToDecision: (ObserveInferenceState.effect_id, ObserveInferenceState.observed, ObserveInferenceState.malformed, ObserveInferenceState.current_binding) => ObserveInferenceDecision.effect_id using try require_unobserved_effect_id, error = CommandError; }
mapping! { ObserveToEventStream: ObserveInference.stream => SessionEvent.stream using clone; }
mapping! { ObserveToEventFact: (ObserveInference.resolution, ObserveInferenceDecision.effect_id) => SessionEvent.fact using try inference_resolved, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    reason = "EventCore's command trait requires this closed fold/decision shape and supplies no related-stream implementation"
)]
impl ModelCommandLogic for ObserveInference {
    type Event = SessionEvent;
    type State = ObserveInferenceState;

    #[expect(
        clippy::pattern_type_mismatch,
        reason = "EventCore supplies borrowed modeled events while the fold deliberately matches their owned semantic facts"
    )]
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        if event.stream_id() != &self.stream {
            next.malformed = true;
        }
        if let SessionFact::SessionStarted { binding } = event.fact() {
            if next.current_binding.is_some() || !task_binding_is_valid(binding) {
                next.malformed = true;
            } else {
                next.current_binding = Some(binding.clone());
            }
        }
        if let SessionFact::SessionSucceeded {
            predecessor,
            binding,
        } = event.fact()
        {
            next.effect_id = None;
            next.observed = false;
            if predecessor == binding
                || next.current_binding.as_ref() != Some(predecessor)
                || !task_binding_is_valid(predecessor)
                || !task_binding_is_valid(binding)
            {
                next.malformed = true;
            } else {
                next.current_binding = Some(binding.clone());
            }
        }
        if let SessionFact::InferenceRequested { effect, .. } = event.fact() {
            if next.effect_id.is_some() && !next.observed
                || next
                    .current_binding
                    .as_ref()
                    .is_none_or(|binding| !effect_has_binding_provenance(effect, binding))
            {
                next.malformed = true;
            } else {
                next.effect_id = Some(effect.effect_id().clone());
                next.observed = false;
            }
        }
        if let SessionFact::InferenceObserved { effect_id, .. } = event.fact() {
            if next.effect_id.as_ref() != Some(effect_id) || next.observed {
                next.malformed = true;
            } else {
                next.observed = true;
            }
        }
        if let SessionFact::InferenceInterrupted { observation } = event.fact() {
            if next.effect_id.as_ref() != Some(observation.effect_id()) || next.observed {
                next.malformed = true;
            } else {
                next.observed = true;
            }
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ObserveInferenceDecision::model_builder()
            .effect_id(ObserveStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            ))?)
            .build();
        Ok(ModeledEvents::one(
            SessionEvent::model_builder()
                .stream(ObserveToEventStream::apply(self))
                .fact(ObserveToEventFact::apply((self, decision.as_ref()))?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled origins for selecting the initial task-bound conversation binding.
struct StartSessionIntent {
    /// Complete caller origin for the immutable binding.
    #[model(origin)]
    binding: SessionBinding,
    /// Fixed caller origin for the repository-local stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(ModelCommand)]
/// Checked command that starts one active task-bound conversation.
struct StartSession {
    /// Complete immutable binding.
    binding: SessionBinding,
    /// Sole declared stream.
    #[stream]
    stream: StreamId,
}

mapping! { StartSessionIntentToBinding: StartSessionIntent.binding => StartSession.binding using clone; }
mapping! { StartSessionIntentToStream: StartSessionIntent.stream => StartSession.stream using clone; }

#[derive(ModelState)]
/// Folded history relevant to singleton active-session selection.
struct StartSessionState {
    /// Whether retained authority contains more than one immutable binding.
    #[model(default)]
    conflicting: bool,
    /// Binding already recorded on the singleton stream.
    #[model(default)]
    existing: Option<SessionBinding>,
}

#[derive(ModelOutput)]
/// Model output consumed to construct a closed session-start publication.
struct StartSessionDecision {
    /// Binding to emit, absent when the identical start is already durable.
    binding: Option<SessionBinding>,
}

mapping! {
    StartSessionStateToDecision:
        (StartSession.binding, StartSessionState.existing, StartSessionState.conflicting) => StartSessionDecision.binding
        using try decide_singleton_start, error = CommandError;
}
mapping! { StartSessionToEventStream: StartSession.stream => SessionEvent.stream using clone; }
mapping! {
    StartSessionToEventFact:
        StartSessionDecision.binding => SessionEvent.fact
        using try session_started, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    reason = "EventCore's command trait requires this closed fold/decision shape and supplies no related-stream implementation"
)]
impl ModelCommandLogic for StartSession {
    type Event = SessionEvent;
    type State = StartSessionState;

    #[expect(
        clippy::pattern_type_mismatch,
        reason = "EventCore supplies borrowed modeled events while the fold deliberately matches their owned semantic facts"
    )]
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        match event.fact() {
            SessionFact::SessionStarted { binding } => match &next.existing {
                None => next.existing = Some(binding.clone()),
                Some(existing) if existing == binding => {}
                Some(_) => next.conflicting = true,
            },
            SessionFact::SessionSucceeded {
                predecessor,
                binding,
            } => {
                if task_binding_is_valid(predecessor)
                    && task_binding_is_valid(binding)
                    && next.existing.as_ref() == Some(predecessor)
                {
                    next.existing = Some(binding.clone());
                } else {
                    next.conflicting = true;
                }
            }
            SessionFact::InferenceRequested { .. }
            | SessionFact::InferenceObserved { .. }
            | SessionFact::InferenceInterrupted { .. }
            | SessionFact::PlanDecided { .. } => {}
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = StartSessionDecision::model_builder()
            .binding(StartSessionStateToDecision::apply((
                self,
                state.as_ref(),
                state.as_ref(),
            ))?)
            .build();
        if decision.as_ref().binding.is_none() {
            return Ok(ModeledEvents::none("identical session already started"));
        }
        Ok(ModeledEvents::one(
            SessionEvent::model_builder()
                .stream(StartSessionToEventStream::apply(self))
                .fact(StartSessionToEventFact::apply(decision.as_ref())?)
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Owner decision and singleton stream origins.
struct DecidePlanIntent {
    /// Requested terminal decision.
    #[model(origin)]
    decision: PlanDecision,
    /// Active session stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "the private checked command action retains complete typed origins"
)]
/// One closed isolated lifecycle intent.
enum IsolatedTurnAction {
    /// Close a resolved child.
    Close,
    /// Record a non-success observation.
    Interrupt(EffectObservation),
    /// Record bounded assistant text.
    Observe(AssistantText),
    /// Open a branch-unique child.
    Open {
        /// Branch binding.
        binding: IsolatedTurnBinding,
        /// Product surface.
        kind: IsolatedTurnKind,
        /// Stable child identity.
        turn_id: IsolatedTurnId,
    },
    /// Request inference within the child.
    Request {
        /// Branch-owned effect.
        effect: tiber_workflow_core::InferEffect,
        /// Exact owner prompt.
        prompt: PromptText,
    },
}

#[derive(ModelInput)]
/// Modeled origins for one isolated lifecycle transition.
struct IsolatedTurnIntent {
    /// Closed domain action.
    #[model(origin)]
    action: IsolatedTurnAction,
    /// Exact child stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(ModelCommand)]
/// Checked isolated lifecycle command.
struct IsolatedTurnCommand {
    /// Closed domain action.
    action: IsolatedTurnAction,
    /// Exact child stream.
    #[stream]
    stream: StreamId,
}

mapping! { IsolatedIntentToAction: IsolatedTurnIntent.action => IsolatedTurnCommand.action using clone; }
mapping! { IsolatedIntentToStream: IsolatedTurnIntent.stream => IsolatedTurnCommand.stream using clone; }

#[derive(ModelState)]
/// Narrow history needed for one isolated decision.
struct IsolatedTurnState {
    /// Retained branch binding.
    #[model(default)]
    binding: Option<IsolatedTurnBinding>,
    /// Whether the lifecycle is terminal.
    #[model(default)]
    closed: bool,
    /// Retained product surface.
    #[model(default)]
    kind: Option<IsolatedTurnKind>,
    /// Whether retained facts violate the lifecycle.
    #[model(default)]
    malformed: bool,
    /// Exact unresolved effect.
    #[model(default)]
    pending_effect: Option<tiber_workflow_core::EffectId>,
    /// Whether one inference resolved.
    #[model(default)]
    resolved: bool,
    /// Retained child identity.
    #[model(default)]
    turn_id: Option<IsolatedTurnId>,
}

#[derive(ModelOutput)]
/// Optional fact for an emitting or idempotent transition.
struct IsolatedTurnDecision {
    /// Sole checked fact, or none for reconciliation.
    fact: Option<IsolatedTurnFact>,
}

mapping! {
    IsolatedCommandToDecision:
        (IsolatedTurnCommand.action, IsolatedTurnState.binding, IsolatedTurnState.closed, IsolatedTurnState.kind, IsolatedTurnState.malformed, IsolatedTurnState.pending_effect, IsolatedTurnState.resolved, IsolatedTurnState.turn_id) => IsolatedTurnDecision.fact
        using try decide_isolated_fact, error = CommandError;
}
mapping! { IsolatedCommandToEventStream: IsolatedTurnCommand.stream => IsolatedTurnEvent.stream using clone; }
mapping! { IsolatedDecisionToEventFact: IsolatedTurnDecision.fact => IsolatedTurnEvent.fact using try require_isolated_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "the checked isolated lifecycle folds only its child-stream facts"
)]
impl ModelCommandLogic for IsolatedTurnCommand {
    type Event = IsolatedTurnEvent;
    type State = IsolatedTurnState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        if event.stream_id() != &self.stream {
            next.malformed = true;
        }
        match event.fact() {
            IsolatedTurnFact::Opened {
                binding,
                kind,
                turn_id,
            } => {
                if next.binding.is_some() || !isolated_binding_is_valid(binding) {
                    next.malformed = true;
                } else {
                    next.binding = Some(binding.clone());
                    next.kind = Some(*kind);
                    next.turn_id = Some(turn_id.clone());
                }
            }
            IsolatedTurnFact::InferenceRequested { effect, .. } => {
                if next
                    .binding
                    .as_ref()
                    .is_none_or(|binding| !isolated_effect_matches(effect, binding))
                    || next.pending_effect.is_some()
                    || next.resolved
                    || next.closed
                {
                    next.malformed = true;
                } else {
                    next.pending_effect = Some(effect.effect_id().clone());
                    next.resolved = false;
                }
            }
            IsolatedTurnFact::InferenceObserved { effect_id, .. } => {
                if next.pending_effect.as_ref() == Some(effect_id) {
                    next.pending_effect = None;
                    next.resolved = true;
                } else {
                    next.malformed = true;
                }
            }
            IsolatedTurnFact::InferenceInterrupted { observation } => {
                if next.pending_effect.as_ref() != Some(observation.effect_id())
                    || matches!(observation, EffectObservation::Succeeded { .. })
                {
                    next.malformed = true;
                } else {
                    next.pending_effect = None;
                    next.resolved = true;
                }
            }
            IsolatedTurnFact::Closed => {
                if next.binding.is_none()
                    || next.pending_effect.is_some()
                    || !next.resolved
                    || next.closed
                {
                    next.malformed = true;
                } else {
                    next.closed = true;
                }
            }
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = IsolatedTurnDecision::model_builder()
            .fact(IsolatedCommandToDecision::apply((
                self,
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            ))?)
            .build();
        if decision.as_ref().fact.is_none() {
            return Ok(ModeledEvents::none(
                "identical isolated transition retained",
            ));
        }
        Ok(ModeledEvents::one(
            IsolatedTurnEvent::model_builder()
                .stream(IsolatedCommandToEventStream::apply(self))
                .fact(IsolatedDecisionToEventFact::apply(decision.as_ref())?)
                .build(),
        ))
    }
}

#[derive(ModelCommand)]
/// Checked command deciding one proposed plan.
struct DecidePlan {
    /// Requested terminal decision.
    decision: PlanDecision,
    /// Active session stream.
    #[stream]
    stream: StreamId,
}

mapping! { DecidePlanIntentToDecision: DecidePlanIntent.decision => DecidePlan.decision using copy; }
mapping! { DecidePlanIntentToStream: DecidePlanIntent.stream => DecidePlan.stream using clone; }

#[derive(ModelState)]
/// Narrow folded state required to decide one proposed plan.
struct DecidePlanState {
    /// Whether retained planning history violates the lifecycle.
    #[model(default)]
    malformed: bool,
    /// Planning effect awaiting a decision.
    #[model(default)]
    pending_effect: Option<tiber_workflow_core::EffectId>,
    /// Bounded proposed plan text.
    #[model(default)]
    proposal: Option<AssistantText>,
    /// Previously retained terminal decision.
    #[model(default)]
    retained_decision: Option<PlanDecision>,
}

#[derive(ModelOutput)]
/// Optional modeled decision emission and its exact effect identity.
struct DecidePlanOutput {
    /// Decision to emit, or none for an identical retry.
    decision: Option<PlanDecision>,
    /// Effect whose proposed plan is decided.
    effect_id: Option<tiber_workflow_core::EffectId>,
}

mapping! {
    DecidePlanToOutputDecision:
        (DecidePlan.decision, DecidePlanState.malformed, DecidePlanState.pending_effect, DecidePlanState.proposal, DecidePlanState.retained_decision) => DecidePlanOutput.decision
        using try validate_plan_decision, error = CommandError;
}
mapping! { DecidePlanToOutputEffect: DecidePlanState.pending_effect => DecidePlanOutput.effect_id using clone; }
mapping! { DecidePlanToEventStream: DecidePlan.stream => SessionEvent.stream using clone; }
mapping! {
    DecidePlanToEventFact:
        (DecidePlanOutput.decision, DecidePlanOutput.effect_id) => SessionEvent.fact
        using try plan_decided, error = CommandError;
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::else_if_without_else,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "EventCore owns the closed planning decision fold and supplies borrowed generated origins"
)]
impl ModelCommandLogic for DecidePlan {
    type Event = SessionEvent;
    type State = DecidePlanState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut next = state.into_inner();
        if event.stream_id() != &self.stream {
            next.malformed = true;
        }
        match event.fact() {
            SessionFact::InferenceRequested {
                effect,
                mode: InferenceMode::Planning,
                ..
            } => {
                if next.retained_decision.is_some() {
                    next.pending_effect = Some(effect.effect_id().clone());
                    next.proposal = None;
                    next.retained_decision = None;
                } else if next.pending_effect.is_some() {
                    next.malformed = true;
                } else {
                    next.pending_effect = Some(effect.effect_id().clone());
                }
            }
            SessionFact::InferenceObserved {
                effect_id,
                assistant,
            } => {
                if next.pending_effect.as_ref() == Some(effect_id) && next.proposal.is_none() {
                    next.proposal = Some(assistant.clone());
                } else if next.pending_effect.as_ref() == Some(effect_id) {
                    next.malformed = true;
                }
            }
            SessionFact::InferenceInterrupted { observation } => {
                if next.pending_effect.as_ref() == Some(observation.effect_id()) {
                    next.pending_effect = None;
                    next.proposal = None;
                    next.retained_decision = None;
                }
            }
            SessionFact::PlanDecided {
                decision,
                effect_id,
            } => {
                if next.pending_effect.as_ref() == Some(effect_id)
                    && next.proposal.is_some()
                    && next.retained_decision.is_none()
                {
                    next.retained_decision = Some(*decision);
                } else {
                    next.malformed = true;
                }
            }
            _ => {}
        }
        Modeled::from_built(next)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let output = DecidePlanOutput::model_builder()
            .decision(DecidePlanToOutputDecision::apply((
                self,
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            ))?)
            .effect_id(DecidePlanToOutputEffect::apply(state.as_ref()))
            .build();
        if output.as_ref().decision.is_none() {
            return Ok(ModeledEvents::none("identical plan decision retained"));
        }
        Ok(ModeledEvents::one(
            SessionEvent::model_builder()
                .stream(DecidePlanToEventStream::apply(self))
                .fact(DecidePlanToEventFact::apply((
                    output.as_ref(),
                    output.as_ref(),
                ))?)
                .build(),
        ))
    }
}

/// Builds the sole checked start fact for a new repository conversation.
///
/// # Errors
///
/// Returns a stable failure if the fixed stream or modeled emission is invalid.
#[inline]
pub fn decide_start_session(
    history: &[SessionEvent],
    binding: SessionBinding,
) -> Result<Option<SessionStartPublication>, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = StartSessionIntent::model_builder()
        .binding(binding)
        .stream(stream.clone())
        .build();
    let command = StartSession::model_builder()
        .binding(StartSessionIntentToBinding::apply(intent.as_ref()))
        .stream(StartSessionIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| {
            if history.iter().any(|event| {
                matches!(event.fact(), SessionFact::SessionStarted { binding: existing } if existing != &command.as_ref().binding)
            }) {
                SessionServiceError::SessionAlreadyStarted
            } else {
                SessionServiceError::ModeledStartFailed
            }
        })?
        .into();
    if events.is_empty() {
        return Ok(None);
    }
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledStart)?;
    Ok(Some(SessionStartPublication {
        event,
        consistency_streams: [stream],
    }))
}

/// Derives the bounded child stream from durable branch identity.
#[expect(
    clippy::single_call_fn,
    reason = "stream derivation has one checked open boundary"
)]
fn isolated_stream(
    turn_id: &IsolatedTurnId,
    binding: &IsolatedTurnBinding,
) -> Result<StreamId, SessionServiceError> {
    let digest = Sha256::digest(
        format!(
            "{}:{}",
            turn_id.0,
            binding
                .workflow_state
                .initial_effect()
                .session_id()
                .as_str()
        )
        .as_bytes(),
    );
    StreamId::try_new(format!("tiber:session:isolated:{digest:x}"))
        .map_err(|_source| SessionServiceError::InvalidIsolatedTurn)
}

/// Executes one checked isolated lifecycle transition against supplied child history.
fn decide_isolated(
    history: &[IsolatedTurnEvent],
    stream: StreamId,
    action: IsolatedTurnAction,
) -> Result<Option<IsolatedTurnEvent>, SessionServiceError> {
    let intent = IsolatedTurnIntent::model_builder()
        .action(action)
        .stream(stream)
        .build();
    let command = IsolatedTurnCommand::model_builder()
        .action(IsolatedIntentToAction::apply(intent.as_ref()))
        .stream(IsolatedIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<IsolatedTurnEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::InvalidIsolatedTurn)?
        .into();
    if events.is_empty() {
        return Ok(None);
    }
    let [event]: [IsolatedTurnEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledIsolatedTurn)?;
    Ok(Some(event))
}

/// Opens one branch-unique isolated conversational stream.
///
/// # Errors
///
/// Returns a stable failure for invalid binding, stream, or retained history.
#[inline]
pub fn decide_open_isolated_turn(
    history: &[IsolatedTurnEvent],
    turn_id: IsolatedTurnId,
    kind: IsolatedTurnKind,
    binding: IsolatedTurnBinding,
) -> Result<Option<IsolatedTurnOpenPublication>, SessionServiceError> {
    let stream = isolated_stream(&turn_id, &binding)?;
    let event = decide_isolated(
        history,
        stream.clone(),
        IsolatedTurnAction::Open {
            binding,
            kind,
            turn_id,
        },
    )?;
    Ok(event.map(|emitted| IsolatedTurnOpenPublication {
        event: emitted,
        consistency_streams: [stream],
    }))
}

/// Requires one nonempty, single-stream isolated history.
fn retained_isolated_stream(
    history: &[IsolatedTurnEvent],
) -> Result<StreamId, SessionServiceError> {
    let stream = history
        .first()
        .map(|event| event.stream_id().clone())
        .ok_or(SessionServiceError::InvalidIsolatedTurn)?;
    if history.iter().any(|event| event.stream_id() != &stream) {
        return Err(SessionServiceError::InvalidIsolatedTurn);
    }
    Ok(stream)
}

/// Requests one inference-only effect inside an isolated child stream.
///
/// # Errors
///
/// Returns a stable failure unless the child is open and the effect matches its binding.
#[inline]
pub fn decide_request_isolated_turn(
    history: &[IsolatedTurnEvent],
    prompt: PromptText,
    effect: tiber_workflow_core::InferEffect,
) -> Result<IsolatedTurnRequestPublication, SessionServiceError> {
    let stream = retained_isolated_stream(history)?;
    let event = decide_isolated(
        history,
        stream.clone(),
        IsolatedTurnAction::Request { effect, prompt },
    )?
    .ok_or(SessionServiceError::InvalidModeledIsolatedTurn)?;
    Ok(IsolatedTurnRequestPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Records bounded assistant text for the exact pending isolated inference.
///
/// # Errors
///
/// Returns a stable failure unless one exact inference is pending.
#[inline]
pub fn decide_observe_isolated_turn(
    history: &[IsolatedTurnEvent],
    assistant: AssistantText,
) -> Result<IsolatedTurnObservationPublication, SessionServiceError> {
    let stream = retained_isolated_stream(history)?;
    let event = decide_isolated(
        history,
        stream.clone(),
        IsolatedTurnAction::Observe(assistant),
    )?
    .ok_or(SessionServiceError::InvalidModeledIsolatedTurn)?;
    Ok(IsolatedTurnObservationPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Records one sanitized non-success observation for an isolated inference.
///
/// # Errors
///
/// Returns a stable failure for successful, mismatched, or absent observations.
#[inline]
pub fn decide_interrupt_isolated_turn(
    history: &[IsolatedTurnEvent],
    observation: EffectObservation,
) -> Result<IsolatedTurnObservationPublication, SessionServiceError> {
    let stream = retained_isolated_stream(history)?;
    let event = decide_isolated(
        history,
        stream.clone(),
        IsolatedTurnAction::Interrupt(observation),
    )?
    .ok_or(SessionServiceError::InvalidModeledIsolatedTurn)?;
    Ok(IsolatedTurnObservationPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Closes a resolved isolated child stream and reconciles an identical retry.
///
/// # Errors
///
/// Returns a stable failure while inference is pending or the history is malformed.
#[inline]
pub fn decide_close_isolated_turn(
    history: &[IsolatedTurnEvent],
) -> Result<Option<IsolatedTurnClosePublication>, SessionServiceError> {
    let stream = retained_isolated_stream(history)?;
    let event = decide_isolated(history, stream.clone(), IsolatedTurnAction::Close)?;
    Ok(event.map(|emitted| IsolatedTurnClosePublication {
        event: emitted,
        consistency_streams: [stream],
    }))
}

/// Projects isolated child-stream restart state without granting effect authority.
///
/// # Errors
///
/// Returns a stable failure for malformed or mixed-stream retained history.
#[inline]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the projection validates all known child facts and rejects future unsupported facts"
)]
pub fn project_isolated_turn_restart_state(
    history: &[IsolatedTurnEvent],
) -> Result<IsolatedTurnRestartState, SessionServiceError> {
    let _stream = retained_isolated_stream(history)?;
    let mut opened = None;
    let mut pending = None;
    let mut resolved = false;
    let mut closed = false;
    for event in history {
        match event.fact() {
            IsolatedTurnFact::Opened { binding, .. }
                if opened.is_none() && isolated_binding_is_valid(binding) =>
            {
                opened = Some(binding.clone());
            }
            IsolatedTurnFact::InferenceRequested { effect, .. }
                if opened
                    .as_ref()
                    .is_some_and(|binding| isolated_effect_matches(effect, binding))
                    && pending.is_none()
                    && !resolved
                    && !closed =>
            {
                pending = Some(effect.effect_id().clone());
            }
            IsolatedTurnFact::InferenceObserved { effect_id, .. }
                if pending.as_ref() == Some(effect_id) =>
            {
                pending = None;
                resolved = true;
            }
            IsolatedTurnFact::InferenceInterrupted { observation }
                if pending.as_ref() == Some(observation.effect_id())
                    && !matches!(observation, EffectObservation::Succeeded { .. }) =>
            {
                pending = None;
                resolved = true;
            }
            IsolatedTurnFact::Closed
                if opened.is_some() && pending.is_none() && resolved && !closed =>
            {
                closed = true;
            }
            _ => return Err(SessionServiceError::InvalidIsolatedTurn),
        }
    }
    if closed {
        Ok(IsolatedTurnRestartState::Closed)
    } else if pending.is_some() {
        Ok(IsolatedTurnRestartState::AwaitingInference)
    } else if resolved {
        Ok(IsolatedTurnRestartState::ReadyToClose)
    } else if opened.is_some() {
        Ok(IsolatedTurnRestartState::Open)
    } else {
        Err(SessionServiceError::InvalidIsolatedTurn)
    }
}

/// Models transfer of active-session ownership to a successor task binding.
///
/// # Errors
///
/// Returns a stable failure when retained history cannot authorize the transfer.
#[inline]
pub fn decide_succeed_session(
    history: &[SessionEvent],
    predecessor: SessionBinding,
    successor: SessionBinding,
) -> Result<SessionSuccessorPublication, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = SucceedSessionIntent::model_builder()
        .predecessor(predecessor)
        .successor(successor)
        .stream(stream.clone())
        .build();
    let command = SucceedSession::model_builder()
        .predecessor(SucceedIntentToPredecessor::apply(intent.as_ref()))
        .successor(SucceedIntentToSuccessor::apply(intent.as_ref()))
        .stream(SucceedIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::ModeledSessionSuccessorFailed)?
        .into();
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledSessionSuccessor)?;
    Ok(SessionSuccessorPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Models one durable prompt request against the supplied session facts.
///
/// # Errors
///
/// Returns a stable failure when no started session supplies the next inference
/// effect or the checked model does not emit exactly one request.
#[inline]
pub fn decide_request_inference(
    history: &[SessionEvent],
    prompt: PromptText,
    effect: tiber_workflow_core::InferEffect,
) -> Result<InferenceRequestPublication, SessionServiceError> {
    decide_request_inference_mode(history, prompt, effect, InferenceMode::Ordinary, None)
}

/// Implements the shared typed inference-request boundary.
#[inline]
fn decide_request_inference_mode(
    history: &[SessionEvent],
    prompt: PromptText,
    effect: tiber_workflow_core::InferEffect,
    mode: InferenceMode,
    planning_binding: Option<SessionBinding>,
) -> Result<InferenceRequestPublication, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = RequestInferenceIntent::model_builder()
        .prompt(prompt)
        .effect(effect)
        .mode(mode)
        .planning_binding(planning_binding)
        .stream(stream.clone())
        .build();
    let command = RequestInference::model_builder()
        .prompt(RequestInferenceIntentToPrompt::apply(intent.as_ref()))
        .effect(RequestInferenceIntentToEffect::apply(intent.as_ref()))
        .mode(RequestInferenceIntentToMode::apply(intent.as_ref()))
        .planning_binding(RequestInferenceIntentToPlanningBinding::apply(
            intent.as_ref(),
        ))
        .stream(RequestInferenceIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::ModeledInferenceRequestFailed)?
        .into();
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledInferenceRequest)?;
    Ok(InferenceRequestPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Models one completed inference observation against its durable request.
///
/// # Errors
///
/// Returns a stable failure when the observation does not match a pending request.
#[inline]
pub fn decide_observe_inference(
    history: &[SessionEvent],
    assistant: AssistantText,
) -> Result<InferenceObservationPublication, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = ObserveInferenceIntent::model_builder()
        .resolution(InferenceResolution::Completed(assistant))
        .stream(stream.clone())
        .build();
    let command = ObserveInference::model_builder()
        .resolution(ObserveIntentToResolution::apply(intent.as_ref()))
        .stream(ObserveIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::ModeledInferenceObservationFailed)?
        .into();
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledInferenceObservation)?;
    Ok(InferenceObservationPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Models a planning-only inference request bound to the exact active session.
///
/// # Errors
///
/// Returns a stable failure when the supplied binding is not current or the effect is invalid.
#[inline]
pub fn decide_request_plan(
    history: &[SessionEvent],
    binding: SessionBinding,
    prompt: PromptText,
    effect: tiber_workflow_core::InferEffect,
) -> Result<InferenceRequestPublication, SessionServiceError> {
    decide_request_inference_mode(
        history,
        prompt,
        effect,
        InferenceMode::Planning,
        Some(binding),
    )
    .map_err(|_source| SessionServiceError::ModeledPlanFailed)
}

/// Records bounded planning inference output as a proposal awaiting owner decision.
///
/// # Errors
///
/// Returns a stable failure unless one planning inference is durably pending.
#[inline]
pub fn decide_propose_plan(
    history: &[SessionEvent],
    proposal: AssistantText,
) -> Result<InferenceObservationPublication, SessionServiceError> {
    if !matches!(
        project_plan_restart_state(history),
        Ok(PlanRestartState::AwaitingProposal { .. })
    ) {
        return Err(SessionServiceError::ModeledPlanFailed);
    }
    decide_observe_inference(history, proposal)
        .map_err(|_source| SessionServiceError::ModeledPlanFailed)
}

/// Records an accepted proposed plan, reconciling an identical retained decision.
///
/// # Errors
///
/// Returns a stable failure unless a valid proposed plan awaits decision.
#[inline]
pub fn decide_accept_plan(
    history: &[SessionEvent],
) -> Result<Option<PlanDecisionPublication>, SessionServiceError> {
    decide_plan(history, PlanDecision::Accepted)
}

/// Atomically models plan acceptance followed by one ordinary inference request.
///
/// # Errors
///
/// Returns a stable failure unless a proposed plan awaits acceptance and the
/// supplied effect is the next valid ordinary turn.
#[inline]
pub fn decide_accept_plan_and_request_inference(
    history: &[SessionEvent],
    prompt: PromptText,
    effect: tiber_workflow_core::InferEffect,
) -> Result<Option<AcceptedPlanInferencePublication>, SessionServiceError> {
    let Some(accepted) = decide_accept_plan(history)? else {
        let (requested, preceding) = history
            .split_last()
            .ok_or(SessionServiceError::ModeledPlanFailed)?;
        let decided = preceding
            .last()
            .ok_or(SessionServiceError::ModeledPlanFailed)?;
        if matches!(
            (decided.fact(), requested.fact()),
            (
                SessionFact::PlanDecided {
                    decision: PlanDecision::Accepted,
                    ..
                },
                SessionFact::InferenceRequested {
                    effect: retained_effect,
                    prompt: retained_prompt,
                    mode: InferenceMode::Ordinary,
                    ..
                }
            ) if retained_effect == &effect && retained_prompt == &prompt
        ) {
            return Ok(None);
        }
        return Err(SessionServiceError::ModeledPlanFailed);
    };
    let accepted_event = accepted.event;
    let consistency_streams = accepted.consistency_streams;
    let mut accepted_history = history.to_vec();
    accepted_history.push(accepted_event.clone());
    let requested = decide_request_inference(&accepted_history, prompt, effect)?;
    if requested.consistency_streams != consistency_streams {
        return Err(SessionServiceError::InvalidModeledPlan);
    }
    Ok(Some(AcceptedPlanInferencePublication {
        events: [accepted_event, requested.event],
        consistency_streams,
    }))
}

/// Records a cancelled proposed plan, reconciling an identical retained decision.
///
/// # Errors
///
/// Returns a stable failure unless a valid proposed plan awaits decision.
#[inline]
pub fn decide_cancel_plan(
    history: &[SessionEvent],
) -> Result<Option<PlanDecisionPublication>, SessionServiceError> {
    decide_plan(history, PlanDecision::Cancelled)
}

/// Runs the checked terminal plan-decision command.
#[inline]
fn decide_plan(
    history: &[SessionEvent],
    decision: PlanDecision,
) -> Result<Option<PlanDecisionPublication>, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = DecidePlanIntent::model_builder()
        .decision(decision)
        .stream(stream.clone())
        .build();
    let command = DecidePlan::model_builder()
        .decision(DecidePlanIntentToDecision::apply(intent.as_ref()))
        .stream(DecidePlanIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::ModeledPlanFailed)?
        .into();
    if events.is_empty() {
        return Ok(None);
    }
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledPlan)?;
    Ok(Some(PlanDecisionPublication {
        event,
        consistency_streams: [stream],
    }))
}

/// Projects the latest planning lifecycle without granting write or effect authority.
///
/// # Errors
///
/// Returns [`SessionProjectionError::InvalidPlanHistory`] for malformed retained facts.
#[inline]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    reason = "the read projection exhausts planning facts while tolerating unrelated future session facts"
)]
pub fn project_plan_restart_state(
    history: &[SessionEvent],
) -> Result<PlanRestartState, SessionProjectionError> {
    let mut binding = None;
    let mut request = None;
    let mut proposal = None;
    let mut decision = None;
    for event in history {
        match event.fact() {
            SessionFact::SessionStarted { binding: current }
            | SessionFact::SessionSucceeded {
                binding: current, ..
            } => binding = Some(current.clone()),
            SessionFact::InferenceRequested {
                effect,
                prompt,
                mode: InferenceMode::Planning,
                ..
            } => {
                if request.is_some() && decision.is_none() {
                    return Err(SessionProjectionError::InvalidPlanHistory);
                }
                let current = binding
                    .clone()
                    .ok_or(SessionProjectionError::InvalidPlanHistory)?;
                request = Some((current, prompt.clone(), effect.clone()));
                proposal = None;
                decision = None;
            }
            SessionFact::InferenceObserved {
                effect_id,
                assistant,
            } => {
                if request
                    .as_ref()
                    .is_some_and(|(_, _, effect)| effect.effect_id() == effect_id)
                {
                    if proposal.is_some() {
                        return Err(SessionProjectionError::InvalidPlanHistory);
                    }
                    proposal = Some(assistant.clone());
                }
            }
            SessionFact::InferenceInterrupted { observation } => {
                if request
                    .as_ref()
                    .is_some_and(|(_, _, effect)| effect.effect_id() == observation.effect_id())
                {
                    request = None;
                    proposal = None;
                    decision = None;
                }
            }
            SessionFact::PlanDecided {
                effect_id,
                decision: retained,
            } => {
                if request
                    .as_ref()
                    .is_none_or(|(_, _, effect)| effect.effect_id() != effect_id)
                    || proposal.is_none()
                    || decision.is_some()
                {
                    return Err(SessionProjectionError::InvalidPlanHistory);
                }
                decision = Some(*retained);
            }
            _ => {}
        }
    }
    match (request, proposal, decision) {
        (None, None, None) => Ok(PlanRestartState::None),
        (Some((binding, prompt, effect)), None, None) => Ok(PlanRestartState::AwaitingProposal {
            binding,
            effect,
            prompt,
        }),
        (Some((binding, prompt, effect)), Some(proposal), None) => {
            Ok(PlanRestartState::AwaitingDecision {
                binding,
                effect,
                prompt,
                proposal,
            })
        }
        (Some(_), Some(proposal), Some(decision)) => {
            Ok(PlanRestartState::Decided { proposal, decision })
        }
        _ => Err(SessionProjectionError::InvalidPlanHistory),
    }
}

/// Models one sanitized terminal interruption against its durable request.
///
/// # Errors
///
/// Returns a stable failure when the observation is successful, belongs to a
/// different effect, or does not match one pending request.
#[inline]
pub fn decide_interrupt_inference(
    history: &[SessionEvent],
    observation: EffectObservation,
) -> Result<InferenceObservationPublication, SessionServiceError> {
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = ObserveInferenceIntent::model_builder()
        .resolution(InferenceResolution::Interrupted(observation))
        .stream(stream.clone())
        .build();
    let command = ObserveInference::model_builder()
        .resolution(ObserveIntentToResolution::apply(intent.as_ref()))
        .stream(ObserveIntentToStream::apply(intent.as_ref()))
        .build();
    let mut state = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<SessionEvent> = CommandLogic::handle(&command, state)
        .map_err(|_source| SessionServiceError::ModeledInferenceObservationFailed)?
        .into();
    let [event]: [SessionEvent; 1] = events
        .try_into()
        .map_err(|_events| SessionServiceError::InvalidModeledInferenceObservation)?;
    Ok(InferenceObservationPublication {
        event,
        consistency_streams: [stream],
    })
}

/// Projects the complete binding carried by one durable start fact.
///
/// This read-side conversion grants no write authority.
///
/// # Errors
///
/// Returns [`SessionProjectionError::NotStarted`] for a non-start fact.
#[inline]
pub fn project_started_session(
    event: &SessionEvent,
) -> Result<SessionBinding, SessionProjectionError> {
    let view = SessionEventView::from_event(event);
    match view.fact().clone() {
        SessionFact::SessionStarted { binding } | SessionFact::SessionSucceeded { binding, .. } => {
            Ok(binding)
        }
        SessionFact::InferenceRequested { .. }
        | SessionFact::InferenceObserved { .. }
        | SessionFact::InferenceInterrupted { .. }
        | SessionFact::PlanDecided { .. } => Err(SessionProjectionError::NotStarted),
    }
}

/// Derives the bounded assignment scope canonically bound to one native task.
///
/// # Errors
///
/// Returns the typed workflow scope error when the derived scope is invalid.
#[inline]
pub fn task_assignment_scope(
    task_id: &TaskId,
) -> Result<tiber_workflow_core::AssignmentScope, tiber_workflow_core::HarnessError> {
    let digest = Sha256::digest(task_id.as_str().as_bytes());
    tiber_workflow_core::AssignmentScope::parse(&format!("task:{digest:x}"))
}

/// Builds the sole start fact from a validated selected binding.
#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::shadow_reuse,
    reason = "EventCore mapping functions supply borrowed optional origins and require this one-purpose fact mapper"
)]
fn session_started(binding: &Option<SessionBinding>) -> Result<SessionFact, CommandError> {
    let binding = binding
        .as_ref()
        .ok_or_else(|| CommandError::from("session_start_not_emitted"))?;
    Ok(SessionFact::SessionStarted {
        binding: binding.clone(),
    })
}

/// Validates a retained predecessor binding before transferring ownership.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::ref_option,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mappings supply borrowed typed origins to this command-specific predecessor validator"
)]
fn validate_predecessor(
    predecessor: &SessionBinding,
    successor: &SessionBinding,
    current: &Option<SessionBinding>,
    malformed: &bool,
    pending_effect: &Option<tiber_workflow_core::EffectId>,
    seen_effect_ids: &[tiber_workflow_core::EffectId],
    seen_idempotency_keys: &[tiber_workflow_core::IdempotencyKey],
) -> Result<SessionBinding, CommandError> {
    if *malformed
        || pending_effect.is_some()
        || seen_effect_ids.len() != seen_idempotency_keys.len()
        || predecessor == successor
        || seen_effect_ids
            .iter()
            .any(|effect_id| effect_id == successor.workflow_state().initial_effect().effect_id())
        || seen_idempotency_keys.iter().any(|key| {
            key == successor
                .workflow_state()
                .initial_effect()
                .idempotency_key()
        })
    {
        return Err("session_successor_history_malformed".into());
    }
    match current {
        Some(current) if current == predecessor => Ok(predecessor.clone()),
        _ => Err("session_successor_predecessor_mismatch".into()),
    }
}

/// Rejects a binding whose task and assignment scope do not match.
#[expect(
    clippy::single_call_fn,
    reason = "the checked-model mapper has one semantic validation use"
)]
fn validate_task_binding(binding: &SessionBinding) -> Result<SessionBinding, CommandError> {
    if !task_binding_is_valid(binding) {
        return Err("session_task_assignment_scope_mismatch".into());
    }
    Ok(binding.clone())
}

/// Reports whether a binding carries the canonical scope for its task.
fn task_binding_is_valid(binding: &SessionBinding) -> bool {
    matches!(task_assignment_scope(binding.task_id()), Ok(scope) if &scope == binding.workflow_state().initial_effect().assignment_scope())
}

/// Builds the ownership-transfer fact for validated predecessor and successor bindings.
#[expect(
    clippy::single_call_fn,
    reason = "the checked-model mapper has one semantic fact-construction use"
)]
fn session_succeeded(predecessor: &SessionBinding, binding: &SessionBinding) -> SessionFact {
    SessionFact::SessionSucceeded {
        predecessor: predecessor.clone(),
        binding: binding.clone(),
    }
}

/// Selects a new binding or reconciles an identical retained singleton binding.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mappings supply borrowed singleton origins to this command-specific decision"
)]
fn decide_singleton_start(
    candidate: &SessionBinding,
    existing: &Option<SessionBinding>,
    conflicting: &bool,
) -> Result<Option<SessionBinding>, CommandError> {
    if !task_binding_is_valid(candidate) {
        return Err("session_task_assignment_scope_mismatch".into());
    }
    if *conflicting {
        return Err("session_already_started".into());
    }
    match existing {
        None => Ok(Some(candidate.clone())),
        Some(binding) if binding == candidate => Ok(None),
        Some(_) => Err("session_already_started".into()),
    }
}

/// Builds the sole request fact from validated workflow-owned effect provenance.
#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore supplies the optional predecessor origin by reference to the one fact mapper"
)]
fn inference_requested(
    prompt: &PromptText,
    effect: &tiber_workflow_core::InferEffect,
    predecessor_effect_id: &Option<tiber_workflow_core::EffectId>,
    mode: &InferenceMode,
    planning_binding: &Option<SessionBinding>,
) -> SessionFact {
    SessionFact::InferenceRequested {
        effect: effect.clone(),
        predecessor_effect_id: predecessor_effect_id.clone(),
        mode: *mode,
        planning_binding: planning_binding.clone(),
        prompt: prompt.clone(),
    }
}

/// Validates the next workflow effect against retained session authority.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::ref_option,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    reason = "the checked command consumes each independent EventCore origin to validate the next durable effect"
)]
fn validate_next_effect(
    candidate: &tiber_workflow_core::InferEffect,
    base: &Option<tiber_workflow_core::InferEffect>,
    seen: &[tiber_workflow_core::EffectId],
    seen_keys: &[tiber_workflow_core::IdempotencyKey],
    pending: &bool,
    malformed: &bool,
    current_binding: &Option<SessionBinding>,
    pending_effect: &Option<tiber_workflow_core::EffectId>,
    mode: &InferenceMode,
    planning_binding: &Option<SessionBinding>,
) -> Result<tiber_workflow_core::InferEffect, CommandError> {
    if *malformed || current_binding.is_none() || (*pending != pending_effect.is_some()) {
        return Err("session_history_malformed".into());
    }
    match mode {
        InferenceMode::Ordinary if planning_binding.is_some() => {
            return Err("session_ordinary_binding_unexpected".into());
        }
        InferenceMode::Planning if planning_binding.as_ref() != current_binding.as_ref() => {
            return Err("session_plan_binding_mismatch".into());
        }
        InferenceMode::Ordinary | InferenceMode::Planning => {}
    }
    let base = base
        .as_ref()
        .ok_or_else(|| CommandError::from("session_start_required"))?;
    if *pending {
        return Err("session_inference_pending".into());
    }
    if seen
        .iter()
        .any(|effect_id| effect_id == candidate.effect_id())
    {
        return Err("session_inference_effect_reused".into());
    }
    if seen_keys
        .iter()
        .any(|key| key == candidate.idempotency_key())
    {
        return Err("session_inference_idempotency_reused".into());
    }
    let same_assignment = candidate.session_id() == base.session_id()
        && candidate.agent_id() == base.agent_id()
        && candidate.workflow_id() == base.workflow_id()
        && candidate.assignment_id() == base.assignment_id()
        && candidate.assignment_scope() == base.assignment_scope()
        && candidate.assignment_epoch() == base.assignment_epoch()
        && candidate.deadline_milliseconds() == base.deadline_milliseconds();
    if !same_assignment {
        return Err("session_inference_provenance_mismatch".into());
    }
    Ok(candidate.clone())
}

/// Builds the sole observation fact from a durable matching request.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the checked-model mapper matches a borrowed closed resolution and has one semantic fact-construction use"
)]
fn inference_resolved(
    resolution: &InferenceResolution,
    effect_id: &tiber_workflow_core::EffectId,
) -> Result<SessionFact, CommandError> {
    match resolution {
        InferenceResolution::Completed(assistant) => Ok(SessionFact::InferenceObserved {
            effect_id: effect_id.clone(),
            assistant: assistant.clone(),
        }),
        InferenceResolution::Interrupted(observation)
            if observation.effect_id() == effect_id
                && !matches!(observation, EffectObservation::Succeeded { .. }) =>
        {
            Ok(SessionFact::InferenceInterrupted {
                observation: observation.clone(),
            })
        }
        InferenceResolution::Interrupted(_) => {
            Err("session_inference_interruption_mismatch".into())
        }
    }
}

/// Requires a durable request to carry its exact effect identity.
#[expect(
    clippy::ref_option,
    reason = "EventCore supplies this modeled optional origin by reference"
)]
#[expect(
    clippy::single_call_fn,
    reason = "the checked-model mapper has one semantic extraction use"
)]
fn require_effect_id(
    effect_id: &Option<tiber_workflow_core::EffectId>,
) -> Result<tiber_workflow_core::EffectId, CommandError> {
    effect_id
        .clone()
        .ok_or_else(|| CommandError::from("session_start_required"))
}

/// Requires a durable request without a completed observation.
#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore supplies modeled optional and scalar origins by reference"
)]
#[expect(
    clippy::single_call_fn,
    reason = "the checked-model mapper has one semantic guard use"
)]
fn require_unobserved_effect_id(
    effect_id: &Option<tiber_workflow_core::EffectId>,
    observed: &bool,
    malformed: &bool,
    current_binding: &Option<SessionBinding>,
) -> Result<tiber_workflow_core::EffectId, CommandError> {
    if *observed || *malformed || current_binding.is_none() {
        return Err("session_inference_already_observed".into());
    }
    require_effect_id(effect_id)
}

/// Reports whether an effect retains the selected binding's workflow provenance.
fn effect_has_binding_provenance(
    effect: &tiber_workflow_core::InferEffect,
    binding: &SessionBinding,
) -> bool {
    let base = binding.workflow_state().initial_effect();
    effect.session_id() == base.session_id()
        && effect.agent_id() == base.agent_id()
        && effect.workflow_id() == base.workflow_id()
        && effect.assignment_id() == base.assignment_id()
        && effect.assignment_scope() == base.assignment_scope()
        && effect.assignment_epoch() == base.assignment_epoch()
        && effect.attempt_number() == base.attempt_number()
        && effect.deadline_milliseconds() == base.deadline_milliseconds()
}

/// Validates a requested owner decision against the narrow retained plan lifecycle.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore supplies the command-specific planning fold origins by reference"
)]
fn validate_plan_decision(
    requested: &PlanDecision,
    malformed: &bool,
    pending_effect: &Option<tiber_workflow_core::EffectId>,
    proposal: &Option<AssistantText>,
    retained: &Option<PlanDecision>,
) -> Result<Option<PlanDecision>, CommandError> {
    if *malformed || pending_effect.is_none() || proposal.is_none() {
        return Err("session_plan_not_proposed".into());
    }
    match retained {
        None => Ok(Some(*requested)),
        Some(existing) if existing == requested => Ok(None),
        Some(_) => Err("session_plan_already_decided".into()),
    }
}

/// Builds the sole durable terminal plan fact from checked optional output.
#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "modeled generated mappings supply optional outputs by reference for idempotent no-emission"
)]
fn plan_decided(
    decision: &Option<PlanDecision>,
    effect_id: &Option<tiber_workflow_core::EffectId>,
) -> Result<SessionFact, CommandError> {
    Ok(SessionFact::PlanDecided {
        decision: decision.ok_or_else(|| CommandError::from("session_plan_not_emitted"))?,
        effect_id: effect_id
            .clone()
            .ok_or_else(|| CommandError::from("session_plan_effect_missing"))?,
    })
}

/// Validates branch provenance and unique execution identities.
fn isolated_binding_is_valid(binding: &IsolatedTurnBinding) -> bool {
    let parent = binding.parent.workflow_state().initial_effect();
    let branch = binding.workflow_state.initial_effect();
    task_binding_is_valid(&binding.parent)
        && branch.session_id() == parent.session_id()
        && branch.agent_id() == parent.agent_id()
        && branch.workflow_id() == parent.workflow_id()
        && branch.assignment_id() == parent.assignment_id()
        && branch.assignment_scope() == parent.assignment_scope()
        && branch.assignment_epoch() == parent.assignment_epoch()
        && branch.attempt_number() == parent.attempt_number()
        && branch.deadline_milliseconds() == parent.deadline_milliseconds()
        && branch.context_receipt_id() == parent.context_receipt_id()
        && branch.policy_decision_id() == parent.policy_decision_id()
        && branch.effect_id() != parent.effect_id()
        && branch.idempotency_key() != parent.idempotency_key()
}

/// Reports whether an effect is exactly the branch-owned inference request.
fn isolated_effect_matches(
    effect: &tiber_workflow_core::InferEffect,
    binding: &IsolatedTurnBinding,
) -> bool {
    effect == binding.workflow_state.initial_effect()
}

#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    reason = "the checked isolated command consumes each narrow folded lifecycle origin"
)]
/// Selects the sole domain fact for one checked isolated transition.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "EventCore mappings supply borrowed action and optional fold origins"
)]
fn decide_isolated_fact(
    action: &IsolatedTurnAction,
    binding: &Option<IsolatedTurnBinding>,
    closed: &bool,
    kind: &Option<IsolatedTurnKind>,
    malformed: &bool,
    pending_effect: &Option<tiber_workflow_core::EffectId>,
    resolved: &bool,
    turn_id: &Option<IsolatedTurnId>,
) -> Result<Option<IsolatedTurnFact>, CommandError> {
    if *malformed {
        return Err("isolated_turn_history_malformed".into());
    }
    match action {
        IsolatedTurnAction::Open {
            binding: candidate,
            kind: candidate_kind,
            turn_id: candidate_id,
        } => {
            if !isolated_binding_is_valid(candidate) {
                return Err("isolated_turn_binding_invalid".into());
            }
            match (binding, kind, turn_id) {
                (None, None, None) => Ok(Some(IsolatedTurnFact::Opened {
                    binding: candidate.clone(),
                    kind: *candidate_kind,
                    turn_id: candidate_id.clone(),
                })),
                (Some(existing), Some(existing_kind), Some(existing_id))
                    if existing == candidate
                        && existing_kind == candidate_kind
                        && existing_id == candidate_id =>
                {
                    Ok(None)
                }
                _ => Err("isolated_turn_already_open".into()),
            }
        }
        IsolatedTurnAction::Request { effect, prompt } => {
            let owner = binding
                .as_ref()
                .ok_or_else(|| CommandError::from("isolated_turn_not_open"))?;
            if *closed
                || *resolved
                || pending_effect.is_some()
                || !isolated_effect_matches(effect, owner)
            {
                return Err("isolated_turn_request_invalid".into());
            }
            Ok(Some(IsolatedTurnFact::InferenceRequested {
                effect: effect.clone(),
                prompt: prompt.clone(),
            }))
        }
        IsolatedTurnAction::Observe(assistant) => {
            let effect_id = pending_effect
                .clone()
                .ok_or_else(|| CommandError::from("isolated_turn_not_pending"))?;
            Ok(Some(IsolatedTurnFact::InferenceObserved {
                effect_id,
                assistant: assistant.clone(),
            }))
        }
        IsolatedTurnAction::Interrupt(observation) => {
            if pending_effect.as_ref() != Some(observation.effect_id())
                || matches!(observation, EffectObservation::Succeeded { .. })
            {
                return Err("isolated_turn_interruption_invalid".into());
            }
            Ok(Some(IsolatedTurnFact::InferenceInterrupted {
                observation: observation.clone(),
            }))
        }
        IsolatedTurnAction::Close => {
            if *closed {
                return Ok(None);
            }
            if binding.is_none() || pending_effect.is_some() || !*resolved {
                return Err("isolated_turn_close_invalid".into());
            }
            Ok(Some(IsolatedTurnFact::Closed))
        }
    }
}

/// Requires the emitting fact after idempotent no-emission was handled.
#[expect(
    clippy::ref_option,
    clippy::single_call_fn,
    reason = "the modeled optional fact preserves idempotent no-emission"
)]
fn require_isolated_fact(
    fact: &Option<IsolatedTurnFact>,
) -> Result<IsolatedTurnFact, CommandError> {
    fact.clone()
        .ok_or_else(|| CommandError::from("isolated_turn_fact_missing"))
}
