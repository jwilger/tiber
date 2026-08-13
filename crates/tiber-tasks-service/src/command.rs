//! Command-specific, pure decisions for native Tiber Tasks.
//!
//! These folds consume canonical transaction-order task facts supplied by an
//! adapter. They deliberately do not reuse [`crate::TaskBoardProjection`],
//! whose broad state is query-only rather than write authority.

use alloc::vec::Vec;
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
    TaskId, TaskSubtaskChecked, TaskSubtaskIdCorrected,
};

use crate::{AcceptanceCheckPublication, SubtaskIdCorrectionPublication};

/// The native board stream receiving task-mutation facts.
pub const TASK_BOARD_STREAM: &str = "tiber:board";

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
    /// The supplied history contains a task fact newer than this command understands.
    UnsupportedTaskEvent,
    /// The derived task stream identity was rejected by `EventCore`.
    InvalidTaskStream,
    /// The checked `EventCore` command could not produce its one internal acceptance fact.
    ModeledAcceptanceDecisionFailed,
    /// The checked `EventCore` command could not produce its one correction fact.
    ModeledSubtaskCorrectionDecisionFailed,
    /// The checked `EventCore` command did not produce exactly one internal acceptance fact.
    InvalidModeledAcceptancePublication,
    /// The checked `EventCore` command did not produce exactly one correction fact.
    InvalidModeledSubtaskCorrectionPublication,
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
            Self::AcceptanceItemMissing { .. } => "tasks_command_acceptance_item_missing",
            Self::SubtaskOccurrenceMissing { .. } => "tasks_command_subtask_occurrence_missing",
            Self::SubtaskCorrectionPreimageMismatch { .. } => {
                "tasks_command_subtask_correction_preimage_mismatch"
            }
            Self::SubtaskIdNotDuplicate { .. } => "tasks_command_subtask_id_not_duplicate",
            Self::SubtaskReplacementIdAlreadyExists { .. } => {
                "tasks_command_subtask_replacement_id_already_exists"
            }
            Self::UnsupportedTaskEvent => "tasks_command_unsupported_task_event",
            Self::InvalidTaskStream => "tasks_command_invalid_task_stream",
            Self::ModeledAcceptanceDecisionFailed => {
                "tasks_command_modeled_acceptance_decision_failed"
            }
            Self::ModeledSubtaskCorrectionDecisionFailed => {
                "tasks_command_modeled_subtask_correction_decision_failed"
            }
            Self::InvalidModeledAcceptancePublication => {
                "tasks_command_invalid_modeled_acceptance_publication"
            }
            Self::InvalidModeledSubtaskCorrectionPublication => {
                "tasks_command_invalid_modeled_subtask_correction_publication"
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

/// Produces the board stream only when the modeled command has not emitted a correction.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
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

/// Converts the semantic occurrence to the persisted zero-based position.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
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
