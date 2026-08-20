//! Modeled decision for reopening one abandoned task into the backlog.

use alloc::{collections::BTreeSet, vec::Vec};
use eventcore::{
    CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput, ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskEvent, TaskId, TaskOrder, TaskStatus, TaskTransitioned};

use super::{TASK_BOARD_STREAM, TaskCommandError};
use crate::TaskReopeningPublication;

/// Request to reopen one exact abandoned task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReopenTask {
    /// Addressed durable task identity.
    task: TaskId,
}

impl ReopenTask {
    /// Creates one semantic reopening request.
    #[must_use]
    pub const fn new(task: TaskId) -> Self {
        Self { task }
    }

    /// Returns the addressed durable task identity.
    #[must_use]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }
}

#[derive(Clone, Debug)]
/// Minimal semantic reopening plan.
struct Plan {
    /// Canonical board stream.
    board: StreamId,
    /// Resulting complete open-board order.
    order: Vec<TaskId>,
    /// Addressed task identity.
    task: TaskId,
    /// Canonical addressed-task stream.
    task_stream: StreamId,
}

#[derive(ModelInput)]
/// Provenance-bearing reopening input.
struct ReopeningIntent {
    /// Complete semantic reopening plan.
    #[model(origin)]
    plan: Plan,
}

#[derive(ModelCommand)]
/// Checked command for one abandoned-to-backlog reopening.
struct ReopeningCommand {
    /// Canonical board stream receiving the modeled fact.
    #[stream]
    board: StreamId,
    /// Complete semantic reopening plan.
    plan: Plan,
}

mapping! { ReopeningIntentPlan: ReopeningIntent.plan => ReopeningCommand.plan using clone; }
mapping! { ReopeningIntentBoard: ReopeningIntent.plan => ReopeningCommand.board using plan_board; }
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact consumed into reopening's durable facts.
struct ReopeningFact {
    /// Canonical board stream.
    board: StreamId,
    /// Whether the single modeled fact was emitted.
    emitted: bool,
    /// Resulting complete open-board order.
    order: Vec<TaskId>,
    /// Addressed task identity.
    task: TaskId,
    /// Canonical addressed-task stream.
    task_stream: StreamId,
}

impl Event for ReopeningFact {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskReopened"
    }
    fn stream_id(&self) -> &StreamId {
        &self.board
    }
}

#[derive(ModelState)]
/// Single-emission checked-model state.
struct ReopeningState {
    /// Whether the modeled fact has already evolved.
    #[model(default)]
    emitted: bool,
}

#[derive(ModelOutput)]
/// Checked single-emission decision.
struct ReopeningDecision {
    /// Whether the modeled fact has already evolved.
    emitted: bool,
}

#[derive(ModelOutput)]
/// Provenance-consuming view used to construct the opaque publication.
struct ReopeningView {
    /// Canonical board stream.
    board: StreamId,
    /// Whether the modeled fact was emitted.
    emitted: bool,
    /// Resulting complete open-board order.
    order: Vec<TaskId>,
    /// Addressed task identity.
    task: TaskId,
    /// Canonical addressed-task stream.
    task_stream: StreamId,
}

mapping! { ReopeningStateDecision: ReopeningState.emitted => ReopeningDecision.emitted using copy; }
mapping! { ReopeningFactBoard: ReopeningCommand.board => ReopeningFact.board using clone; }
mapping! { ReopeningFactEmitted: ReopeningDecision.emitted => ReopeningFact.emitted using invert; }
mapping! { ReopeningFactOrder: ReopeningCommand.plan => ReopeningFact.order using plan_order; }
mapping! { ReopeningFactTask: ReopeningCommand.plan => ReopeningFact.task using plan_task; }
mapping! { ReopeningFactTaskStream: ReopeningCommand.plan => ReopeningFact.task_stream using plan_task_stream; }
mapping! { ReopeningViewBoard: ReopeningFact.board => ReopeningView.board using clone; }
mapping! { ReopeningViewEmitted: ReopeningFact.emitted => ReopeningView.emitted using copy; }
mapping! { ReopeningViewOrder: ReopeningFact.order => ReopeningView.order using clone; }
mapping! { ReopeningViewTask: ReopeningFact.task => ReopeningView.task using clone; }
mapping! { ReopeningViewTaskStream: ReopeningFact.task_stream => ReopeningView.task_stream using clone; }

