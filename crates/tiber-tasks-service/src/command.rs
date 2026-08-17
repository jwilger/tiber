//! Command-specific, pure decisions for native Tiber Tasks.
//!
//! These folds consume canonical transaction-order task facts supplied by an
//! adapter. They deliberately do not reuse [`crate::TaskBoardProjection`],
//! whose broad state is query-only rather than write authority.

#[path = "task_administration.rs"]
mod task_administration;

use alloc::{collections::BTreeMap, vec::Vec};
use core::{error::Error, fmt};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{
    ChecklistItem, Subtask, TaskAcceptanceChecked, TaskAcceptanceRemoved, TaskCreated, TaskEvent,
    TaskId, TaskOrder, TaskStatus, TaskSubtaskChecked, TaskSubtaskIdCorrected,
    TaskSubtaskOccurrenceChecked, TaskTransitioned,
};

use crate::{
    AcceptanceCheckPublication, SubtaskIdCorrectionPublication, SubtaskOccurrenceCheckPublication,
    TaskActivationPublication, TaskCompletionPublication,
};

/// The native board stream receiving task-mutation facts.
pub const TASK_BOARD_STREAM: &str = "tiber:board";

/// Request to create one backlog task from owner-supplied title and adapter-assigned metadata.
pub type CreateTask = task_administration::CreateTask;

/// Closed result of one task-creation decision.
pub type TaskCreationDecision = task_administration::TaskCreationDecision;

/// A zero-based durable acceptance-item position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptanceIndex(usize);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the semantic index API presents construction, human-boundary parsing, then durable inspection"
)]
impl AcceptanceIndex {
    /// Constructs a durable zero-based position.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the small semantic index constructor reads clearly as its final value"
    )]
    pub const fn zero_based(index: usize) -> Self {
        Self(index)
    }

    /// Parses a human-facing one-based acceptance position.
    ///
    /// # Errors
    ///
    /// Returns [`TaskCommandError::InvalidAcceptanceIndex`] when the text is
    /// empty, not an unsigned integer, or denotes zero.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded integer parser preserves the input error at its semantic boundary"
    )]
    pub fn parse_one_based(input: &str) -> Result<Self, TaskCommandError> {
        let parsed = input
            .parse::<usize>()
            .map_err(|_source| TaskCommandError::InvalidAcceptanceIndex)?;
        let index = parsed
            .checked_sub(1)
            .ok_or(TaskCommandError::InvalidAcceptanceIndex)?;
        Ok(Self(index))
    }

    /// Returns the durable zero-based position encoded in task facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied semantic position reads clearly as its final expression"
    )]
    pub const fn zero_based_value(self) -> usize {
        self.0
    }
}

/// A zero-based durable subtask occurrence position.
///
/// Unlike a subtask identifier, an occurrence remains unambiguous while a
/// retained task contains a malformed duplicate identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubtaskOccurrence(usize);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the semantic occurrence API presents construction, human-boundary parsing, then durable inspection"
)]
impl SubtaskOccurrence {
    /// Constructs a durable zero-based occurrence position.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the small semantic occurrence constructor reads clearly as its final value"
    )]
    pub const fn zero_based(index: usize) -> Self {
        Self(index)
    }

    /// Parses a human-facing one-based subtask occurrence.
    ///
    /// # Errors
    ///
    /// Returns [`TaskCommandError::InvalidSubtaskOccurrence`] when the text is
    /// empty, not an unsigned integer, or denotes zero.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded integer parser preserves the input error at its semantic boundary"
    )]
    pub fn parse_one_based(input: &str) -> Result<Self, TaskCommandError> {
        let parsed = input
            .parse::<usize>()
            .map_err(|_source| TaskCommandError::InvalidSubtaskOccurrence)?;
        let index = parsed
            .checked_sub(1)
            .ok_or(TaskCommandError::InvalidSubtaskOccurrence)?;
        Ok(Self(index))
    }

    /// Returns the durable zero-based occurrence encoded in task facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied semantic occurrence is clearest as its final accessor expression"
    )]
    pub const fn zero_based_value(self) -> usize {
        self.0
    }
}

/// A canonical non-empty replacement identity for one corrected subtask occurrence.
///
/// This semantic boundary removes surrounding whitespace so identical command
/// invocations address the same durable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskReplacementId(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the semantic identity API presents human-boundary parsing before durable inspection"
)]
impl SubtaskReplacementId {
    /// Parses and canonicalizes a human-facing replacement subtask identity.
    ///
    /// # Errors
    ///
    /// Returns [`TaskCommandError::InvalidSubtaskReplacementId`] when the
    /// replacement is empty after trimming or contains a control character.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic replacement parser is clearest as its final canonical construction"
    )]
    pub fn parse(input: &str) -> Result<Self, TaskCommandError> {
        let replacement_id = input.trim();
        if replacement_id.is_empty() || replacement_id.chars().any(char::is_control) {
            return Err(TaskCommandError::InvalidSubtaskReplacementId);
        }
        Ok(Self(replacement_id.to_owned()))
    }

    /// Returns the canonical durable replacement identity.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed canonical replacement identity is clearest as its final accessor expression"
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The request to check one task acceptance item.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the request fields follow task and position decision flow"
)]
pub struct CheckAcceptance {
    /// The exact task whose current canonical checklist is addressed.
    task: TaskId,
    /// The durable checklist position.
    index: AcceptanceIndex,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the request API presents construction then semantic task and position accessors"
)]
impl CheckAcceptance {
    /// Creates a semantic acceptance-check request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable task-command request is clearest as its final constructed value"
    )]
    pub fn new(task: TaskId, index: AcceptanceIndex) -> Self {
        Self { task, index }
    }

    /// Returns the exact task identity addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed task identity is clearest as its final accessor expression"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the addressed durable acceptance position.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied semantic index is clearest as its final accessor expression"
    )]
    pub const fn index(&self) -> AcceptanceIndex {
        self.index
    }
}

/// The request to correct one malformed duplicate subtask identity.
///
/// The complete expected subtask is part of the request rather than a display
/// hint. It makes the command fail closed if canonical history changed at the
/// addressed occurrence after a caller inspected it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the correction request fields follow its task, exact occurrence, preimage, and replacement decision flow"
)]
pub struct RepairDuplicateSubtaskId {
    /// The exact task whose canonical subtask list is addressed.
    task: TaskId,
    /// The durable occurrence position, not an ambiguous legacy identifier.
    occurrence: SubtaskOccurrence,
    /// Exact current subtask required before correction.
    expected: Subtask,
    /// New unique identity for that occurrence.
    replacement_id: SubtaskReplacementId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the correction request API presents construction then semantic accessors in command data-flow order"
)]
impl RepairDuplicateSubtaskId {
    /// Creates a semantic duplicate-subtask correction request.
    #[inline]
    #[must_use]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable correction request is clearest as its final semantic construction"
    )]
    pub fn new(
        task: TaskId,
        occurrence: SubtaskOccurrence,
        expected: Subtask,
        replacement_id: SubtaskReplacementId,
    ) -> Self {
        Self {
            task,
            occurrence,
            expected,
            replacement_id,
        }
    }

    /// Returns the exact task identity addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed task identity is clearest as its final accessor expression"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the exact durable subtask occurrence addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied occurrence is clearest as its final accessor expression"
    )]
    pub const fn occurrence(&self) -> SubtaskOccurrence {
        self.occurrence
    }

    /// Returns the complete current preimage required by this correction.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed exact preimage is clearest as its final accessor expression"
    )]
    pub const fn expected(&self) -> &Subtask {
        &self.expected
    }

    /// Returns the replacement identity for the exact addressed occurrence.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed semantic replacement identifier is clearest as its final accessor expression"
    )]
    pub fn replacement_id(&self) -> &str {
        self.replacement_id.as_str()
    }
}

/// The request to check one exact current subtask occurrence.
///
/// The complete expected subtask is an immutable preimage, not display input:
/// a changed identifier, prerequisite, title, or check state must not turn an
/// occurrence-addressed request into a different durable mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the occurrence-check request fields follow task, exact occurrence, then immutable preimage decision flow"
)]
pub struct CheckSubtaskOccurrence {
    /// The exact task whose canonical subtask list is addressed.
    task: TaskId,
    /// The durable occurrence position, never a potentially duplicate legacy identifier.
    occurrence: SubtaskOccurrence,
    /// Exact current unchecked subtask required before checking it.
    expected: Subtask,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the occurrence-check request API presents construction then semantic accessors in command data-flow order"
)]
impl CheckSubtaskOccurrence {
    /// Creates a semantic exact-subtask-occurrence check request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable exact-occurrence request is clearest as its final semantic construction"
    )]
    pub fn new(task: TaskId, occurrence: SubtaskOccurrence, expected: Subtask) -> Self {
        Self {
            task,
            occurrence,
            expected,
        }
    }

    /// Returns the exact task identity addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed task identity is clearest as its final accessor expression"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the exact durable subtask occurrence addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied occurrence is clearest as its final accessor expression"
    )]
    pub const fn occurrence(&self) -> SubtaskOccurrence {
        self.occurrence
    }

    /// Returns the complete current preimage required by this occurrence check.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed exact preimage is clearest as its final accessor expression"
    )]
    pub const fn expected(&self) -> &Subtask {
        &self.expected
    }
}

/// The request to complete one task after all current requirements are checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteTask {
    /// The exact task whose lifecycle and strict board entry are addressed.
    task: TaskId,
}

/// The request to activate one strict-next eligible backlog task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartTask {
    /// The exact task whose backlog lifecycle is addressed.
    task: TaskId,
}

impl StartTask {
    /// Creates a semantic task-activation request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable activation request is clearest as its final semantic construction"
    )]
    pub fn new(task: TaskId) -> Self {
        Self { task }
    }

    /// Returns the exact task identity addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed task identity is clearest as its final accessor expression"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }
}

impl CompleteTask {
    /// Creates a semantic task-completion request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable completion request is clearest as its final semantic construction"
    )]
    pub fn new(task: TaskId) -> Self {
        Self { task }
    }

    /// Returns the exact task identity addressed by this request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed task identity is clearest as its final accessor expression"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }
}

/// Modeled external intent for one check-only acceptance command.
#[derive(ModelInput)]
struct CheckAcceptanceIntent {
    /// Board stream that receives the durable mutation.
    #[model(origin)]
    board_stream: StreamId,
    /// Exact current acceptance position selected by the parsed command boundary.
    #[model(origin)]
    index: AcceptanceIndex,
    /// Exact task selected by the parsed command boundary.
    #[model(origin)]
    task: TaskId,
    /// Legacy task stream whose version also fences the decision.
    #[model(origin)]
    task_stream: StreamId,
}

/// Checked `EventCore` command that produces exactly one internal acceptance fact.
#[derive(ModelCommand)]
struct ModeledCheckAcceptance {
    /// Board stream that receives the durable mutation.
    #[stream]
    board_stream: StreamId,
    /// Exact current acceptance position selected by the parsed command boundary.
    index: AcceptanceIndex,
    /// Exact task selected by the parsed command boundary.
    task: TaskId,
    /// Legacy task stream read for optimistic consistency.
    #[stream]
    task_stream: StreamId,
}

mapping! {
    CheckAcceptanceIntentToBoardStream:
        CheckAcceptanceIntent.board_stream => ModeledCheckAcceptance.board_stream
        using clone;
}

mapping! {
    CheckAcceptanceIntentToTaskStream:
        CheckAcceptanceIntent.task_stream => ModeledCheckAcceptance.task_stream
        using clone;
}

mapping! {
    CheckAcceptanceIntentToTask:
        CheckAcceptanceIntent.task => ModeledCheckAcceptance.task
        using clone;
}

mapping! {
    CheckAcceptanceIntentToIndex:
        CheckAcceptanceIntent.index => ModeledCheckAcceptance.index
        using copy;
}

/// Internal modeled fact from which the opaque durable publication token is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledAcceptanceChecked {
    /// Closed check-only state; this command never emits `false`.
    checked: bool,
    /// Zero-based durable acceptance position.
    index: usize,
    /// Board stream receiving the durable acceptance check.
    stream: StreamId,
    /// Task that owns the checked item.
    task: TaskId,
}

#[expect(
    clippy::implicit_return,
    reason = "the EventCore trait names the event type after its stream accessor, and both direct accessors remain clearest as final expressions"
)]
impl Event for ModeledAcceptanceChecked {
    fn event_type_name() -> &'static str {
        "TiberModeledAcceptanceChecked"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled fact field for provenance checking.
#[derive(ModelOutput)]
struct ModeledAcceptanceCheckedView {
    /// Closed check-only state.
    checked: bool,
    /// Zero-based durable acceptance position.
    index: usize,
    /// Board stream that received the durable acceptance check.
    stream: StreamId,
    /// Task that owns the checked item.
    task: TaskId,
}

impl ModeledAcceptanceCheckedView {
    /// Projects every modeled event field into the durable-fact boundary shape.
    #[expect(
        clippy::implicit_return,
        reason = "the modeled view construction is clearest as the final checked builder expression"
    )]
    fn from_event(event: &ModeledAcceptanceChecked) -> Self {
        Self::model_builder()
            .stream(ModeledAcceptanceCheckedToViewStream::apply(event))
            .task(ModeledAcceptanceCheckedToViewTask::apply(event))
            .index(ModeledAcceptanceCheckedToViewIndex::apply(event))
            .checked(ModeledAcceptanceCheckedToViewChecked::apply(event))
            .build()
            .into_inner()
    }

    /// Converts the modeled fact view into the one retained task-fact variant.
    #[expect(
        clippy::implicit_return,
        reason = "the closed modeled projection converts directly into the sole durable fact authorized by this command"
    )]
    fn into_task_acceptance_checked(self) -> TaskAcceptanceChecked {
        TaskAcceptanceChecked::new(self.stream, self.task, self.index, self.checked)
    }
}

mapping! {
    ModeledAcceptanceCheckedToViewStream:
        ModeledAcceptanceChecked.stream => ModeledAcceptanceCheckedView.stream
        using clone;
}

mapping! {
    ModeledAcceptanceCheckedToViewTask:
        ModeledAcceptanceChecked.task => ModeledAcceptanceCheckedView.task
        using clone;
}

mapping! {
    ModeledAcceptanceCheckedToViewIndex:
        ModeledAcceptanceChecked.index => ModeledAcceptanceCheckedView.index
        using copy;
}

mapping! {
    ModeledAcceptanceCheckedToViewChecked:
        ModeledAcceptanceChecked.checked => ModeledAcceptanceCheckedView.checked
        using copy;
}

