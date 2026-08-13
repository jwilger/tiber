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
    ChecklistItem, TaskAcceptanceChecked, TaskAcceptanceRemoved, TaskCreated, TaskEvent, TaskId,
};

use crate::AcceptanceCheckPublication;

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
    /// The requested current checklist position is absent.
    AcceptanceItemMissing {
        /// The task owning the absent current checklist position.
        task: TaskId,
        /// The zero-based absent requested position.
        index: AcceptanceIndex,
    },
    /// The supplied history contains a task fact newer than this command understands.
    UnsupportedTaskEvent,
    /// The derived task stream identity was rejected by `EventCore`.
    InvalidTaskStream,
    /// The checked `EventCore` command could not produce its one internal acceptance fact.
    ModeledAcceptanceDecisionFailed,
    /// The checked `EventCore` command did not produce exactly one internal acceptance fact.
    InvalidModeledAcceptancePublication,
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
            Self::TaskMissing { .. } => "tasks_command_task_missing",
            Self::DuplicateTaskCreation { .. } => "tasks_command_duplicate_task_creation",
            Self::TargetTaskFactUnexpectedStream { .. } => {
                "tasks_command_target_task_fact_unexpected_stream"
            }
            Self::HistoryAcceptanceItemMissing { .. } => {
                "tasks_command_history_acceptance_item_missing"
            }
            Self::AcceptanceItemMissing { .. } => "tasks_command_acceptance_item_missing",
            Self::UnsupportedTaskEvent => "tasks_command_unsupported_task_event",
            Self::InvalidTaskStream => "tasks_command_invalid_task_stream",
            Self::ModeledAcceptanceDecisionFailed => {
                "tasks_command_modeled_acceptance_decision_failed"
            }
            Self::InvalidModeledAcceptancePublication => {
                "tasks_command_invalid_modeled_acceptance_publication"
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