#[expect(
    clippy::missing_trait_methods,
    reason = "the EventCore command uses the trait's default stream selection and error hooks"
)]
impl ModelCommandLogic for ReopeningCommand {
    type Event = ReopeningFact;
    type State = ReopeningState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, eventcore::CommandError> {
        let decision = ReopeningDecision::model_builder()
            .emitted(ReopeningStateDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ReopeningFact::model_builder()
                .board(ReopeningFactBoard::apply(self))
                .emitted(ReopeningFactEmitted::apply(decision.as_ref()))
                .order(ReopeningFactOrder::apply(self))
                .task(ReopeningFactTask::apply(self))
                .task_stream(ReopeningFactTaskStream::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        Modeled::from_built(ReopeningState {
            emitted: state.as_ref().emitted || event.emitted,
        })
    }
}

/// Converts not-yet-emitted state into the one modeled emission marker.
#[expect(
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the named mapping helper has one checked-model caller whose generated contract passes its field by reference"
)]
const fn invert(value: &bool) -> bool {
    !*value
}
/// Selects the canonical board stream from the reopening plan.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_board(plan: &Plan) -> StreamId {
    plan.board.clone()
}
/// Copies the resulting complete order into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_order(plan: &Plan) -> Vec<TaskId> {
    plan.order.clone()
}
/// Copies the addressed task identity into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_task(plan: &Plan) -> TaskId {
    plan.task.clone()
}
/// Copies the canonical addressed-task stream into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_task_stream(plan: &Plan) -> StreamId {
    plan.task_stream.clone()
}