/// Minimal modeled state for exactly-once emission within one `EventCore` command execution.
#[derive(ModelState)]
struct ModeledCheckAcceptanceState {
    /// Whether this modeled command already emitted its one fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the check-only fact constructor.
#[derive(ModelOutput)]
struct ModeledCheckAcceptanceDecision {
    /// Whether the command had already emitted its one fact.
    emitted: bool,
}

mapping! {
    ModeledCheckAcceptanceStateToDecision:
        ModeledCheckAcceptanceState.emitted => ModeledCheckAcceptanceDecision.emitted
        using copy;
}

mapping! {
    ModeledCheckAcceptanceToFactStream:
        ModeledCheckAcceptance.board_stream => ModeledAcceptanceChecked.stream
        using clone;
}

mapping! {
    ModeledCheckAcceptanceToFactTask:
        ModeledCheckAcceptance.task => ModeledAcceptanceChecked.task
        using clone;
}

mapping! {
    ModeledCheckAcceptanceToFactIndex:
        ModeledCheckAcceptance.index => ModeledAcceptanceChecked.index
        using durable_acceptance_index;
}

mapping! {
    ModeledCheckAcceptanceDecisionToFactChecked:
        ModeledCheckAcceptanceDecision.emitted => ModeledAcceptanceChecked.checked
        using try checked_once, error = CommandError;
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "the EventCore trait fixes the evolve/decide API and uses a default stream-discovery method; modeled event construction is the checked terminal expression"
)]
impl ModelCommandLogic for ModeledCheckAcceptance {
    type Event = ModeledAcceptanceChecked;
    type State = ModeledCheckAcceptanceState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledCheckAcceptanceDecision::model_builder()
            .emitted(ModeledCheckAcceptanceStateToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledAcceptanceChecked::model_builder()
                .stream(ModeledCheckAcceptanceToFactStream::apply(self))
                .task(ModeledCheckAcceptanceToFactTask::apply(self))
                .index(ModeledCheckAcceptanceToFactIndex::apply(self))
                .checked(ModeledCheckAcceptanceDecisionToFactChecked::apply(
                    decision.as_ref(),
                )?)
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = ModeledAcceptanceCheckedView::from_event(event).checked;
        Modeled::from_built(folded)
    }
}

/// Stable failures from a narrow native task-command decision.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "the closed error set follows input, retained-history, and command-decision flow"
)]
pub enum TaskCommandError {
    /// A human-facing acceptance index was not a positive integer.
    InvalidAcceptanceIndex,
    /// A human-facing subtask occurrence was not a positive integer.
    InvalidSubtaskOccurrence,
    /// A replacement subtask identifier was empty or contained a control character.
    InvalidSubtaskReplacementId,
    /// The requested task is not currently active and therefore cannot receive a new mutation.
    TaskNotInProgress {
        /// The task whose current lifecycle blocks the request.
        task: TaskId,
        /// Current durable lifecycle state.
        status: TaskStatus,
    },
    /// An activation request addressed a task that is not currently queued.
    TaskActivationNotBacklog {
        /// The task whose current lifecycle blocks activation.
        task: TaskId,
        /// Current durable lifecycle state.
        status: TaskStatus,
    },
    /// An activation request found one different active task already in progress.
    TaskActivationActiveTask {
        /// The sole task that must be continued before another task can start.
        active_task: TaskId,
    },
    /// Retained history currently contains more than one active task.
    MultipleActiveTasks {
        /// Active task identities in stable identity order.
        active_tasks: Vec<TaskId>,
    },
    /// A requested backlog task has a missing or nonterminal blocker.
    TaskActivationBlocked {
        /// The queued task that cannot start yet.
        task: TaskId,
        /// The first stable unresolved blocker identity.
        blocker: TaskId,
    },
    /// A requested backlog task is not the strict first eligible board task.
    TaskActivationNotNextEligible {
        /// The requested task that cannot bypass board priority.
        task: TaskId,
        /// The first current eligible backlog task in strict board order.
        next: TaskId,
    },
    /// The addressed task is absent or duplicated in the current strict board order.
    TaskActivationOrderDrift {
        /// The task whose exact board membership is malformed.
        task: TaskId,
    },
    /// A fact needed to decide activation has no valid current task/board interpretation.
    TaskActivationMalformedHistory,
    /// The current strict board order cannot authorize task creation.
    TaskCreationMalformedHistory,
    /// The requested task has no creation fact in the supplied canonical history.
    TaskMissing {
        /// The absent task identity.
        task: TaskId,
    },
    /// Retained task facts repeated a task creation.
    DuplicateTaskCreation {
        /// The duplicate task identity.
        task: TaskId,
    },
    /// A retained target task fact came from a stream this command does not fence.
    TargetTaskFactUnexpectedStream {
        /// The target task addressed by the malformed retained fact.
        task: TaskId,
        /// The non-canonical stream that supplied the retained fact.
        stream: StreamId,
    },
    /// Retained acceptance mutations referenced an absent position.
    HistoryAcceptanceItemMissing {
        /// The task owning the malformed checklist fact.
        task: TaskId,
        /// The zero-based missing position in the historical fact.
        index: usize,
    },
    /// Retained subtask mutations referenced an absent subtask identifier.
    HistorySubtaskMissing {
        /// The task owning the malformed subtask fact.
        task: TaskId,
        /// The missing identifier in the historical fact.
        subtask: String,
    },
    /// A retained correction's complete preimage did not match its occurrence.
    HistorySubtaskCorrectionPreimageMismatch {
        /// The task owning the malformed correction fact.
        task: TaskId,
        /// The zero-based malformed correction occurrence.
        index: usize,
    },
    /// A retained correction targeted an identifier that was not duplicated at that point in history.
    HistorySubtaskIdNotDuplicate {
        /// The task owning the malformed correction fact.
        task: TaskId,
        /// The non-duplicate identifier that the malformed correction attempted to repair.
        subtask: String,
    },
    /// A retained exact occurrence check's complete preimage did not match current history.
    HistorySubtaskOccurrenceCheckPreimageMismatch {
        /// The task owning the malformed occurrence fact.
        task: TaskId,
        /// The zero-based malformed occurrence position.
        index: usize,
    },
    /// The requested current checklist position is absent.
    AcceptanceItemMissing {
        /// The task owning the absent current checklist position.
        task: TaskId,
        /// The zero-based absent requested position.
        index: AcceptanceIndex,
    },
    /// The requested current subtask occurrence is absent.
    SubtaskOccurrenceMissing {
        /// The task owning the absent current occurrence.
        task: TaskId,
        /// The zero-based absent requested occurrence.
        occurrence: SubtaskOccurrence,
    },
    /// The requested complete subtask preimage differs from canonical history.
    SubtaskCorrectionPreimageMismatch {
        /// The task owning the stale correction request.
        task: TaskId,
        /// The zero-based requested occurrence.
        occurrence: SubtaskOccurrence,
    },
    /// The requested complete subtask preimage differs from canonical history.
    SubtaskOccurrenceCheckPreimageMismatch {
        /// The task owning the stale occurrence-check request.
        task: TaskId,
        /// The zero-based requested occurrence.
        occurrence: SubtaskOccurrence,
    },
    /// The addressed current subtask is not one of a duplicate identifier pair.
    SubtaskIdNotDuplicate {
        /// The task owning the non-duplicate occurrence.
        task: TaskId,
        /// The current non-duplicate identifier.
        subtask: String,
    },
    /// The requested replacement identity already names another current subtask.
    SubtaskReplacementIdAlreadyExists {
        /// The task owning the occupied replacement identity.
        task: TaskId,
        /// The already-used replacement identity.
        replacement_id: String,
    },
    /// A current task acceptance criterion is still unchecked.
    AcceptanceItemUnchecked {
        /// The task that cannot yet complete.
        task: TaskId,
        /// The zero-based unchecked criterion position.
        index: AcceptanceIndex,
    },
    /// A current task subtask occurrence is still unchecked.
    SubtaskOccurrenceUnchecked {
        /// The task that cannot yet complete.
        task: TaskId,
        /// The zero-based unchecked occurrence position.
        occurrence: SubtaskOccurrence,
    },
    /// The supplied history contains a task fact newer than this command understands.
    UnsupportedTaskEvent,
    /// The derived task stream identity was rejected by `EventCore`.
    InvalidTaskStream,
    /// The checked `EventCore` command could not produce its one internal acceptance fact.
    ModeledAcceptanceDecisionFailed,
    /// The checked `EventCore` command could not produce its one correction fact.
    ModeledSubtaskCorrectionDecisionFailed,
    /// The checked `EventCore` command could not produce its one occurrence-check fact.
    ModeledSubtaskOccurrenceDecisionFailed,
    /// The checked `EventCore` command could not produce its one activation fact.
    ModeledTaskActivationDecisionFailed,
    /// The checked `EventCore` command did not produce exactly one internal acceptance fact.
    InvalidModeledAcceptancePublication,
    /// The checked `EventCore` command did not produce exactly one correction fact.
    InvalidModeledSubtaskCorrectionPublication,
    /// The checked `EventCore` command did not produce exactly one occurrence-check fact.
    InvalidModeledSubtaskOccurrencePublication,
    /// The closed completion decision did not produce a valid terminal/order batch.
    InvalidModeledTaskCompletionPublication,
    /// The checked `EventCore` command did not produce one valid activation fact.
    InvalidModeledTaskActivationPublication,
    /// The checked `EventCore` command did not produce one valid creation batch.
    InvalidModeledTaskCreationPublication,
    /// The checked `EventCore` command could not produce its creation batch.
    ModeledTaskCreationDecisionFailed,
}

impl TaskCommandError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed borrowed error-code mapping is clearest as a concise final match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidAcceptanceIndex => "tasks_invalid_acceptance_index",
            Self::InvalidSubtaskOccurrence => "tasks_invalid_subtask_occurrence",
            Self::InvalidSubtaskReplacementId => "tasks_invalid_subtask_replacement_id",
            Self::TaskNotInProgress { .. } => "tasks_command_task_not_in_progress",
            Self::TaskActivationNotBacklog { .. } => "tasks_command_task_activation_not_backlog",
            Self::TaskActivationActiveTask { .. } => "tasks_command_task_activation_active_task",
            Self::MultipleActiveTasks { .. } => "tasks_command_multiple_active_tasks",
            Self::TaskActivationBlocked { .. } => "tasks_command_task_activation_blocked",
            Self::TaskActivationNotNextEligible { .. } => {
                "tasks_command_task_activation_not_next_eligible"
            }
            Self::TaskActivationOrderDrift { .. } => "tasks_command_task_activation_order_drift",
            Self::TaskActivationMalformedHistory => {
                "tasks_command_task_activation_malformed_history"
            }
            Self::TaskCreationMalformedHistory => "tasks_command_task_creation_malformed_history",
            Self::TaskMissing { .. } => "tasks_command_task_missing",
            Self::DuplicateTaskCreation { .. } => "tasks_command_duplicate_task_creation",
            Self::TargetTaskFactUnexpectedStream { .. } => {
                "tasks_command_target_task_fact_unexpected_stream"
            }
            Self::HistoryAcceptanceItemMissing { .. } => {
                "tasks_command_history_acceptance_item_missing"
            }
            Self::HistorySubtaskMissing { .. } => "tasks_command_history_subtask_missing",
            Self::HistorySubtaskCorrectionPreimageMismatch { .. } => {
                "tasks_command_history_subtask_correction_preimage_mismatch"
            }
            Self::HistorySubtaskIdNotDuplicate { .. } => {
                "tasks_command_history_subtask_id_not_duplicate"
            }
            Self::HistorySubtaskOccurrenceCheckPreimageMismatch { .. } => {
                "tasks_command_history_subtask_occurrence_check_preimage_mismatch"
            }
            Self::AcceptanceItemMissing { .. } => "tasks_command_acceptance_item_missing",
            Self::SubtaskOccurrenceMissing { .. } => "tasks_command_subtask_occurrence_missing",
            Self::SubtaskCorrectionPreimageMismatch { .. } => {
                "tasks_command_subtask_correction_preimage_mismatch"
            }
            Self::SubtaskOccurrenceCheckPreimageMismatch { .. } => {
                "tasks_command_subtask_occurrence_check_preimage_mismatch"
            }
            Self::SubtaskIdNotDuplicate { .. } => "tasks_command_subtask_id_not_duplicate",
            Self::SubtaskReplacementIdAlreadyExists { .. } => {
                "tasks_command_subtask_replacement_id_already_exists"
            }
            Self::AcceptanceItemUnchecked { .. } => "tasks_command_acceptance_item_unchecked",
            Self::SubtaskOccurrenceUnchecked { .. } => "tasks_command_subtask_occurrence_unchecked",
            Self::UnsupportedTaskEvent => "tasks_command_unsupported_task_event",
            Self::InvalidTaskStream => "tasks_command_invalid_task_stream",
            Self::ModeledAcceptanceDecisionFailed => {
                "tasks_command_modeled_acceptance_decision_failed"
            }
            Self::ModeledSubtaskCorrectionDecisionFailed => {
                "tasks_command_modeled_subtask_correction_decision_failed"
            }
            Self::ModeledSubtaskOccurrenceDecisionFailed => {
                "tasks_command_modeled_subtask_occurrence_decision_failed"
            }
            Self::ModeledTaskActivationDecisionFailed => {
                "tasks_command_modeled_task_activation_decision_failed"
            }
            Self::InvalidModeledAcceptancePublication => {
                "tasks_command_invalid_modeled_acceptance_publication"
            }
            Self::InvalidModeledSubtaskCorrectionPublication => {
                "tasks_command_invalid_modeled_subtask_correction_publication"
            }
            Self::InvalidModeledSubtaskOccurrencePublication => {
                "tasks_command_invalid_modeled_subtask_occurrence_publication"
            }
            Self::InvalidModeledTaskCompletionPublication => {
                "tasks_command_invalid_modeled_task_completion_publication"
            }
            Self::InvalidModeledTaskActivationPublication => {
                "tasks_command_invalid_modeled_task_activation_publication"
            }
            Self::InvalidModeledTaskCreationPublication => {
                "tasks_command_invalid_modeled_task_creation_publication"
            }
            Self::ModeledTaskCreationDecisionFailed => {
                "tasks_command_modeled_task_creation_decision_failed"
            }
        }
    }
}

impl fmt::Display for TaskCommandError {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable code is the intentionally sanitized command-error display"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "task command decisions retain no lower-level source beyond their durable fact context"
)]
impl Error for TaskCommandError {}

