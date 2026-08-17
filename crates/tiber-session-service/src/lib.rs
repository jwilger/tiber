//! Durable task-bound conversation authority.
//!
//! Public decisions mint closed publication values from checked `EventCore`
//! commands. Adapters may publish those values but cannot construct arbitrary
//! conversation facts.

#![forbid(unsafe_code)]
#![expect(
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
use tiber_workflow_core::HarnessState;

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
#[expect(
    clippy::large_enum_variant,
    reason = "the complete durable successor binding must remain directly inspectable as one typed fact"
)]
pub enum SessionFact {
    /// A requested inference completed with observable assistant text.
    InferenceObserved {
        /// Stable effect identity paired with the request.
        effect_id: tiber_workflow_core::EffectId,
        /// Complete assistant text assembled from protocol deltas.
        assistant: AssistantText,
    },
    /// A prompt was durably paired with the workflow's next inference effect.
    InferenceRequested {
        /// Complete workflow-owned effect provenance for this inference.
        effect: tiber_workflow_core::InferEffect,
        /// Immediately preceding effect whose completed workflow authorized this turn.
        #[serde(default)]
        predecessor_effect_id: Option<tiber_workflow_core::EffectId>,
        /// Exact owner-authored prompt.
        prompt: PromptText,
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

/// Opaque checked inference-observation fact accepted by the publication adapter.
pub struct InferenceObservationPublication {
    /// Exact repository-local consistency stream.
    consistency_streams: [StreamId; 1],
    /// Checked durable assistant observation.
    event: SessionEvent,
}

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
    /// The checked observation model did not emit exactly one fact.
    InvalidModeledInferenceObservation,
    /// The checked model emitted a shape other than one inference request.
    InvalidModeledInferenceRequest,
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
    /// The projected fact does not establish a session binding.
    NotStarted,
}

impl SessionProjectionError {
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        "session_fact_not_started"
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
    /// Owner-authored prompt for this turn.
    prompt: PromptText,
    /// Singleton durable session stream.
    #[stream]
    stream: StreamId,
}

mapping! { RequestInferenceIntentToPrompt: RequestInferenceIntent.prompt => RequestInference.prompt using clone; }
mapping! { RequestInferenceIntentToEffect: RequestInferenceIntent.effect => RequestInference.effect using clone; }
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
        (RequestInference.effect, RequestInferenceState.base_effect, RequestInferenceState.seen_effect_ids, RequestInferenceState.seen_idempotency_keys, RequestInferenceState.pending, RequestInferenceState.malformed, RequestInferenceState.current_binding, RequestInferenceState.pending_effect) => RequestInferenceDecision.effect
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
        (RequestInference.prompt, RequestInferenceDecision.effect, RequestInferenceDecision.predecessor_effect_id) => SessionEvent.fact
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
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
/// Modeled origins for recording one completed inference observation.
struct ObserveInferenceIntent {
    /// Validated assistant response for the pending effect.
    #[model(origin)]
    assistant: AssistantText,
    /// Singleton durable session stream.
    #[model(origin)]
    stream: StreamId,
}

#[derive(ModelCommand)]
/// Checked command that records one assistant observation.
struct ObserveInference {
    /// Validated assistant response for the pending effect.
    assistant: AssistantText,
    /// Singleton durable session stream.
    #[stream]
    stream: StreamId,
}

mapping! { ObserveIntentToAssistant: ObserveInferenceIntent.assistant => ObserveInference.assistant using clone; }
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
mapping! { ObserveToEventFact: (ObserveInference.assistant, ObserveInferenceDecision.effect_id) => SessionEvent.fact using inference_observed; }

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
                .fact(ObserveToEventFact::apply((self, decision.as_ref())))
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
            SessionFact::InferenceRequested { .. } | SessionFact::InferenceObserved { .. } => {}
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

/// Builds the sole checked start fact for a new repository conversation.
///
/// # Errors
///
/// Returns a stable failure if the fixed `EventCore` stream or checked emission
/// cannot be constructed.
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
    let stream = StreamId::try_new(ACTIVE_SESSION_STREAM.to_owned())
        .map_err(|_source| SessionServiceError::InvalidSessionStream)?;
    let intent = RequestInferenceIntent::model_builder()
        .prompt(prompt)
        .effect(effect)
        .stream(stream.clone())
        .build();
    let command = RequestInference::model_builder()
        .prompt(RequestInferenceIntentToPrompt::apply(intent.as_ref()))
        .effect(RequestInferenceIntentToEffect::apply(intent.as_ref()))
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
        .assistant(assistant)
        .stream(stream.clone())
        .build();
    let command = ObserveInference::model_builder()
        .assistant(ObserveIntentToAssistant::apply(intent.as_ref()))
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
        SessionFact::InferenceRequested { .. } | SessionFact::InferenceObserved { .. } => {
            Err(SessionProjectionError::NotStarted)
        }
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
    clippy::shadow_reuse,
    clippy::single_call_fn,
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
    reason = "EventCore supplies the optional predecessor origin by reference to the one fact mapper"
)]
fn inference_requested(
    prompt: &PromptText,
    effect: &tiber_workflow_core::InferEffect,
    predecessor_effect_id: &Option<tiber_workflow_core::EffectId>,
) -> SessionFact {
    SessionFact::InferenceRequested {
        effect: effect.clone(),
        predecessor_effect_id: predecessor_effect_id.clone(),
        prompt: prompt.clone(),
    }
}

/// Validates the next workflow effect against retained session authority.
#[expect(
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
) -> Result<tiber_workflow_core::InferEffect, CommandError> {
    if *malformed || current_binding.is_none() || (*pending != pending_effect.is_some()) {
        return Err("session_history_malformed".into());
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
    clippy::single_call_fn,
    reason = "the checked-model mapper has one semantic fact-construction use"
)]
fn inference_observed(
    assistant: &AssistantText,
    effect_id: &tiber_workflow_core::EffectId,
) -> SessionFact {
    SessionFact::InferenceObserved {
        effect_id: effect_id.clone(),
        assistant: assistant.clone(),
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