/// Decides the exact abandoned-to-backlog transition and board-tail insertion.
///
/// # Errors
///
/// Returns a typed command error when canonical retained history is malformed,
/// the task is absent or not abandoned, or the checked model cannot close the
/// exact transition/order publication.
#[inline]
#[expect(
    clippy::match_same_arms,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    clippy::too_many_lines,
    reason = "the command folds a borrowed non-exhaustive event vocabulary at one semantic boundary; known irrelevant facts are explicit and the wildcard preserves forward compatibility"
)]
pub fn decide(
    events: &[TaskEvent],
    request: &ReopenTask,
) -> Result<TaskReopeningPublication, TaskCommandError> {
    let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
        .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", request.task.as_str()))
        .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?;
    let mut exists = false;
    let mut status = None;
    let mut order = Vec::new();
    let mut order_observed = false;
    for event in events {
        match event {
            TaskEvent::TaskCreated(created) if created.task.stem == request.task => {
                if created.stream_id != board && created.stream_id != task_stream {
                    return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                        task: request.task.clone(),
                        stream: created.stream_id.clone(),
                    });
                }
                if exists {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                exists = true;
                status = Some(created.task.status);
            }
            TaskEvent::TaskTransitioned(changed) if changed.stem == request.task => {
                if changed.stream_id != board && changed.stream_id != task_stream {
                    return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                        task: request.task.clone(),
                        stream: changed.stream_id.clone(),
                    });
                }
                if !exists {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                let Some(previous) = status else {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                };
                if !allowed_transition(previous, changed.status) {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                status = Some(changed.status);
            }
            TaskEvent::TaskPriorityChanged(changed) | TaskEvent::BoardReordered(changed) => {
                if changed.stream_id != board {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                if has_duplicates(&changed.order) {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                order_observed = true;
                order.clone_from(&changed.order);
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                if repaired.stream_id != board {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                if let Some(changed) = repaired.order_change.as_ref() {
                    if changed.stream_id != board {
                        return Err(TaskCommandError::TaskReopeningMalformedHistory);
                    }
                    if has_duplicates(&changed.order) {
                        return Err(TaskCommandError::TaskReopeningMalformedHistory);
                    }
                    order_observed = true;
                    order.clone_from(&changed.order);
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                if closed.stream_id != board {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                if has_duplicates(&closed.order) {
                    return Err(TaskCommandError::TaskReopeningMalformedHistory);
                }
                order_observed = true;
                if closed.stems.contains(&request.task) {
                    status = Some(TaskStatus::Done);
                }
                order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) if closed.stem == request.task => {
                if closed.stream_id != board && closed.stream_id != task_stream {
                    return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                        task: request.task.clone(),
                        stream: closed.stream_id.clone(),
                    });
                }
                status = Some(TaskStatus::Done);
            }
            TaskEvent::HistoricalTaskRemoved(removed) if removed.stem == request.task => {
                if removed.stream_id != board && removed.stream_id != task_stream {
                    return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                        task: request.task.clone(),
                        stream: removed.stream_id.clone(),
                    });
                }
                exists = false;
                status = None;
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
            _ => {}
        }
    }
    let Some(current) = status else {
        return Err(TaskCommandError::TaskMissing {
            task: request.task.clone(),
        });
    };
    if !order_observed {
        return Err(TaskCommandError::TaskReopeningMalformedHistory);
    }
    if current != TaskStatus::Abandoned {
        return Err(TaskCommandError::TaskReopeningNotAbandoned {
            task: request.task.clone(),
            status: current,
        });
    }
    if order.contains(&request.task) {
        return Err(TaskCommandError::TaskReopeningMalformedHistory);
    }
    order.push(request.task.clone());
    let plan = Plan {
        board: board.clone(),
        order,
        task: request.task.clone(),
        task_stream: task_stream.clone(),
    };
    let intent = ReopeningIntent::model_builder().plan(plan).build();
    let command = ReopeningCommand::model_builder()
        .board(ReopeningIntentBoard::apply(intent.as_ref()))
        .plan(ReopeningIntentPlan::apply(intent.as_ref()))
        .build();
    let modeled_facts: Vec<ReopeningFact> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_model_error| TaskCommandError::ModeledTaskReopeningDecisionFailed)?
        .into();
    let [fact]: [ReopeningFact; 1] = modeled_facts
        .try_into()
        .map_err(|_fact_count| TaskCommandError::InvalidModeledTaskReopeningPublication)?;
    let view = ReopeningView::model_builder()
        .board(ReopeningViewBoard::apply(&fact))
        .emitted(ReopeningViewEmitted::apply(&fact))
        .order(ReopeningViewOrder::apply(&fact))
        .task(ReopeningViewTask::apply(&fact))
        .task_stream(ReopeningViewTaskStream::apply(&fact))
        .build();
    if !view.as_ref().emitted {
        return Err(TaskCommandError::InvalidModeledTaskReopeningPublication);
    }
    TaskReopeningPublication::from_modeled_facts(
        TaskTransitioned::new(
            view.as_ref().task_stream.clone(),
            view.as_ref().task.clone(),
            TaskStatus::Backlog,
            None,
        ),
        TaskOrder::new(view.as_ref().board.clone(), view.as_ref().order.clone()),
        [board, task_stream],
    )
}

/// Returns whether a purported strict board order repeats any identity.
fn has_duplicates(order: &[TaskId]) -> bool {
    let mut seen = BTreeSet::new();
    order.iter().any(|task| !seen.insert(task))
}

/// Returns whether one retained lifecycle transition is semantically legal.
#[expect(
    clippy::single_call_fn,
    reason = "the reopening fold has one lifecycle validation helper"
)]
const fn allowed_transition(from: TaskStatus, to: TaskStatus) -> bool {
    matches!(
        (from, to),
        (
            TaskStatus::Backlog,
            TaskStatus::InProgress | TaskStatus::Abandoned
        ) | (
            TaskStatus::InProgress,
            TaskStatus::Backlog | TaskStatus::Done | TaskStatus::Abandoned
        ) | (TaskStatus::Done, TaskStatus::InProgress)
            | (TaskStatus::Abandoned, TaskStatus::Backlog)
    )
}