/// The only task fact state needed to decide one acceptance mutation.
struct AcceptanceChecklistState {
    /// Whether a target task creation fact has been seen.
    exists: bool,
    /// Current target checklist after canonical ordered mutations.
    items: Vec<ChecklistItem>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow fold implementation follows canonical replay, retained mutation application, and final decision flow"
)]
impl AcceptanceChecklistState {
    /// Folds just the addressed task's acceptance facts from canonical history.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the command-local fold retains only target existence and its mutable checklist"
    )]
    fn fold(events: &[TaskEvent], task: &TaskId) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            exists: false,
            items: Vec::new(),
        };
        let expected_streams = acceptance_consistency_streams(task)?;
        for event in events {
            state.apply(event, task, &expected_streams)?;
        }
        if !state.exists {
            return Err(TaskCommandError::TaskMissing { task: task.clone() });
        }
        Ok(state)
    }

    /// Applies one relevant retained fact to the bounded acceptance state.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the ordered fact dispatcher keeps all acceptance mutation handling visible at this narrow command boundary"
    )]
    fn apply(
        &mut self,
        event: &TaskEvent,
        task: &TaskId,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        match event {
            TaskEvent::TaskCreated(created) if &created.task.stem == task => {
                Self::require_target_fact_stream(&created.stream_id, task, expected_streams)?;
                self.apply_created(created, task)?;
            }
            TaskEvent::TaskAcceptanceAdded(added) if &added.stem == task => {
                Self::require_target_fact_stream(&added.stream_id, task, expected_streams)?;
                self.require_exists(task)?;
                self.items.push(added.item.clone());
            }
            TaskEvent::TaskAcceptanceChecked(checked) if &checked.stem == task => {
                Self::require_target_fact_stream(&checked.stream_id, task, expected_streams)?;
                self.apply_checked(checked, task)?;
            }
            TaskEvent::TaskAcceptanceRemoved(removed) if &removed.stem == task => {
                Self::require_target_fact_stream(&removed.stream_id, task, expected_streams)?;
                self.apply_removed(removed, task)?;
            }
            TaskEvent::HistoricalTaskRemoved(removed) if &removed.stem == task => {
                Self::require_target_fact_stream(&removed.stream_id, task, expected_streams)?;
                self.exists = false;
                self.items.clear();
            }
            TaskEvent::RepositoryInitialized(_)
            | TaskEvent::TaskCreated(_)
            | TaskEvent::TaskTransitioned(_)
            | TaskEvent::TaskPriorityChanged(_)
            | TaskEvent::TaskLinksChanged(_)
            | TaskEvent::TaskSubtaskAdded(_)
            | TaskEvent::TaskSubtaskChecked(_)
            | TaskEvent::TaskSubtaskOccurrenceChecked(_)
            | TaskEvent::TaskSubtaskIdCorrected(_)
            | TaskEvent::TaskDetailsUpdated(_)
            | TaskEvent::HistoricalTaskClaimChanged(_)
            | TaskEvent::TaskPullRequestChanged(_)
            | TaskEvent::TaskAcceptanceAdded(_)
            | TaskEvent::TaskAcceptanceChecked(_)
            | TaskEvent::TaskAcceptanceRemoved(_)
            | TaskEvent::TaskNoteAdded(_)
            | TaskEvent::TaskValidationRepaired(_)
            | TaskEvent::TasksClosedFromCommitTrailers(_)
            | TaskEvent::HistoricalTaskClosedFromTrailer(_)
            | TaskEvent::HistoricalTaskRemoved(_)
            | TaskEvent::BoardReordered(_)
            | TaskEvent::HistoricalTaskStatePublished(_) => {}
            _ => return Err(TaskCommandError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Folds a target task creation after rejecting a duplicated durable identity.
    #[expect(
        clippy::implicit_return,
        reason = "the exact target creation rule ends in the idiomatic unit result"
    )]
    fn apply_created(
        &mut self,
        created: &TaskCreated,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        if self.exists {
            return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() });
        }
        self.exists = true;
        self.items.clone_from(&created.task.acceptance);
        Ok(())
    }

    /// Rejects a target fact whose stream is outside this command's fenced authority.
    #[expect(
        clippy::implicit_return,
        reason = "the two-stream membership guard remains clearest as a direct result expression"
    )]
    fn require_target_fact_stream(
        stream: &StreamId,
        task: &TaskId,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        if expected_streams.contains(stream) {
            Ok(())
        } else {
            Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                task: task.clone(),
                stream: stream.clone(),
            })
        }
    }

    /// Rejects a board-wide fact unless it came from the named board authority stream.
    #[expect(
        clippy::implicit_return,
        reason = "the board-only authority guard remains clearest as a direct result expression"
    )]
    fn require_board_fact_stream(
        stream: &StreamId,
        task: &TaskId,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        if stream == &expected_streams[0] {
            Ok(())
        } else {
            Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                task: task.clone(),
                stream: stream.clone(),
            })
        }
    }

    /// Applies one retained acceptance check after validating its current position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded retained checklist update uses compact option-to-error conversion"
    )]
    fn apply_checked(
        &mut self,
        checked: &TaskAcceptanceChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let item = self.items.get_mut(checked.index).ok_or_else(|| {
            TaskCommandError::HistoryAcceptanceItemMissing {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        item.checked = checked.checked;
        Ok(())
    }

    /// Applies one retained acceptance removal after validating its current position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded retained checklist removal preserves its exact missing-position failure"
    )]
    fn apply_removed(
        &mut self,
        removed: &TaskAcceptanceRemoved,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        if removed.index >= self.items.len() {
            return Err(TaskCommandError::HistoryAcceptanceItemMissing {
                task: task.clone(),
                index: removed.index,
            });
        }
        let _: ChecklistItem = self.items.remove(removed.index);
        Ok(())
    }

    /// Rejects a target mutation before the target task's creation fact.
    #[expect(
        clippy::implicit_return,
        reason = "the command-local existence guard retains the exact target identity in its typed failure"
    )]
    fn require_exists(&self, task: &TaskId) -> Result<(), TaskCommandError> {
        if self.exists {
            Ok(())
        } else {
            Err(TaskCommandError::TaskMissing { task: task.clone() })
        }
    }

    /// Decides either no publication or the exact canonical check-only publication.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the final bounded decision retains only current target checklist state and one closed check-only publication"
    )]
    fn decide(
        &self,
        request: &CheckAcceptance,
    ) -> Result<Option<AcceptanceCheckPublication>, TaskCommandError> {
        let item = self
            .items
            .get(request.index().zero_based_value())
            .ok_or_else(|| TaskCommandError::AcceptanceItemMissing {
                task: request.task().clone(),
                index: request.index(),
            })?;
        if item.checked {
            return Ok(None);
        }
        modeled_acceptance_publication(request).map(Some)
    }
}

/// Modeled external intent for one exact duplicate-subtask correction.
#[derive(ModelInput)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the private modeled request remains beside its public decision entry point so its data-flow provenance is locally inspectable"
)]
struct RepairDuplicateSubtaskIdIntent {
    /// Board stream that receives the durable correction.
    #[model(origin)]
    board_stream: StreamId,
    /// Exact target occurrence selected by the parsed command boundary.
    #[model(origin)]
    occurrence: SubtaskOccurrence,
    /// Complete current target subtask selected by the caller's query boundary.
    #[model(origin)]
    expected: Subtask,
    /// New unique identifier selected at the parsed command boundary.
    #[model(origin)]
    replacement_id: String,
    /// Exact task selected by the parsed command boundary.
    #[model(origin)]
    task: TaskId,
    /// Legacy task stream whose version also fences the decision.
    #[model(origin)]
    task_stream: StreamId,
}

/// Checked `EventCore` command that produces exactly one internal correction fact.
#[derive(ModelCommand)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the checked command retains task-operation field order rather than alphabetizing its semantic boundary"
)]
struct ModeledRepairDuplicateSubtaskId {
    /// Board stream that receives the durable correction.
    #[stream]
    board_stream: StreamId,
    /// Exact target occurrence selected by the parsed command boundary.
    occurrence: SubtaskOccurrence,
    /// Complete current target subtask selected by the caller's query boundary.
    expected: Subtask,
    /// New unique identifier selected at the parsed command boundary.
    replacement_id: String,
    /// Exact task selected by the parsed command boundary.
    task: TaskId,
    /// Legacy task stream read for optimistic consistency.
    #[stream]
    task_stream: StreamId,
}

mapping! {
    RepairDuplicateSubtaskIdIntentToBoardStream:
        RepairDuplicateSubtaskIdIntent.board_stream => ModeledRepairDuplicateSubtaskId.board_stream
        using clone;
}

mapping! {
    RepairDuplicateSubtaskIdIntentToOccurrence:
        RepairDuplicateSubtaskIdIntent.occurrence => ModeledRepairDuplicateSubtaskId.occurrence
        using copy;
}

mapping! {
    RepairDuplicateSubtaskIdIntentToExpected:
        RepairDuplicateSubtaskIdIntent.expected => ModeledRepairDuplicateSubtaskId.expected
        using clone;
}

mapping! {
    RepairDuplicateSubtaskIdIntentToReplacementId:
        RepairDuplicateSubtaskIdIntent.replacement_id => ModeledRepairDuplicateSubtaskId.replacement_id
        using clone;
}

mapping! {
    RepairDuplicateSubtaskIdIntentToTask:
        RepairDuplicateSubtaskIdIntent.task => ModeledRepairDuplicateSubtaskId.task
        using clone;
}

mapping! {
    RepairDuplicateSubtaskIdIntentToTaskStream:
        RepairDuplicateSubtaskIdIntent.task_stream => ModeledRepairDuplicateSubtaskId.task_stream
        using clone;
}

/// Internal modeled fact from which the opaque durable publication token is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledSubtaskIdCorrected {
    /// Exact current occurrence required before applying the correction.
    expected: Subtask,
    /// Zero-based durable occurrence position.
    index: usize,
    /// New unique identifier for that occurrence.
    replacement_id: String,
    /// Board stream receiving the durable correction.
    stream: StreamId,
    /// Task that owns the corrected occurrence.
    task: TaskId,
}

#[expect(
    clippy::implicit_return,
    reason = "the EventCore trait names the event type after its stream accessor, and both direct accessors remain clearest as final expressions"
)]
impl Event for ModeledSubtaskIdCorrected {
    fn event_type_name() -> &'static str {
        "TiberModeledSubtaskIdCorrected"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled correction fact field for provenance checking.
#[derive(ModelOutput)]
struct ModeledSubtaskIdCorrectedView {
    /// Exact current occurrence required before applying the correction.
    expected: Subtask,
    /// Zero-based durable occurrence position.
    index: usize,
    /// New unique identifier for that occurrence.
    replacement_id: String,
    /// Board stream receiving the durable correction.
    stream: StreamId,
    /// Task that owns the corrected occurrence.
    task: TaskId,
}

impl ModeledSubtaskIdCorrectedView {
    /// Projects every modeled event field into the durable-fact boundary shape.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the modeled correction view construction is clearest as the final checked builder expression"
    )]
    fn from_event(event: &ModeledSubtaskIdCorrected) -> Self {
        Self::model_builder()
            .stream(ModeledSubtaskIdCorrectedToViewStream::apply(event))
            .task(ModeledSubtaskIdCorrectedToViewTask::apply(event))
            .index(ModeledSubtaskIdCorrectedToViewIndex::apply(event))
            .expected(ModeledSubtaskIdCorrectedToViewExpected::apply(event))
            .replacement_id(ModeledSubtaskIdCorrectedToViewReplacementId::apply(event))
            .build()
            .into_inner()
    }

    /// Converts the modeled correction view into the one retained task-fact variant.
    #[expect(
        clippy::implicit_return,
        reason = "the closed modeled projection converts directly into the sole durable correction fact authorized by this command"
    )]
    fn into_task_subtask_id_corrected(self) -> TaskSubtaskIdCorrected {
        TaskSubtaskIdCorrected::new(
            self.stream,
            self.task,
            self.index,
            self.expected,
            self.replacement_id,
        )
    }
}

mapping! {
    ModeledSubtaskIdCorrectedToViewStream:
        ModeledSubtaskIdCorrected.stream => ModeledSubtaskIdCorrectedView.stream
        using clone;
}

mapping! {
    ModeledSubtaskIdCorrectedToViewTask:
        ModeledSubtaskIdCorrected.task => ModeledSubtaskIdCorrectedView.task
        using clone;
}

mapping! {
    ModeledSubtaskIdCorrectedToViewIndex:
        ModeledSubtaskIdCorrected.index => ModeledSubtaskIdCorrectedView.index
        using copy;
}

mapping! {
    ModeledSubtaskIdCorrectedToViewExpected:
        ModeledSubtaskIdCorrected.expected => ModeledSubtaskIdCorrectedView.expected
        using clone;
}

mapping! {
    ModeledSubtaskIdCorrectedToViewReplacementId:
        ModeledSubtaskIdCorrected.replacement_id => ModeledSubtaskIdCorrectedView.replacement_id
        using clone;
}

/// Minimal modeled state for exactly-once emission within one `EventCore` command execution.
#[derive(ModelState)]
struct ModeledRepairDuplicateSubtaskIdState {
    /// Whether this modeled command already emitted its one fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the correction fact constructor.
#[derive(ModelOutput)]
struct ModeledRepairDuplicateSubtaskIdDecision {
    /// Whether the command had already emitted its one fact.
    emitted: bool,
}

mapping! {
    ModeledRepairDuplicateSubtaskIdStateToDecision:
        ModeledRepairDuplicateSubtaskIdState.emitted => ModeledRepairDuplicateSubtaskIdDecision.emitted
        using copy;
}

mapping! {
    ModeledRepairDuplicateSubtaskIdToFactStream:
        (ModeledRepairDuplicateSubtaskId.board_stream, ModeledRepairDuplicateSubtaskIdDecision.emitted) => ModeledSubtaskIdCorrected.stream
        using try correction_stream_once, error = CommandError;
}

mapping! {
    ModeledRepairDuplicateSubtaskIdToFactTask:
        ModeledRepairDuplicateSubtaskId.task => ModeledSubtaskIdCorrected.task
        using clone;
}

mapping! {
    ModeledRepairDuplicateSubtaskIdToFactExpected:
        ModeledRepairDuplicateSubtaskId.expected => ModeledSubtaskIdCorrected.expected
        using clone;
}

mapping! {
    ModeledRepairDuplicateSubtaskIdToFactReplacementId:
        ModeledRepairDuplicateSubtaskId.replacement_id => ModeledSubtaskIdCorrected.replacement_id
        using clone;
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "the EventCore trait fixes the evolve/decide API and uses a default stream-discovery method; modeled correction construction is the checked terminal expression"
)]
impl ModelCommandLogic for ModeledRepairDuplicateSubtaskId {
    type Event = ModeledSubtaskIdCorrected;
    type State = ModeledRepairDuplicateSubtaskIdState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledRepairDuplicateSubtaskIdDecision::model_builder()
            .emitted(ModeledRepairDuplicateSubtaskIdStateToDecision::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            ModeledSubtaskIdCorrected::model_builder()
                .stream(ModeledRepairDuplicateSubtaskIdToFactStream::apply((
                    self,
                    decision.as_ref(),
                ))?)
                .task(ModeledRepairDuplicateSubtaskIdToFactTask::apply(self))
                .index(ModeledRepairDuplicateSubtaskIdToFactIndex::apply(self))
                .expected(ModeledRepairDuplicateSubtaskIdToFactExpected::apply(self))
                .replacement_id(ModeledRepairDuplicateSubtaskIdToFactReplacementId::apply(
                    self,
                ))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = true;
        Modeled::from_built(folded)
    }
}

/// The only task fact state needed to decide one duplicate-subtask identity correction.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow correction state fields follow replay and idempotency data flow rather than alphabetical order"
)]
struct SubtaskCorrectionState {
    /// Whether a target task creation fact has been seen.
    exists: bool,
    /// Whether canonical history already contains this exact correction fact.
    exact_correction_present: bool,
    /// Current target subtask occurrences after canonical ordered mutations.
    subtasks: Vec<Subtask>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow fold implementation follows canonical replay, retained mutation application, and final decision flow"
)]
impl SubtaskCorrectionState {
    /// Folds only the addressed task's subtask facts from canonical history.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the command-local fold retains only target existence, subtask occurrences, and exact-correction idempotency"
    )]
    fn fold(
        events: &[TaskEvent],
        request: &RepairDuplicateSubtaskId,
    ) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            exists: false,
            exact_correction_present: false,
            subtasks: Vec::new(),
        };
        let expected_streams = acceptance_consistency_streams(request.task())?;
        for event in events {
            state.apply(event, request, &expected_streams)?;
        }
        if !state.exists {
            return Err(TaskCommandError::TaskMissing {
                task: request.task().clone(),
            });
        }
        Ok(state)
    }

    /// Applies one relevant retained fact to the bounded subtask state.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the ordered fact dispatcher keeps every relevant subtask mutation visible at this narrow command boundary"
    )]
    fn apply(
        &mut self,
        event: &TaskEvent,
        request: &RepairDuplicateSubtaskId,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        let task = request.task();
        match event {
            TaskEvent::TaskCreated(created) if &created.task.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &created.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_created(created, task)?;
            }
            TaskEvent::TaskSubtaskAdded(added) if &added.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &added.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.subtasks.push(added.subtask.clone());
            }
            TaskEvent::TaskSubtaskChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_checked(checked, task)?;
            }
            TaskEvent::TaskSubtaskOccurrenceChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_occurrence_checked(checked, task)?;
            }
            TaskEvent::TaskSubtaskIdCorrected(corrected) if &corrected.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &corrected.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_corrected(corrected, request, task)?;
            }
            TaskEvent::HistoricalTaskRemoved(removed) if &removed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &removed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.exists = false;
                self.exact_correction_present = false;
                self.subtasks.clear();
            }
            TaskEvent::RepositoryInitialized(_)
            | TaskEvent::TaskCreated(_)
            | TaskEvent::TaskTransitioned(_)
            | TaskEvent::TaskPriorityChanged(_)
            | TaskEvent::TaskLinksChanged(_)
            | TaskEvent::TaskSubtaskAdded(_)
            | TaskEvent::TaskSubtaskChecked(_)
            | TaskEvent::TaskSubtaskOccurrenceChecked(_)
            | TaskEvent::TaskSubtaskIdCorrected(_)
            | TaskEvent::TaskDetailsUpdated(_)
            | TaskEvent::HistoricalTaskClaimChanged(_)
            | TaskEvent::TaskPullRequestChanged(_)
            | TaskEvent::TaskAcceptanceAdded(_)
            | TaskEvent::TaskAcceptanceChecked(_)
            | TaskEvent::TaskAcceptanceRemoved(_)
            | TaskEvent::TaskNoteAdded(_)
            | TaskEvent::TaskValidationRepaired(_)
            | TaskEvent::TasksClosedFromCommitTrailers(_)
            | TaskEvent::HistoricalTaskClosedFromTrailer(_)
            | TaskEvent::HistoricalTaskRemoved(_)
            | TaskEvent::BoardReordered(_)
            | TaskEvent::HistoricalTaskStatePublished(_) => {}
            _ => return Err(TaskCommandError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Folds a target task creation after rejecting a duplicated durable identity.
    #[expect(
        clippy::implicit_return,
        reason = "the exact target creation rule ends in the idiomatic unit result"
    )]
    fn apply_created(
        &mut self,
        created: &TaskCreated,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        if self.exists {
            return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() });
        }
        self.exists = true;
        self.subtasks.clone_from(&created.task.subtasks);
        Ok(())
    }

    /// Applies one legacy identifier-based check with the retained first-match semantics.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "legacy ID-only facts cannot distinguish duplicates, so replay preserves the established first-match read semantics"
    )]
    fn apply_checked(
        &mut self,
        checked: &TaskSubtaskChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let subtask = self
            .subtasks
            .iter_mut()
            .find(|subtask| subtask.id == checked.subtask_id)
            .ok_or_else(|| TaskCommandError::HistorySubtaskMissing {
                task: task.clone(),
                subtask: checked.subtask_id.clone(),
            })?;
        subtask.checked = checked.checked;
        Ok(())
    }

    /// Applies one exact retained occurrence check before a later identity correction.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the retained occurrence fact validates its complete unchecked preimage before changing only the selected occurrence state"
    )]
    fn apply_occurrence_checked(
        &mut self,
        checked: &TaskSubtaskOccurrenceChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let current = self.subtasks.get(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        if current != &checked.expected || checked.expected.checked {
            return Err(
                TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                    task: task.clone(),
                    index: checked.index,
                },
            );
        }
        let target = self.subtasks.get_mut(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        target.checked = true;
        Ok(())
    }

    /// Applies one exact preconditioned correction fact and records exact idempotency.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the correction fact checks its fixed occurrence and complete preimage before changing the sole permitted identifier"
    )]
    fn apply_corrected(
        &mut self,
        corrected: &TaskSubtaskIdCorrected,
        request: &RepairDuplicateSubtaskId,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let occurrence = self.subtasks.get(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        if occurrence != &corrected.expected {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let expected_id_duplicate_count = self
            .subtasks
            .iter()
            .filter(|subtask| subtask.id == corrected.expected.id)
            .count();
        if expected_id_duplicate_count < 2 {
            return Err(TaskCommandError::HistorySubtaskIdNotDuplicate {
                task: task.clone(),
                subtask: corrected.expected.id.clone(),
            });
        }
        if corrected.expected.id == corrected.replacement_id
            || corrected.replacement_id.trim().is_empty()
            || corrected.replacement_id.chars().any(char::is_control)
            || self
                .subtasks
                .iter()
                .any(|subtask| subtask.id == corrected.replacement_id)
        {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let corrected_occurrence = self.subtasks.get_mut(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        corrected_occurrence
            .id
            .clone_from(&corrected.replacement_id);
        if corrected.index == request.occurrence().zero_based_value()
            && corrected.expected == *request.expected()
            && corrected.replacement_id == request.replacement_id()
        {
            self.exact_correction_present = true;
        }
        Ok(())
    }

    /// Rejects a target mutation before the target task's creation fact.
    #[expect(
        clippy::implicit_return,
        reason = "the command-local existence guard retains the exact target identity in its typed failure"
    )]
    fn require_exists(&self, task: &TaskId) -> Result<(), TaskCommandError> {
        if self.exists {
            Ok(())
        } else {
            Err(TaskCommandError::TaskMissing { task: task.clone() })
        }
    }

    /// Decides either no publication or the exact canonical duplicate-identity correction.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the final bounded decision retains only the current target occurrences and one closed correction publication"
    )]
    fn decide(
        &self,
        request: &RepairDuplicateSubtaskId,
    ) -> Result<Option<SubtaskIdCorrectionPublication>, TaskCommandError> {
        if self.exact_correction_present {
            return Ok(None);
        }
        let occurrence = self
            .subtasks
            .get(request.occurrence().zero_based_value())
            .ok_or_else(|| TaskCommandError::SubtaskOccurrenceMissing {
                task: request.task().clone(),
                occurrence: request.occurrence(),
            })?;
        if occurrence != request.expected() {
            return Err(TaskCommandError::SubtaskCorrectionPreimageMismatch {
                task: request.task().clone(),
                occurrence: request.occurrence(),
            });
        }
        let duplicate_count = self
            .subtasks
            .iter()
            .filter(|subtask| subtask.id == occurrence.id)
            .count();
        if duplicate_count < 2 {
            return Err(TaskCommandError::SubtaskIdNotDuplicate {
                task: request.task().clone(),
                subtask: occurrence.id.clone(),
            });
        }
        if self
            .subtasks
            .iter()
            .any(|subtask| subtask.id == request.replacement_id())
        {
            return Err(TaskCommandError::SubtaskReplacementIdAlreadyExists {
                task: request.task().clone(),
                replacement_id: request.replacement_id().to_owned(),
            });
        }
        modeled_subtask_correction_publication(request).map(Some)
    }
}

/// The only task fact state needed to decide one exact subtask-occurrence check.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow occurrence-check state follows target lifetime, exact idempotency, and current occurrence replay flow"
)]
struct SubtaskOccurrenceCheckState {
    /// Whether the target task currently exists after its complete retained lifetime.
    exists: bool,
    /// Current lifecycle needed to reject new mutations after a terminal transition.
    status: Option<TaskStatus>,
    /// Whether canonical history contains this exact occurrence-check fact.
    ///
    /// A later retained mutation can supersede its current effect, so the
    /// final decision also verifies the addressed occurrence still reflects
    /// this exact check before treating a retry as idempotent.
    exact_check_present: bool,
    /// Current target subtask occurrences after canonical ordered mutations.
    subtasks: Vec<Subtask>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow occurrence-check fold follows canonical replay, retained mutation application, and final decision flow"
)]
impl SubtaskOccurrenceCheckState {
    /// Folds only the addressed task's occurrence and lifecycle facts from canonical history.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the command-local fold retains only the target lifetime, lifecycle, exact idempotency, and subtask occurrences"
    )]
    fn fold(
        events: &[TaskEvent],
        request: &CheckSubtaskOccurrence,
    ) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            exists: false,
            status: None,
            exact_check_present: false,
            subtasks: Vec::new(),
        };
        let expected_streams = acceptance_consistency_streams(request.task())?;
        for event in events {
            state.apply(event, request, &expected_streams)?;
        }
        if !state.exists {
            return Err(TaskCommandError::TaskMissing {
                task: request.task().clone(),
            });
        }
        Ok(state)
    }

    /// Applies one relevant retained fact to the bounded occurrence-check state.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        clippy::too_many_lines,
        reason = "the ordered dispatcher keeps target lifetime, lifecycle, legacy checks, corrections, and exact occurrence checks visible at the authority boundary"
    )]
    fn apply(
        &mut self,
        event: &TaskEvent,
        request: &CheckSubtaskOccurrence,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        let task = request.task();
        match event {
            TaskEvent::TaskCreated(created) if &created.task.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &created.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_created(created, task)?;
            }
            TaskEvent::TaskTransitioned(transitioned) if &transitioned.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &transitioned.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.status = Some(transitioned.status);
            }
            TaskEvent::TaskSubtaskAdded(added) if &added.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &added.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.subtasks.push(added.subtask.clone());
            }
            TaskEvent::TaskSubtaskChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_legacy_checked(checked, task)?;
            }
            TaskEvent::TaskSubtaskIdCorrected(corrected) if &corrected.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &corrected.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_corrected(corrected, task)?;
            }
            TaskEvent::TaskSubtaskOccurrenceChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_occurrence_checked(checked, request, task)?;
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                AcceptanceChecklistState::require_board_fact_stream(
                    &closed.stream_id,
                    task,
                    expected_streams,
                )?;
                if closed.stems.contains(task) {
                    self.require_exists(task)?;
                    self.status = Some(TaskStatus::Done);
                }
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) if &closed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &closed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.status = Some(TaskStatus::Done);
            }
            TaskEvent::HistoricalTaskRemoved(removed) if &removed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &removed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.exists = false;
                self.status = None;
                self.exact_check_present = false;
                self.subtasks.clear();
            }
            TaskEvent::RepositoryInitialized(_)
            | TaskEvent::TaskCreated(_)
            | TaskEvent::TaskTransitioned(_)
            | TaskEvent::TaskPriorityChanged(_)
            | TaskEvent::TaskLinksChanged(_)
            | TaskEvent::TaskSubtaskAdded(_)
            | TaskEvent::TaskSubtaskChecked(_)
            | TaskEvent::TaskSubtaskOccurrenceChecked(_)
            | TaskEvent::TaskSubtaskIdCorrected(_)
            | TaskEvent::TaskDetailsUpdated(_)
            | TaskEvent::HistoricalTaskClaimChanged(_)
            | TaskEvent::TaskPullRequestChanged(_)
            | TaskEvent::TaskAcceptanceAdded(_)
            | TaskEvent::TaskAcceptanceChecked(_)
            | TaskEvent::TaskAcceptanceRemoved(_)
            | TaskEvent::TaskNoteAdded(_)
            | TaskEvent::TaskValidationRepaired(_)
            | TaskEvent::HistoricalTaskClosedFromTrailer(_)
            | TaskEvent::HistoricalTaskRemoved(_)
            | TaskEvent::BoardReordered(_)
            | TaskEvent::HistoricalTaskStatePublished(_) => {}
            _ => return Err(TaskCommandError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Folds a target task creation after allowing a prior removal to start a new lifetime.
    #[expect(
        clippy::implicit_return,
        reason = "the exact target creation rule resets every command-local lifetime field before ending in the idiomatic unit result"
    )]
    fn apply_created(
        &mut self,
        created: &TaskCreated,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        if self.exists {
            return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() });
        }
        self.exists = true;
        self.status = Some(created.task.status);
        self.exact_check_present = false;
        self.subtasks.clone_from(&created.task.subtasks);
        Ok(())
    }

    /// Applies a legacy identifier-only check using its retained first-match semantics.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "legacy ID-only facts cannot distinguish duplicates, so replay preserves the established first-match semantics"
    )]
    fn apply_legacy_checked(
        &mut self,
        checked: &TaskSubtaskChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let subtask = self
            .subtasks
            .iter_mut()
            .find(|subtask| subtask.id == checked.subtask_id)
            .ok_or_else(|| TaskCommandError::HistorySubtaskMissing {
                task: task.clone(),
                subtask: checked.subtask_id.clone(),
            })?;
        subtask.checked = checked.checked;
        Ok(())
    }

    /// Applies one exact retained identifier correction before later occurrence decisions.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the retained correction validates its fixed preimage and duplicate identity before changing exactly one current occurrence"
    )]
    fn apply_corrected(
        &mut self,
        corrected: &TaskSubtaskIdCorrected,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let occurrence = self.subtasks.get(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        if occurrence != &corrected.expected {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let duplicate_count = self
            .subtasks
            .iter()
            .filter(|subtask| subtask.id == corrected.expected.id)
            .count();
        if duplicate_count < 2 {
            return Err(TaskCommandError::HistorySubtaskIdNotDuplicate {
                task: task.clone(),
                subtask: corrected.expected.id.clone(),
            });
        }
        if corrected.expected.id == corrected.replacement_id
            || corrected.replacement_id.trim().is_empty()
            || corrected.replacement_id.chars().any(char::is_control)
            || self
                .subtasks
                .iter()
                .any(|subtask| subtask.id == corrected.replacement_id)
        {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let target = self.subtasks.get_mut(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        target.id.clone_from(&corrected.replacement_id);
        Ok(())
    }

    /// Applies one exact retained occurrence check and records exact idempotency.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the occurrence fact validates its complete immutable preimage before changing only its selected check state"
    )]
    fn apply_occurrence_checked(
        &mut self,
        checked: &TaskSubtaskOccurrenceChecked,
        request: &CheckSubtaskOccurrence,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let current = self.subtasks.get(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        if current != &checked.expected || checked.expected.checked {
            return Err(
                TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                    task: task.clone(),
                    index: checked.index,
                },
            );
        }
        let target = self.subtasks.get_mut(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        target.checked = true;
        let mut checked_postimage = checked.expected.clone();
        checked_postimage.checked = true;
        if checked.index == request.occurrence().zero_based_value()
            && (checked.expected == *request.expected() || checked_postimage == *request.expected())
        {
            self.exact_check_present = true;
        }
        Ok(())
    }

    /// Rejects a target mutation before its current creation fact.
    #[expect(
        clippy::implicit_return,
        reason = "the command-local existence guard retains the exact target identity in its typed failure"
    )]
    fn require_exists(&self, task: &TaskId) -> Result<(), TaskCommandError> {
        if self.exists {
            Ok(())
        } else {
            Err(TaskCommandError::TaskMissing { task: task.clone() })
        }
    }

    /// Reports whether the exact retained check still owns the current addressed occurrence.
    ///
    /// Legacy identifier-based checks remain valid retained history and can
    /// subsequently clear the addressed occurrence. A historical exact fact
    /// alone therefore cannot establish idempotency; the final occurrence must
    /// still equal the request's complete preimage with only `checked` changed
    /// to `true`.
    #[expect(
        clippy::implicit_return,
        reason = "the current-state idempotency predicate is clearest as one bounded exact-preimage comparison"
    )]
    fn exact_check_still_current(&self, request: &CheckSubtaskOccurrence) -> bool {
        if !self.exact_check_present {
            return false;
        }
        let mut checked_preimage = request.expected().clone();
        checked_preimage.checked = true;
        self.subtasks
            .get(request.occurrence().zero_based_value())
            .is_some_and(|current| current == &checked_preimage)
    }

    /// Decides either no publication or the exact canonical occurrence-check fact.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the final bounded decision retains only target lifecycle, current occurrences, exact idempotency, and one closed occurrence-check publication"
    )]
    fn decide(
        &self,
        request: &CheckSubtaskOccurrence,
    ) -> Result<Option<SubtaskOccurrenceCheckPublication>, TaskCommandError> {
        if self.exact_check_still_current(request) {
            return Ok(None);
        }
        let status = self.status.ok_or_else(|| TaskCommandError::TaskMissing {
            task: request.task().clone(),
        })?;
        if status != TaskStatus::InProgress {
            return Err(TaskCommandError::TaskNotInProgress {
                task: request.task().clone(),
                status,
            });
        }
        let occurrence = self
            .subtasks
            .get(request.occurrence().zero_based_value())
            .ok_or_else(|| TaskCommandError::SubtaskOccurrenceMissing {
                task: request.task().clone(),
                occurrence: request.occurrence(),
            })?;
        if occurrence != request.expected() {
            return Err(TaskCommandError::SubtaskOccurrenceCheckPreimageMismatch {
                task: request.task().clone(),
                occurrence: request.occurrence(),
            });
        }
        if occurrence.checked {
            return Ok(None);
        }
        modeled_subtask_occurrence_publication(request).map(Some)
    }
}

/// The only task and board facts needed to decide one task completion.
///
/// This is a command-local fold, not the broad query projection: it retains
/// only the addressed task's current lifecycle/checklists and the one strict
/// board order that completion must repair atomically.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the bounded completion state follows target lifetime, lifecycle requirements, then board repair flow"
)]
struct TaskCompletionState {
    /// Whether the target task currently exists after its complete retained lifetime.
    exists: bool,
    /// Current lifecycle used to decide first completion versus stale-order repair.
    status: Option<TaskStatus>,
    /// Current acceptance requirements after retained ordered mutations.
    acceptance: Vec<ChecklistItem>,
    /// Current subtask requirements after retained ordered mutations.
    subtasks: Vec<Subtask>,
    /// Current strict board order, including historical stale/duplicate target entries.
    board_order: Vec<TaskId>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the narrow completion fold follows canonical replay, requirement validation, and the closed terminal/order decision"
)]
impl TaskCompletionState {
    /// Folds only completion-relevant target and board facts from canonical history.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the command-local fold retains only task completion requirements and the strict board order it must repair"
    )]
    fn fold(events: &[TaskEvent], request: &CompleteTask) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            exists: false,
            status: None,
            acceptance: Vec::new(),
            subtasks: Vec::new(),
            board_order: Vec::new(),
        };
        let expected_streams = acceptance_consistency_streams(request.task())?;
        for event in events {
            state.apply(event, request.task(), &expected_streams)?;
        }
        if !state.exists {
            return Err(TaskCommandError::TaskMissing {
                task: request.task().clone(),
            });
        }
        Ok(state)
    }

    /// Applies one retained fact to the bounded completion state.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        clippy::too_many_lines,
        reason = "the ordered dispatcher keeps every task-lifetime, requirement, and board-order fact that completion depends on visible at its authority boundary"
    )]
    fn apply(
        &mut self,
        event: &TaskEvent,
        task: &TaskId,
        expected_streams: &[StreamId; 2],
    ) -> Result<(), TaskCommandError> {
        match event {
            TaskEvent::TaskCreated(created) if &created.task.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &created.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_created(created, task)?;
            }
            TaskEvent::TaskTransitioned(transitioned) if &transitioned.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &transitioned.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.status = Some(transitioned.status);
            }
            TaskEvent::TaskAcceptanceAdded(added) if &added.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &added.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.acceptance.push(added.item.clone());
            }
            TaskEvent::TaskAcceptanceChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_acceptance_checked(checked, task)?;
            }
            TaskEvent::TaskAcceptanceRemoved(removed) if &removed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &removed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_acceptance_removed(removed, task)?;
            }
            TaskEvent::TaskSubtaskAdded(added) if &added.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &added.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.subtasks.push(added.subtask.clone());
            }
            TaskEvent::TaskSubtaskChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_legacy_subtask_checked(checked, task)?;
            }
            TaskEvent::TaskSubtaskIdCorrected(corrected) if &corrected.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &corrected.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_subtask_corrected(corrected, task)?;
            }
            TaskEvent::TaskSubtaskOccurrenceChecked(checked) if &checked.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &checked.stream_id,
                    task,
                    expected_streams,
                )?;
                self.apply_subtask_occurrence_checked(checked, task)?;
            }
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                AcceptanceChecklistState::require_board_fact_stream(
                    &order.stream_id,
                    task,
                    expected_streams,
                )?;
                self.board_order.clone_from(&order.order);
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                if let Some(order) = &repaired.order_change {
                    AcceptanceChecklistState::require_board_fact_stream(
                        &repaired.stream_id,
                        task,
                        expected_streams,
                    )?;
                    if order.stream_id != repaired.stream_id {
                        return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                            task: task.clone(),
                            stream: order.stream_id.clone(),
                        });
                    }
                    self.board_order.clone_from(&order.order);
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                AcceptanceChecklistState::require_board_fact_stream(
                    &closed.stream_id,
                    task,
                    expected_streams,
                )?;
                if closed.stems.contains(task) {
                    self.require_exists(task)?;
                    self.status = Some(TaskStatus::Done);
                }
                self.board_order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) if &closed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &closed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.require_exists(task)?;
                self.status = Some(TaskStatus::Done);
            }
            TaskEvent::HistoricalTaskRemoved(removed) if &removed.stem == task => {
                AcceptanceChecklistState::require_target_fact_stream(
                    &removed.stream_id,
                    task,
                    expected_streams,
                )?;
                self.exists = false;
                self.status = None;
                self.acceptance.clear();
                self.subtasks.clear();
            }
            TaskEvent::RepositoryInitialized(_)
            | TaskEvent::TaskCreated(_)
            | TaskEvent::TaskTransitioned(_)
            | TaskEvent::TaskLinksChanged(_)
            | TaskEvent::TaskSubtaskAdded(_)
            | TaskEvent::TaskSubtaskChecked(_)
            | TaskEvent::TaskSubtaskOccurrenceChecked(_)
            | TaskEvent::TaskSubtaskIdCorrected(_)
            | TaskEvent::TaskDetailsUpdated(_)
            | TaskEvent::HistoricalTaskClaimChanged(_)
            | TaskEvent::TaskPullRequestChanged(_)
            | TaskEvent::TaskAcceptanceAdded(_)
            | TaskEvent::TaskAcceptanceChecked(_)
            | TaskEvent::TaskAcceptanceRemoved(_)
            | TaskEvent::TaskNoteAdded(_)
            | TaskEvent::HistoricalTaskClosedFromTrailer(_)
            | TaskEvent::HistoricalTaskRemoved(_)
            | TaskEvent::HistoricalTaskStatePublished(_) => {}
            _ => return Err(TaskCommandError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Folds a target task creation after allowing a prior removal to start a new lifetime.
    #[expect(
        clippy::implicit_return,
        reason = "the exact target creation rule resets every completion requirement before ending in the idiomatic unit result"
    )]
    fn apply_created(
        &mut self,
        created: &TaskCreated,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        if self.exists {
            return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() });
        }
        self.exists = true;
        self.status = Some(created.task.status);
        self.acceptance.clone_from(&created.task.acceptance);
        self.subtasks.clone_from(&created.task.subtasks);
        Ok(())
    }

    /// Applies one retained acceptance check after validating its current position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded retained checklist update uses compact option-to-error conversion"
    )]
    fn apply_acceptance_checked(
        &mut self,
        checked: &TaskAcceptanceChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let item = self.acceptance.get_mut(checked.index).ok_or_else(|| {
            TaskCommandError::HistoryAcceptanceItemMissing {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        item.checked = checked.checked;
        Ok(())
    }

    /// Applies one retained acceptance removal after validating its current position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded retained checklist removal preserves its exact missing-position failure"
    )]
    fn apply_acceptance_removed(
        &mut self,
        removed: &TaskAcceptanceRemoved,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        if removed.index >= self.acceptance.len() {
            return Err(TaskCommandError::HistoryAcceptanceItemMissing {
                task: task.clone(),
                index: removed.index,
            });
        }
        let _: ChecklistItem = self.acceptance.remove(removed.index);
        Ok(())
    }

    /// Applies a legacy identifier-only check using its retained first-match semantics.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "legacy ID-only facts cannot distinguish duplicates, so completion replay preserves the established first-match semantics"
    )]
    fn apply_legacy_subtask_checked(
        &mut self,
        checked: &TaskSubtaskChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let subtask = self
            .subtasks
            .iter_mut()
            .find(|subtask| subtask.id == checked.subtask_id)
            .ok_or_else(|| TaskCommandError::HistorySubtaskMissing {
                task: task.clone(),
                subtask: checked.subtask_id.clone(),
            })?;
        subtask.checked = checked.checked;
        Ok(())
    }

    /// Applies one exact retained identifier correction before completion checks its current subtasks.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the retained correction validates its fixed preimage and duplicate identity before changing exactly one current occurrence"
    )]
    fn apply_subtask_corrected(
        &mut self,
        corrected: &TaskSubtaskIdCorrected,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let current = self.subtasks.get(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        if current != &corrected.expected {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let duplicate_count = self
            .subtasks
            .iter()
            .filter(|subtask| subtask.id == corrected.expected.id)
            .count();
        if duplicate_count < 2 {
            return Err(TaskCommandError::HistorySubtaskIdNotDuplicate {
                task: task.clone(),
                subtask: corrected.expected.id.clone(),
            });
        }
        if corrected.expected.id == corrected.replacement_id
            || corrected.replacement_id.trim().is_empty()
            || corrected.replacement_id.chars().any(char::is_control)
            || self
                .subtasks
                .iter()
                .any(|subtask| subtask.id == corrected.replacement_id)
        {
            return Err(TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            });
        }
        let target = self.subtasks.get_mut(corrected.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskCorrectionPreimageMismatch {
                task: task.clone(),
                index: corrected.index,
            }
        })?;
        target.id.clone_from(&corrected.replacement_id);
        Ok(())
    }

    /// Applies one exact retained occurrence check before completion checks all current subtasks.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the retained occurrence fact validates its complete immutable preimage before changing only its selected check state"
    )]
    fn apply_subtask_occurrence_checked(
        &mut self,
        checked: &TaskSubtaskOccurrenceChecked,
        task: &TaskId,
    ) -> Result<(), TaskCommandError> {
        self.require_exists(task)?;
        let current = self.subtasks.get(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        if current != &checked.expected || checked.expected.checked {
            return Err(
                TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                    task: task.clone(),
                    index: checked.index,
                },
            );
        }
        let target = self.subtasks.get_mut(checked.index).ok_or_else(|| {
            TaskCommandError::HistorySubtaskOccurrenceCheckPreimageMismatch {
                task: task.clone(),
                index: checked.index,
            }
        })?;
        target.checked = true;
        Ok(())
    }

    /// Rejects a target mutation before its current creation fact.
    #[expect(
        clippy::implicit_return,
        reason = "the command-local existence guard retains the exact target identity in its typed failure"
    )]
    fn require_exists(&self, task: &TaskId) -> Result<(), TaskCommandError> {
        if self.exists {
            Ok(())
        } else {
            Err(TaskCommandError::TaskMissing { task: task.clone() })
        }
    }

    /// Decides first completion, an order-only stale-board repair, or an idempotent no-op.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the final bounded decision validates current requirements before constructing exactly the terminal/order batch the adapter may publish"
    )]
    fn decide(
        &self,
        request: &CompleteTask,
    ) -> Result<Option<TaskCompletionPublication>, TaskCommandError> {
        let status = self.status.ok_or_else(|| TaskCommandError::TaskMissing {
            task: request.task().clone(),
        })?;
        let repaired_order = self
            .board_order
            .iter()
            .filter(|entry| *entry != request.task())
            .cloned()
            .collect::<Vec<_>>();
        if status == TaskStatus::Done {
            if repaired_order == self.board_order {
                return Ok(None);
            }
            return modeled_task_completion_publication(request.task(), false, repaired_order)
                .map(Some);
        }
        if status != TaskStatus::InProgress {
            return Err(TaskCommandError::TaskNotInProgress {
                task: request.task().clone(),
                status,
            });
        }
        if let Some((index, _item)) = self
            .acceptance
            .iter()
            .enumerate()
            .find(|&(_index, item)| !item.checked)
        {
            return Err(TaskCommandError::AcceptanceItemUnchecked {
                task: request.task().clone(),
                index: AcceptanceIndex::zero_based(index),
            });
        }
        if let Some((index, _subtask)) = self
            .subtasks
            .iter()
            .enumerate()
            .find(|&(_index, subtask)| !subtask.checked)
        {
            return Err(TaskCommandError::SubtaskOccurrenceUnchecked {
                task: request.task().clone(),
                occurrence: SubtaskOccurrence::zero_based(index),
            });
        }
        modeled_task_completion_publication(request.task(), true, repaired_order).map(Some)
    }
}

/// Current lifecycle and dependency data needed only for one activation decision.
#[derive(Clone, Debug)]
struct ActivationTask {
    /// Current prerequisite task identities.
    blockers: Vec<TaskId>,
    /// Current durable lifecycle state.
    status: TaskStatus,
}

/// Command-specific state for activating one strict-next backlog task.
///
/// This is intentionally narrower than the query projection: it folds only
/// current lifecycle, blocker, and strict-order facts needed to select a
/// single active task without normalizing unrelated historical board state.
#[derive(Debug, Default)]
struct TaskActivationState {
    /// Latest strict board ordering, retaining unrelated stale entries.
    board_order: Vec<TaskId>,
    /// Current task lifecycle and blocker data by durable identity.
    tasks: BTreeMap<TaskId, ActivationTask>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the activation fold follows canonical history, bounded current-state updates, then one strict-next decision"
)]
impl TaskActivationState {
    /// Folds only activation-relevant facts from canonical task history.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the command-local fold retains only lifecycle, blockers, and board order required for activation"
    )]
    fn fold(events: &[TaskEvent]) -> Result<Self, TaskCommandError> {
        let mut state = Self::default();
        for event in events {
            state.apply(event)?;
        }
        Ok(state)
    }

    /// Applies one retained fact that can change activation authority.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the bounded dispatcher keeps every lifecycle, dependency, and board-order fact affecting strict activation visible at its authority boundary"
    )]
    fn apply(&mut self, event: &TaskEvent) -> Result<(), TaskCommandError> {
        match event {
            TaskEvent::TaskCreated(created) => self.apply_created(created)?,
            TaskEvent::TaskTransitioned(transitioned) => self.apply_transition(transitioned)?,
            TaskEvent::TaskLinksChanged(changed) => self.apply_links_changed(changed)?,
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                self.apply_board_order(order)?;
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                Self::require_board_stream(&repaired.stream_id)?;
                for changed in &repaired.link_changes {
                    self.apply_links_changed(changed)?;
                }
                if let Some(order) = &repaired.order_change {
                    if order.stream_id != repaired.stream_id {
                        return Err(TaskCommandError::TaskActivationMalformedHistory);
                    }
                    self.apply_board_order(order)?;
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                Self::require_board_stream(&closed.stream_id)?;
                for task in &closed.stems {
                    self.task_mut(task)?.status = TaskStatus::Done;
                }
                self.board_order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) => {
                Self::require_task_stream(&closed.stream_id, &closed.stem)?;
                self.task_mut(&closed.stem)?.status = TaskStatus::Done;
            }
            TaskEvent::HistoricalTaskRemoved(removed) => {
                Self::require_task_stream(&removed.stream_id, &removed.stem)?;
                self.remove_task(&removed.stem)?;
            }
            TaskEvent::RepositoryInitialized(_)
            | TaskEvent::TaskSubtaskAdded(_)
            | TaskEvent::TaskSubtaskChecked(_)
            | TaskEvent::TaskSubtaskOccurrenceChecked(_)
            | TaskEvent::TaskSubtaskIdCorrected(_)
            | TaskEvent::TaskDetailsUpdated(_)
            | TaskEvent::HistoricalTaskClaimChanged(_)
            | TaskEvent::TaskPullRequestChanged(_)
            | TaskEvent::TaskAcceptanceAdded(_)
            | TaskEvent::TaskAcceptanceChecked(_)
            | TaskEvent::TaskAcceptanceRemoved(_)
            | TaskEvent::TaskNoteAdded(_)
            | TaskEvent::HistoricalTaskStatePublished(_) => {}
            _ => return Err(TaskCommandError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Folds one current task creation after checking its stream ownership.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small creation fold validates source ownership before preserving just lifecycle and blockers"
    )]
    fn apply_created(&mut self, created: &TaskCreated) -> Result<(), TaskCommandError> {
        let task = &created.task;
        Self::require_task_stream(&created.stream_id, &task.stem)?;
        if self.tasks.contains_key(&task.stem) {
            return Err(TaskCommandError::DuplicateTaskCreation {
                task: task.stem.clone(),
            });
        }
        let _: Option<ActivationTask> = self.tasks.insert(
            task.stem.clone(),
            ActivationTask {
                status: task.status,
                blockers: task.blocked_by.clone(),
            },
        );
        Ok(())
    }

    /// Folds one current lifecycle fact after checking its stream ownership.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small lifecycle fold validates source ownership and preserves only the current state needed by activation"
    )]
    fn apply_transition(
        &mut self,
        transitioned: &TaskTransitioned,
    ) -> Result<(), TaskCommandError> {
        Self::require_task_stream(&transitioned.stream_id, &transitioned.stem)?;
        self.task_mut(&transitioned.stem)?.status = transitioned.status;
        Ok(())
    }

    /// Folds one dependency replacement after checking its source ownership.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small dependency fold validates source ownership and preserves only blockers needed for eligibility"
    )]
    fn apply_links_changed(
        &mut self,
        changed: &tiber_tasks_core::TaskLinksChanged,
    ) -> Result<(), TaskCommandError> {
        Self::require_task_stream(&changed.stream_id, &changed.stem)?;
        self.task_mut(&changed.stem)?
            .blockers
            .clone_from(&changed.blocked_by);
        Ok(())
    }

    /// Replaces the strict order after checking that it is board-authoritative.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the board-order fold validates the sole board authority before preserving the exact sequence"
    )]
    fn apply_board_order(&mut self, order: &TaskOrder) -> Result<(), TaskCommandError> {
        Self::require_board_stream(&order.stream_id)?;
        self.board_order.clone_from(&order.order);
        Ok(())
    }

    /// Requires a task fact to come from its task stream or the board stream.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the narrow stream check derives the exact two-stream activation fence before deciding current authority"
    )]
    fn require_task_stream(stream: &StreamId, task: &TaskId) -> Result<(), TaskCommandError> {
        let [board_stream, task_stream] = acceptance_consistency_streams(task)?;
        if stream == &board_stream || stream == &task_stream {
            Ok(())
        } else {
            Err(TaskCommandError::TaskActivationMalformedHistory)
        }
    }

    /// Requires a board-wide fact to come from the fixed board stream.
    #[expect(
        clippy::implicit_return,
        reason = "the fixed board ownership predicate is clearest as a compact conditional result"
    )]
    fn require_board_stream(stream: &StreamId) -> Result<(), TaskCommandError> {
        if stream.as_ref() == TASK_BOARD_STREAM {
            Ok(())
        } else {
            Err(TaskCommandError::TaskActivationMalformedHistory)
        }
    }

    /// Borrows current task data or reports an activation-relevant malformed fact.
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed lookup and typed malformed-history conversion are clearest as expressions"
    )]
    fn task_mut(&mut self, task: &TaskId) -> Result<&mut ActivationTask, TaskCommandError> {
        self.tasks
            .get_mut(task)
            .ok_or(TaskCommandError::TaskActivationMalformedHistory)
    }

    /// Removes one current task lifetime or reports an activation-relevant malformed fact.
    #[expect(
        clippy::implicit_return,
        reason = "the single current-lifetime removal maps an absent durable task to its typed activation-history failure"
    )]
    fn remove_task(&mut self, task: &TaskId) -> Result<(), TaskCommandError> {
        if self.tasks.remove(task).is_some() {
            Ok(())
        } else {
            Err(TaskCommandError::TaskActivationMalformedHistory)
        }
    }

    /// Decides one first activation, a sole-target retry, or a stable refusal.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the final narrow decision validates exact board membership, one-active-task policy, blockers, and strict eligible order before one modeled publication"
    )]
    fn decide(
        &self,
        request: &StartTask,
    ) -> Result<Option<TaskActivationPublication>, TaskCommandError> {
        let target =
            self.tasks
                .get(request.task())
                .ok_or_else(|| TaskCommandError::TaskMissing {
                    task: request.task().clone(),
                })?;
        let target_order_entries = self
            .board_order
            .iter()
            .filter(|entry| *entry == request.task())
            .count();
        if target_order_entries != 1 {
            return Err(TaskCommandError::TaskActivationOrderDrift {
                task: request.task().clone(),
            });
        }
        let mut active_tasks = Vec::new();
        for (task, state) in &self.tasks {
            if state.status == TaskStatus::InProgress {
                active_tasks.push(task.clone());
            }
        }
        match active_tasks.as_slice() {
            [] => {}
            [active_task] if active_task == request.task() => return Ok(None),
            [active_task] => {
                return Err(TaskCommandError::TaskActivationActiveTask {
                    active_task: active_task.clone(),
                });
            }
            _ => return Err(TaskCommandError::MultipleActiveTasks { active_tasks }),
        }
        if target.status != TaskStatus::Backlog {
            return Err(TaskCommandError::TaskActivationNotBacklog {
                task: request.task().clone(),
                status: target.status,
            });
        }
        if let Some(blocker) = self.first_unresolved_blocker(target) {
            return Err(TaskCommandError::TaskActivationBlocked {
                task: request.task().clone(),
                blocker,
            });
        }
        let next = self
            .first_eligible_backlog_task()
            .ok_or(TaskCommandError::TaskActivationMalformedHistory)?;
        if &next != request.task() {
            return Err(TaskCommandError::TaskActivationNotNextEligible {
                task: request.task().clone(),
                next,
            });
        }
        modeled_task_activation_publication(request).map(Some)
    }

    /// Returns the first target blocker that is missing or not done.
    #[expect(
        clippy::implicit_return,
        reason = "the first stable unresolved blocker makes an activation refusal deterministic without broad projection state"
    )]
    fn first_unresolved_blocker(&self, task: &ActivationTask) -> Option<TaskId> {
        task.blockers.iter().find_map(|blocker| {
            self.tasks
                .get(blocker)
                .is_none_or(|state| state.status != TaskStatus::Done)
                .then(|| blocker.clone())
        })
    }

    /// Returns the first current eligible backlog task in strict board order.
    #[expect(
        clippy::implicit_return,
        reason = "the strict order scan intentionally skips only unrelated stale or terminal entries that cannot become the next backlog task"
    )]
    fn first_eligible_backlog_task(&self) -> Option<TaskId> {
        for task in &self.board_order {
            let Some(state) = self.tasks.get(task) else {
                continue;
            };
            if state.status == TaskStatus::Backlog && self.first_unresolved_blocker(state).is_none()
            {
                return Some(task.clone());
            }
        }
        None
    }
}

/// Modeled external intent for one exact subtask-occurrence check.
#[derive(ModelInput)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the private modeled request remains beside its public exact-occurrence decision so data-flow provenance stays locally inspectable"
)]
struct CheckSubtaskOccurrenceIntent {
    /// Board stream that receives the durable occurrence fact.
    #[model(origin)]
    board_stream: StreamId,
    /// Exact target occurrence selected by the parsed command boundary.
    #[model(origin)]
    occurrence: SubtaskOccurrence,
    /// Complete current unchecked target selected by the caller's query boundary.
    #[model(origin)]
    expected: Subtask,
    /// Exact task selected by the parsed command boundary.
    #[model(origin)]
    task: TaskId,
    /// Legacy task stream whose version also fences the decision.
    #[model(origin)]
    task_stream: StreamId,
}

/// Checked `EventCore` command that produces exactly one internal occurrence fact.
#[derive(ModelCommand)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the checked command retains task-operation field order rather than alphabetizing its semantic boundary"
)]
struct ModeledCheckSubtaskOccurrence {
    /// Board stream that receives the durable occurrence fact.
    #[stream]
    board_stream: StreamId,
    /// Exact target occurrence selected by the parsed command boundary.
    occurrence: SubtaskOccurrence,
    /// Complete current unchecked target selected by the caller's query boundary.
    expected: Subtask,
    /// Exact task selected by the parsed command boundary.
    task: TaskId,
    /// Legacy task stream read for optimistic consistency.
    #[stream]
    task_stream: StreamId,
}

mapping! {
    CheckSubtaskOccurrenceIntentToBoardStream:
        CheckSubtaskOccurrenceIntent.board_stream => ModeledCheckSubtaskOccurrence.board_stream
        using clone;
}

mapping! {
    CheckSubtaskOccurrenceIntentToOccurrence:
        CheckSubtaskOccurrenceIntent.occurrence => ModeledCheckSubtaskOccurrence.occurrence
        using copy;
}

mapping! {
    CheckSubtaskOccurrenceIntentToExpected:
        CheckSubtaskOccurrenceIntent.expected => ModeledCheckSubtaskOccurrence.expected
        using clone;
}

mapping! {
    CheckSubtaskOccurrenceIntentToTask:
        CheckSubtaskOccurrenceIntent.task => ModeledCheckSubtaskOccurrence.task
        using clone;
}

mapping! {
    CheckSubtaskOccurrenceIntentToTaskStream:
        CheckSubtaskOccurrenceIntent.task_stream => ModeledCheckSubtaskOccurrence.task_stream
        using clone;
}

/// Internal modeled fact from which the opaque durable occurrence token is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledSubtaskOccurrenceChecked {
    /// Exact current unchecked occurrence required before checking it.
    expected: Subtask,
    /// Zero-based durable occurrence position.
    index: usize,
    /// Board stream receiving the occurrence check.
    stream: StreamId,
    /// Task owning the checked occurrence.
    task: TaskId,
}

#[expect(
    clippy::implicit_return,
    reason = "the EventCore trait names the internal exact-occurrence fact after its direct stream accessor"
)]
impl Event for ModeledSubtaskOccurrenceChecked {
    fn event_type_name() -> &'static str {
        "TiberModeledSubtaskOccurrenceChecked"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled occurrence fact field for provenance checking.
#[derive(ModelOutput)]
struct ModeledSubtaskOccurrenceCheckedView {
    /// Exact current unchecked occurrence required before checking it.
    expected: Subtask,
    /// Zero-based durable occurrence position.
    index: usize,
    /// Board stream receiving the occurrence check.
    stream: StreamId,
    /// Task owning the checked occurrence.
    task: TaskId,
}

impl ModeledSubtaskOccurrenceCheckedView {
    /// Projects every modeled event field into the durable occurrence-fact boundary shape.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the modeled occurrence view construction is clearest as the final checked builder expression"
    )]
    fn from_event(event: &ModeledSubtaskOccurrenceChecked) -> Self {
        Self::model_builder()
            .stream(ModeledSubtaskOccurrenceCheckedToViewStream::apply(event))
            .task(ModeledSubtaskOccurrenceCheckedToViewTask::apply(event))
            .index(ModeledSubtaskOccurrenceCheckedToViewIndex::apply(event))
            .expected(ModeledSubtaskOccurrenceCheckedToViewExpected::apply(event))
            .build()
            .into_inner()
    }

    /// Converts the modeled occurrence view into the sole durable fact this command permits.
    #[expect(
        clippy::implicit_return,
        reason = "the closed modeled projection converts directly into the sole durable occurrence fact authorized by this command"
    )]
    fn into_task_subtask_occurrence_checked(self) -> TaskSubtaskOccurrenceChecked {
        TaskSubtaskOccurrenceChecked::new(self.stream, self.task, self.index, self.expected)
    }
}

mapping! {
    ModeledSubtaskOccurrenceCheckedToViewStream:
        ModeledSubtaskOccurrenceChecked.stream => ModeledSubtaskOccurrenceCheckedView.stream
        using clone;
}

mapping! {
    ModeledSubtaskOccurrenceCheckedToViewTask:
        ModeledSubtaskOccurrenceChecked.task => ModeledSubtaskOccurrenceCheckedView.task
        using clone;
}

mapping! {
    ModeledSubtaskOccurrenceCheckedToViewIndex:
        ModeledSubtaskOccurrenceChecked.index => ModeledSubtaskOccurrenceCheckedView.index
        using copy;
}

mapping! {
    ModeledSubtaskOccurrenceCheckedToViewExpected:
        ModeledSubtaskOccurrenceChecked.expected => ModeledSubtaskOccurrenceCheckedView.expected
        using clone;
}

/// Minimal modeled state for exactly-once emission within one occurrence-check execution.
#[derive(ModelState)]
struct ModeledCheckSubtaskOccurrenceState {
    /// Whether this modeled command already emitted its one fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the exact occurrence-fact constructor.
#[derive(ModelOutput)]
struct ModeledCheckSubtaskOccurrenceDecision {
    /// Whether the command had already emitted its one fact.
    emitted: bool,
}

mapping! {
    ModeledCheckSubtaskOccurrenceStateToDecision:
        ModeledCheckSubtaskOccurrenceState.emitted => ModeledCheckSubtaskOccurrenceDecision.emitted
        using copy;
}

mapping! {
    ModeledCheckSubtaskOccurrenceToFactStream:
        (ModeledCheckSubtaskOccurrence.board_stream, ModeledCheckSubtaskOccurrenceDecision.emitted) => ModeledSubtaskOccurrenceChecked.stream
        using try correction_stream_once, error = CommandError;
}

mapping! {
    ModeledCheckSubtaskOccurrenceToFactTask:
        ModeledCheckSubtaskOccurrence.task => ModeledSubtaskOccurrenceChecked.task
        using clone;
}

mapping! {
    ModeledCheckSubtaskOccurrenceToFactIndex:
        ModeledCheckSubtaskOccurrence.occurrence => ModeledSubtaskOccurrenceChecked.index
        using durable_subtask_occurrence;
}

mapping! {
    ModeledCheckSubtaskOccurrenceToFactExpected:
        ModeledCheckSubtaskOccurrence.expected => ModeledSubtaskOccurrenceChecked.expected
        using clone;
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "the EventCore trait fixes the evolve/decide API and uses a default stream-discovery method; modeled occurrence construction is the checked terminal expression"
)]
impl ModelCommandLogic for ModeledCheckSubtaskOccurrence {
    type Event = ModeledSubtaskOccurrenceChecked;
    type State = ModeledCheckSubtaskOccurrenceState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledCheckSubtaskOccurrenceDecision::model_builder()
            .emitted(ModeledCheckSubtaskOccurrenceStateToDecision::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            ModeledSubtaskOccurrenceChecked::model_builder()
                .stream(ModeledCheckSubtaskOccurrenceToFactStream::apply((
                    self,
                    decision.as_ref(),
                ))?)
                .task(ModeledCheckSubtaskOccurrenceToFactTask::apply(self))
                .index(ModeledCheckSubtaskOccurrenceToFactIndex::apply(self))
                .expected(ModeledCheckSubtaskOccurrenceToFactExpected::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = true;
        Modeled::from_built(folded)
    }
}

/// Modeled external intent for one closed task-completion publication.
#[derive(ModelInput)]
struct CompleteTaskIntent {
    /// Board stream receiving the terminal and strict-order facts.
    #[model(origin)]
    board_stream: StreamId,
    /// Whether this batch performs the first terminal lifecycle transition.
    #[model(origin)]
    emit_transition: bool,
    /// Complete strict board order after removing every stale target entry.
    #[model(origin)]
    order: Vec<TaskId>,
    /// Exact task selected by the parsed command boundary.
    #[model(origin)]
    task: TaskId,
    /// Legacy task stream whose version also fences the decision.
    #[model(origin)]
    task_stream: StreamId,
}

/// Checked `EventCore` command that produces one internal terminal/order decision fact.
#[derive(ModelCommand)]
struct ModeledCompleteTask {
    /// Board stream receiving the terminal and strict-order facts.
    #[stream]
    board_stream: StreamId,
    /// Whether this batch performs the first terminal lifecycle transition.
    emit_transition: bool,
    /// Complete strict board order after removing every stale target entry.
    order: Vec<TaskId>,
    /// Exact task selected by the parsed command boundary.
    task: TaskId,
    /// Legacy task stream read for optimistic consistency.
    #[stream]
    task_stream: StreamId,
}

mapping! {
    CompleteTaskIntentToBoardStream:
        CompleteTaskIntent.board_stream => ModeledCompleteTask.board_stream
        using clone;
}

mapping! {
    CompleteTaskIntentToEmitTransition:
        CompleteTaskIntent.emit_transition => ModeledCompleteTask.emit_transition
        using copy;
}

mapping! {
    CompleteTaskIntentToOrder:
        CompleteTaskIntent.order => ModeledCompleteTask.order
        using clone;
}

mapping! {
    CompleteTaskIntentToTask:
        CompleteTaskIntent.task => ModeledCompleteTask.task
        using clone;
}

mapping! {
    CompleteTaskIntentToTaskStream:
        CompleteTaskIntent.task_stream => ModeledCompleteTask.task_stream
        using clone;
}

/// Internal modeled fact from which the opaque closed completion token is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledTaskCompletion {
    /// Complete strict board order after removing every stale target entry.
    order: Vec<TaskId>,
    /// Board stream receiving the terminal and strict-order facts.
    stream: StreamId,
    /// Exact task completing or receiving an order-only stale-entry repair.
    task: TaskId,
    /// Whether this batch performs the terminal lifecycle transition.
    transition: bool,
}

#[expect(
    clippy::implicit_return,
    reason = "the EventCore trait names the internal completion decision after its direct stream accessor"
)]
impl Event for ModeledTaskCompletion {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskCompletion"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled completion field for provenance checking.
#[derive(ModelOutput)]
struct ModeledTaskCompletionView {
    /// Complete strict board order after removing every stale target entry.
    order: Vec<TaskId>,
    /// Board stream receiving the terminal and strict-order facts.
    stream: StreamId,
    /// Exact task completing or receiving an order-only stale-entry repair.
    task: TaskId,
    /// Whether this batch performs the terminal lifecycle transition.
    transition: bool,
}

impl ModeledTaskCompletionView {
    /// Projects every modeled field into the closed durable-completion boundary shape.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the modeled completion view construction is clearest as the final checked builder expression"
    )]
    fn from_event(event: &ModeledTaskCompletion) -> Self {
        Self::model_builder()
            .stream(ModeledTaskCompletionToViewStream::apply(event))
            .task(ModeledTaskCompletionToViewTask::apply(event))
            .transition(ModeledTaskCompletionToViewTransition::apply(event))
            .order(ModeledTaskCompletionToViewOrder::apply(event))
            .build()
            .into_inner()
    }

    /// Converts one internal modeled decision into the closed terminal/order durable facts.
    #[expect(
        clippy::implicit_return,
        reason = "the completion view maps its one modeled decision into the optional terminal fact and mandatory strict-order fact without widening the publication vocabulary"
    )]
    fn into_task_completion_facts(self) -> (TaskId, Option<TaskTransitioned>, TaskOrder) {
        let transitioned_fact = self.transition.then(|| {
            TaskTransitioned::new(
                self.stream.clone(),
                self.task.clone(),
                TaskStatus::Done,
                None,
            )
        });
        let reordered_fact = TaskOrder::new(self.stream, self.order);
        (self.task, transitioned_fact, reordered_fact)
    }
}

mapping! {
    ModeledTaskCompletionToViewStream:
        ModeledTaskCompletion.stream => ModeledTaskCompletionView.stream
        using clone;
}

mapping! {
    ModeledTaskCompletionToViewTask:
        ModeledTaskCompletion.task => ModeledTaskCompletionView.task
        using clone;
}

mapping! {
    ModeledTaskCompletionToViewTransition:
        ModeledTaskCompletion.transition => ModeledTaskCompletionView.transition
        using copy;
}

mapping! {
    ModeledTaskCompletionToViewOrder:
        ModeledTaskCompletion.order => ModeledTaskCompletionView.order
        using clone;
}

/// Minimal modeled state for exactly-once emission within one completion execution.
#[derive(ModelState)]
struct ModeledCompleteTaskState {
    /// Whether this modeled command already emitted its one internal decision fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the modeled completion-fact constructor.
#[derive(ModelOutput)]
struct ModeledCompleteTaskDecision {
    /// Whether the command had already emitted its one internal decision fact.
    emitted: bool,
}

mapping! {
    ModeledCompleteTaskStateToDecision:
        ModeledCompleteTaskState.emitted => ModeledCompleteTaskDecision.emitted
        using copy;
}

mapping! {
    ModeledCompleteTaskToFactStream:
        (ModeledCompleteTask.board_stream, ModeledCompleteTaskDecision.emitted) => ModeledTaskCompletion.stream
        using try completion_stream_once, error = CommandError;
}

mapping! {
    ModeledCompleteTaskToFactTask:
        ModeledCompleteTask.task => ModeledTaskCompletion.task
        using clone;
}

mapping! {
    ModeledCompleteTaskToFactTransition:
        ModeledCompleteTask.emit_transition => ModeledTaskCompletion.transition
        using copy;
}

mapping! {
    ModeledCompleteTaskToFactOrder:
        ModeledCompleteTask.order => ModeledTaskCompletion.order
        using clone;
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "the EventCore trait fixes the evolve/decide API and uses a default stream-discovery method; modeled completion construction is the checked terminal expression"
)]
impl ModelCommandLogic for ModeledCompleteTask {
    type Event = ModeledTaskCompletion;
    type State = ModeledCompleteTaskState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledCompleteTaskDecision::model_builder()
            .emitted(ModeledCompleteTaskStateToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledTaskCompletion::model_builder()
                .stream(ModeledCompleteTaskToFactStream::apply((
                    self,
                    decision.as_ref(),
                ))?)
                .task(ModeledCompleteTaskToFactTask::apply(self))
                .transition(ModeledCompleteTaskToFactTransition::apply(self))
                .order(ModeledCompleteTaskToFactOrder::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = true;
        Modeled::from_built(folded)
    }
}

/// Modeled external intent for one closed task-activation publication.
#[derive(ModelInput)]
struct StartTaskIntent {
    /// Board stream receiving the sole activation fact.
    #[model(origin)]
    board_stream: StreamId,
    /// Exact backlog task selected by the parsed command boundary.
    #[model(origin)]
    task: TaskId,
    /// Legacy task stream whose version also fences the decision.
    #[model(origin)]
    task_stream: StreamId,
}

/// Checked `EventCore` command that produces one internal activation decision fact.
#[derive(ModelCommand)]
struct ModeledStartTask {
    /// Board stream receiving the sole activation fact.
    #[stream]
    board_stream: StreamId,
    /// Exact backlog task selected by the parsed command boundary.
    task: TaskId,
    /// Legacy task stream read for optimistic consistency.
    #[stream]
    task_stream: StreamId,
}

mapping! {
    StartTaskIntentToBoardStream:
        StartTaskIntent.board_stream => ModeledStartTask.board_stream
        using clone;
}

mapping! {
    StartTaskIntentToTask:
        StartTaskIntent.task => ModeledStartTask.task
        using clone;
}

mapping! {
    StartTaskIntentToTaskStream:
        StartTaskIntent.task_stream => ModeledStartTask.task_stream
        using clone;
}

/// Internal modeled fact from which the opaque activation token is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledTaskActivation {
    /// Board stream receiving the durable activation fact.
    stream: StreamId,
    /// Exact task entering the active lifecycle.
    task: TaskId,
}

#[expect(
    clippy::implicit_return,
    reason = "the EventCore trait names the internal activation decision after its direct stream accessor"
)]
impl Event for ModeledTaskActivation {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskActivation"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled activation field for provenance checking.
#[derive(ModelOutput)]
struct ModeledTaskActivationView {
    /// Board stream receiving the durable activation fact.
    stream: StreamId,
    /// Exact task entering the active lifecycle.
    task: TaskId,
}

impl ModeledTaskActivationView {
    /// Projects every modeled activation field into the durable-fact boundary shape.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the modeled activation view construction is clearest as the final checked builder expression"
    )]
    fn from_event(event: &ModeledTaskActivation) -> Self {
        Self::model_builder()
            .stream(ModeledTaskActivationToViewStream::apply(event))
            .task(ModeledTaskActivationToViewTask::apply(event))
            .build()
            .into_inner()
    }

    /// Converts the modeled activation decision into its sole durable lifecycle fact.
    #[expect(
        clippy::implicit_return,
        reason = "the closed activation view maps directly into one unclaimed in-progress fact"
    )]
    fn into_task_transitioned(self) -> TaskTransitioned {
        TaskTransitioned::new(self.stream, self.task, TaskStatus::InProgress, None)
    }
}

mapping! {
    ModeledTaskActivationToViewStream:
        ModeledTaskActivation.stream => ModeledTaskActivationView.stream
        using clone;
}

mapping! {
    ModeledTaskActivationToViewTask:
        ModeledTaskActivation.task => ModeledTaskActivationView.task
        using clone;
}

/// Minimal modeled state for exactly-once emission within one activation execution.
#[derive(ModelState)]
struct ModeledStartTaskState {
    /// Whether this modeled command already emitted its one activation fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the modeled activation-fact constructor.
#[derive(ModelOutput)]
struct ModeledStartTaskDecision {
    /// Whether the command had already emitted its one activation fact.
    emitted: bool,
}

mapping! {
    ModeledStartTaskStateToDecision:
        ModeledStartTaskState.emitted => ModeledStartTaskDecision.emitted
        using copy;
}

mapping! {
    ModeledStartTaskToFactStream:
        (ModeledStartTask.board_stream, ModeledStartTaskDecision.emitted) => ModeledTaskActivation.stream
        using try activation_stream_once, error = CommandError;
}

mapping! {
    ModeledStartTaskToFactTask:
        ModeledStartTask.task => ModeledTaskActivation.task
        using clone;
}

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "the EventCore trait fixes the evolve/decide API and uses a default stream-discovery method; modeled activation construction is the checked terminal expression"
)]
impl ModelCommandLogic for ModeledStartTask {
    type Event = ModeledTaskActivation;
    type State = ModeledStartTaskState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledStartTaskDecision::model_builder()
            .emitted(ModeledStartTaskStateToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledTaskActivation::model_builder()
                .stream(ModeledStartTaskToFactStream::apply((
                    self,
                    decision.as_ref(),
                ))?)
                .task(ModeledStartTaskToFactTask::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = true;
        Modeled::from_built(folded)
    }
}

/// Produces the board stream only when the modeled command has not emitted a correction.
#[expect(
    clippy::implicit_return,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore mapping API passes model fields by reference, so this narrow one-use transform retains its checked signature"
)]
fn correction_stream_once(stream: &StreamId, emitted: &bool) -> Result<StreamId, CommandError> {
    if *emitted {
        return Err(CommandError::ValidationError(
            "tasks_command_subtask_correction_already_emitted".to_owned(),
        ));
    }
    Ok(stream.clone())
}

/// Produces the board stream only when the modeled completion has not emitted its decision.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore mapping API passes model fields by reference, so this narrow one-use transform retains its checked signature"
)]
fn completion_stream_once(stream: &StreamId, emitted: &bool) -> Result<StreamId, CommandError> {
    if *emitted {
        return Err(CommandError::ValidationError(
            "tasks_command_completion_already_emitted".to_owned(),
        ));
    }
    Ok(stream.clone())
}

/// Produces the board stream only when the modeled activation has not emitted its decision.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore mapping API passes model fields by reference, so this narrow one-use transform retains its checked signature"
)]
fn activation_stream_once(stream: &StreamId, emitted: &bool) -> Result<StreamId, CommandError> {
    if *emitted {
        return Err(CommandError::ValidationError(
            "tasks_command_task_activation_already_emitted".to_owned(),
        ));
    }
    Ok(stream.clone())
}

/// Converts the semantic occurrence to the persisted zero-based position.
#[expect(
    clippy::implicit_return,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore mapping API passes model fields by reference, so this small one-use transform keeps the modeled signature exact"
)]
fn durable_subtask_occurrence(occurrence: &SubtaskOccurrence) -> usize {
    occurrence.zero_based_value()
}

mapping! {
    ModeledRepairDuplicateSubtaskIdToFactIndex:
        ModeledRepairDuplicateSubtaskId.occurrence => ModeledSubtaskIdCorrected.index
        using durable_subtask_occurrence;
}

/// Executes the checked `EventCore` model and returns its one closed durable correction token.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the narrow pure correction maps exact parsed intent through one checked EventCore command and keeps model failures typed"
)]
fn modeled_subtask_correction_publication(
    request: &RepairDuplicateSubtaskId,
) -> Result<SubtaskIdCorrectionPublication, TaskCommandError> {
    let consistency_streams = acceptance_consistency_streams(request.task())?;
    let intent = RepairDuplicateSubtaskIdIntent::model_builder()
        .board_stream(consistency_streams[0].clone())
        .task_stream(consistency_streams[1].clone())
        .task(request.task().clone())
        .occurrence(request.occurrence())
        .expected(request.expected().clone())
        .replacement_id(request.replacement_id().to_owned())
        .build();
    let command = ModeledRepairDuplicateSubtaskId::model_builder()
        .board_stream(RepairDuplicateSubtaskIdIntentToBoardStream::apply(
            intent.as_ref(),
        ))
        .task_stream(RepairDuplicateSubtaskIdIntentToTaskStream::apply(
            intent.as_ref(),
        ))
        .task(RepairDuplicateSubtaskIdIntentToTask::apply(intent.as_ref()))
        .occurrence(RepairDuplicateSubtaskIdIntentToOccurrence::apply(
            intent.as_ref(),
        ))
        .expected(RepairDuplicateSubtaskIdIntentToExpected::apply(
            intent.as_ref(),
        ))
        .replacement_id(RepairDuplicateSubtaskIdIntentToReplacementId::apply(
            intent.as_ref(),
        ))
        .build();
    let events: Vec<ModeledSubtaskIdCorrected> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledSubtaskCorrectionDecisionFailed)?
        .into();
    let [event]: [ModeledSubtaskIdCorrected; 1] =
        events
            .try_into()
            .map_err(|_events: Vec<ModeledSubtaskIdCorrected>| {
                TaskCommandError::InvalidModeledSubtaskCorrectionPublication
            })?;
    let fact = ModeledSubtaskIdCorrectedView::from_event(&event).into_task_subtask_id_corrected();
    SubtaskIdCorrectionPublication::from_modeled_fact(fact, consistency_streams)
}

/// Executes the checked `EventCore` model and returns its one closed durable occurrence token.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the narrow pure occurrence check maps exact parsed intent through one checked EventCore command and keeps model failures typed"
)]
fn modeled_subtask_occurrence_publication(
    request: &CheckSubtaskOccurrence,
) -> Result<SubtaskOccurrenceCheckPublication, TaskCommandError> {
    let consistency_streams = acceptance_consistency_streams(request.task())?;
    let intent = CheckSubtaskOccurrenceIntent::model_builder()
        .board_stream(consistency_streams[0].clone())
        .task_stream(consistency_streams[1].clone())
        .task(request.task().clone())
        .occurrence(request.occurrence())
        .expected(request.expected().clone())
        .build();
    let command = ModeledCheckSubtaskOccurrence::model_builder()
        .board_stream(CheckSubtaskOccurrenceIntentToBoardStream::apply(
            intent.as_ref(),
        ))
        .task_stream(CheckSubtaskOccurrenceIntentToTaskStream::apply(
            intent.as_ref(),
        ))
        .task(CheckSubtaskOccurrenceIntentToTask::apply(intent.as_ref()))
        .occurrence(CheckSubtaskOccurrenceIntentToOccurrence::apply(
            intent.as_ref(),
        ))
        .expected(CheckSubtaskOccurrenceIntentToExpected::apply(
            intent.as_ref(),
        ))
        .build();
    let events: Vec<ModeledSubtaskOccurrenceChecked> =
        CommandLogic::handle(&command, Modeled::default())
            .map_err(|_source| TaskCommandError::ModeledSubtaskOccurrenceDecisionFailed)?
            .into();
    let [event]: [ModeledSubtaskOccurrenceChecked; 1] =
        events
            .try_into()
            .map_err(|_events: Vec<ModeledSubtaskOccurrenceChecked>| {
                TaskCommandError::InvalidModeledSubtaskOccurrencePublication
            })?;
    let fact = ModeledSubtaskOccurrenceCheckedView::from_event(&event)
        .into_task_subtask_occurrence_checked();
    SubtaskOccurrenceCheckPublication::from_modeled_fact(fact, consistency_streams)
}

/// Executes the checked `EventCore` model and returns one closed terminal/order publication.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the narrow pure completion maps its exact parsed intent through one checked EventCore command and keeps model failures typed"
)]
fn modeled_task_completion_publication(
    task: &TaskId,
    emit_transition: bool,
    order: Vec<TaskId>,
) -> Result<TaskCompletionPublication, TaskCommandError> {
    let consistency_streams = acceptance_consistency_streams(task)?;
    let intent = CompleteTaskIntent::model_builder()
        .board_stream(consistency_streams[0].clone())
        .task_stream(consistency_streams[1].clone())
        .task(task.clone())
        .emit_transition(emit_transition)
        .order(order)
        .build();
    let command = ModeledCompleteTask::model_builder()
        .board_stream(CompleteTaskIntentToBoardStream::apply(intent.as_ref()))
        .task_stream(CompleteTaskIntentToTaskStream::apply(intent.as_ref()))
        .task(CompleteTaskIntentToTask::apply(intent.as_ref()))
        .emit_transition(CompleteTaskIntentToEmitTransition::apply(intent.as_ref()))
        .order(CompleteTaskIntentToOrder::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledTaskCompletion> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::InvalidModeledTaskCompletionPublication)?
        .into();
    let [event]: [ModeledTaskCompletion; 1] =
        events
            .try_into()
            .map_err(|_events: Vec<ModeledTaskCompletion>| {
                TaskCommandError::InvalidModeledTaskCompletionPublication
            })?;
    let (modeled_task, transitioned_fact, reordered_fact) =
        ModeledTaskCompletionView::from_event(&event).into_task_completion_facts();
    TaskCompletionPublication::from_modeled_facts(
        modeled_task,
        transitioned_fact,
        reordered_fact,
        consistency_streams,
    )
}

/// Executes the checked `EventCore` model and returns one closed activation publication.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the narrow pure activation maps one exact parsed intent through a checked EventCore command and keeps model failures typed"
)]
fn modeled_task_activation_publication(
    request: &StartTask,
) -> Result<TaskActivationPublication, TaskCommandError> {
    let consistency_streams = acceptance_consistency_streams(request.task())?;
    let intent = StartTaskIntent::model_builder()
        .board_stream(consistency_streams[0].clone())
        .task(request.task().clone())
        .task_stream(consistency_streams[1].clone())
        .build();
    let command = ModeledStartTask::model_builder()
        .board_stream(StartTaskIntentToBoardStream::apply(intent.as_ref()))
        .task(StartTaskIntentToTask::apply(intent.as_ref()))
        .task_stream(StartTaskIntentToTaskStream::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledTaskActivation> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledTaskActivationDecisionFailed)?
        .into();
    let [event]: [ModeledTaskActivation; 1] =
        events
            .try_into()
            .map_err(|_events: Vec<ModeledTaskActivation>| {
                TaskCommandError::InvalidModeledTaskActivationPublication
            })?;
    let fact = ModeledTaskActivationView::from_event(&event).into_task_transitioned();
    TaskActivationPublication::from_modeled_fact(fact, consistency_streams)
}

/// Produces the only permitted acceptance state after rejecting a repeated modeled command.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore fallible mapping API passes model fields by reference, so the narrow one-use transform retains its checked signature"
)]
fn checked_once(emitted: &bool) -> Result<bool, CommandError> {
    if *emitted {
        return Err(CommandError::ValidationError(
            "tasks_command_acceptance_already_checked".to_owned(),
        ));
    }
    Ok(true)
}

/// Converts the semantic command index to the persisted zero-based position.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the EventCore mapping API passes model fields by reference, so this small one-use transform keeps the modeled signature exact"
)]
fn durable_acceptance_index(index: &AcceptanceIndex) -> usize {
    index.zero_based_value()
}

/// Executes the checked `EventCore` model and returns its one closed durable publication token.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the narrow pure decision maps its exact parsed intent through one checked EventCore command and keeps model failures typed"
)]
fn modeled_acceptance_publication(
    request: &CheckAcceptance,
) -> Result<AcceptanceCheckPublication, TaskCommandError> {
    let consistency_streams = acceptance_consistency_streams(request.task())?;
    let intent = CheckAcceptanceIntent::model_builder()
        .board_stream(consistency_streams[0].clone())
        .task_stream(consistency_streams[1].clone())
        .task(request.task().clone())
        .index(request.index())
        .build();
    let command = ModeledCheckAcceptance::model_builder()
        .board_stream(CheckAcceptanceIntentToBoardStream::apply(intent.as_ref()))
        .task_stream(CheckAcceptanceIntentToTaskStream::apply(intent.as_ref()))
        .task(CheckAcceptanceIntentToTask::apply(intent.as_ref()))
        .index(CheckAcceptanceIntentToIndex::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledAcceptanceChecked> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledAcceptanceDecisionFailed)?
        .into();
    let [event]: [ModeledAcceptanceChecked; 1] =
        events
            .try_into()
            .map_err(|_events: Vec<ModeledAcceptanceChecked>| {
                TaskCommandError::InvalidModeledAcceptancePublication
            })?;
    let fact = ModeledAcceptanceCheckedView::from_event(&event).into_task_acceptance_checked();
    AcceptanceCheckPublication::from_modeled_fact(fact, consistency_streams)
}

/// Returns the board and exact legacy task streams whose versions fence this decision.
///
/// # Errors
///
/// Returns [`TaskCommandError::InvalidTaskStream`] only if a durable task ID
/// cannot form an `EventCore` stream identity.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the two bounded stream identities preserve a direct typed construction boundary"
)]
pub fn acceptance_consistency_streams(task: &TaskId) -> Result<[StreamId; 2], TaskCommandError> {
    let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    Ok([board, task_stream])
}

/// Decides the two-fact publication for one new backlog task.
///
/// The fold retains only current task identities and strict open-task order.
/// It never uses the broad read projection as write authority.
///
/// # Errors
///
/// Returns a typed task-command failure when retained history or the modeled
/// publication violates creation authority.
#[inline]
#[expect(
    clippy::implicit_return,
    reason = "the public forwarding boundary preserves the typed decision result"
)]
pub fn decide_create_task(
    events: &[TaskEvent],
    request: &CreateTask,
) -> Result<TaskCreationDecision, TaskCommandError> {
    task_administration::decide_create_task(events, request)
}

/// Decides the check-only publication needed to check a current acceptance item.
///
/// The caller supplies all relevant facts in canonical transaction order. This
/// fold retains only the addressed task's checklist; it is deliberately not a
/// task-board projection or generic mutable task aggregate.
///
/// # Errors
///
/// Returns a stable typed failure for malformed target history or an absent
/// task/checklist position. `Ok(None)` means the item is already checked and
/// no durable fact should be appended.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the check-only command stays a compact pure fold with explicit typed propagation"
)]
pub fn decide_check_acceptance(
    events: &[TaskEvent],
    request: &CheckAcceptance,
) -> Result<Option<AcceptanceCheckPublication>, TaskCommandError> {
    let state = AcceptanceChecklistState::fold(events, request.task())?;
    state.decide(request)
}

/// Decides the closed correction needed to repair one duplicate legacy subtask identifier.
///
/// The caller supplies all relevant facts in canonical transaction order. This
/// fold retains only the addressed task's subtask occurrences; it is
/// deliberately not a task-board projection or generic mutable task aggregate.
///
/// # Errors
///
/// Returns a stable typed failure for malformed target history, a stale exact
/// preimage, a non-duplicate target, or an already-used replacement identity.
/// `Ok(None)` means the exact correction fact is already present in canonical
/// history and no durable fact should be appended.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the correction stays a compact pure fold with explicit typed propagation"
)]
pub fn decide_repair_duplicate_subtask_id(
    events: &[TaskEvent],
    request: &RepairDuplicateSubtaskId,
) -> Result<Option<SubtaskIdCorrectionPublication>, TaskCommandError> {
    let state = SubtaskCorrectionState::fold(events, request)?;
    state.decide(request)
}

/// Decides the exact occurrence-check publication needed to check one current subtask.
///
/// The caller supplies relevant facts in canonical transaction order. This
/// fold retains only the addressed task's lifetime, lifecycle, and subtask
/// occurrences; it is deliberately not a task-board projection or a generic
/// mutable task aggregate.
///
/// # Errors
///
/// Returns a stable typed failure for malformed target history, a stale exact
/// preimage, an absent occurrence, or a target that is no longer in progress.
/// `Ok(None)` means this exact occurrence was already checked in its current
/// task lifetime and no durable fact should be appended.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the exact occurrence command stays a compact pure fold with explicit typed propagation"
)]
pub fn decide_check_subtask_occurrence(
    events: &[TaskEvent],
    request: &CheckSubtaskOccurrence,
) -> Result<Option<SubtaskOccurrenceCheckPublication>, TaskCommandError> {
    let state = SubtaskOccurrenceCheckState::fold(events, request)?;
    state.decide(request)
}

/// Decides the closed activation publication needed to start one strict-next task.
///
/// The caller supplies relevant facts in canonical transaction order. This
/// fold retains only current lifecycle, blockers, and strict board order; it
/// is deliberately not a task-board projection or generic lifecycle setter.
///
/// # Errors
///
/// Returns a stable typed failure for malformed activation-relevant history,
/// board-order drift, another active task, blocked prerequisites, or a request
/// that would bypass the next eligible backlog task. `Ok(None)` means the
/// addressed task is already the sole active task in current history.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the activation command stays a compact pure fold with explicit typed propagation"
)]
pub fn decide_start_task(
    events: &[TaskEvent],
    request: &StartTask,
) -> Result<Option<TaskActivationPublication>, TaskCommandError> {
    let state = TaskActivationState::fold(events)?;
    state.decide(request)
}

/// Decides the closed completion publication needed to finish one current task.
///
/// The caller supplies relevant facts in canonical transaction order. This
/// fold retains only the addressed task's lifecycle and current requirements,
/// plus the strict board order that must lose every stale target entry. It is
/// deliberately not a task-board projection or generic mutable task model.
///
/// # Errors
///
/// Returns a stable typed failure for malformed target history, a task that is
/// not in progress, or any unchecked current acceptance/subtask requirement.
/// `Ok(None)` means the task is already done and absent from current board
/// order; an already-done task still named by that order produces an order-only
/// repair publication.
#[inline]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the completion command stays a compact pure fold with explicit typed propagation"
)]
pub fn decide_complete_task(
    events: &[TaskEvent],
    request: &CompleteTask,
) -> Result<Option<TaskCompletionPublication>, TaskCommandError> {
    let state = TaskCompletionState::fold(events, request)?;
    state.decide(request)
}
