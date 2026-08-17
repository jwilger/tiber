//! Modeled decision for abandoning one open task.

use alloc::vec::Vec;
use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskClaim, TaskEvent, TaskId, TaskOrder, TaskStatus, TaskTransitioned};

use super::{TASK_BOARD_STREAM, TaskCommandError};
use crate::TaskAbandonmentPublication;

/// Request to abandon one current open task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbandonTask {
    /// Exact durable task identity addressed by the request.
    task: TaskId,
}
impl AbandonTask {
    /// Creates one semantic abandonment request.
    #[must_use]
    #[expect(
        clippy::needless_return,
        reason = "the project explicit-return policy keeps constructor exits visually uniform"
    )]
    pub const fn new(task: TaskId) -> Self {
        return Self { task };
    }
    /// Returns the exact task bound to retry recovery.
    #[must_use]
    #[expect(
        clippy::needless_return,
        reason = "the project explicit-return policy keeps accessor exits visually uniform"
    )]
    pub const fn task(&self) -> &TaskId {
        return &self.task;
    }
}

#[derive(ModelInput)]
/// Provenance-bearing inputs to the modeled abandonment decision.
struct Intent {
    /// Authoritative board stream receiving the order fact.
    #[model(origin)]
    board: StreamId,
    /// Current semantic lifecycle and board-order intent.
    #[model(origin)]
    plan: Plan,
    /// Addressed task stream receiving the transition fact.
    #[model(origin)]
    task_stream: StreamId,
}
#[derive(Clone, Debug)]
/// Minimal current task and order state needed by the modeled decision.
struct Plan {
    /// Current complete strict board order.
    order: Vec<TaskId>,
    /// Current durable lifecycle status.
    status: TaskStatus,
    /// Task addressed by the abandonment request.
    task: TaskId,
}
#[derive(ModelCommand)]
/// Checked command carrying the semantic abandonment plan.
struct Command {
    /// Authoritative board stream receiving the modeled fact.
    #[stream]
    board: StreamId,
    /// Current semantic lifecycle and board-order intent.
    plan: Plan,
    /// Addressed task stream receiving the transition fact.
    task_stream: StreamId,
}
mapping! { AbandonmentIntentBoard: Intent.board => Command.board using clone; }
mapping! { AbandonmentIntentPlan: Intent.plan => Command.plan using clone; }
mapping! { AbandonmentIntentTaskStream: Intent.task_stream => Command.task_stream using clone; }
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact from which the durable abandonment batch is built.
struct Fact {
    /// Authoritative board stream receiving the modeled fact.
    board: StreamId,
    /// Cleared task claim for the terminal transition.
    claim: Option<TaskClaim>,
    /// Whether this modeled execution emitted its single fact.
    emitted: bool,
    /// Resulting complete strict board order.
    order: Vec<TaskId>,
    /// Resulting terminal task status.
    status: TaskStatus,
    /// Addressed task identity.
    task: TaskId,
    /// Addressed task stream receiving the transition fact.
    task_stream: StreamId,
}
impl Event for Fact {
    #[expect(
        clippy::needless_return,
        reason = "the project explicit-return policy applies to emitted trait method bodies"
    )]
    fn event_type_name() -> &'static str {
        return "TiberModeledTaskAbandoned";
    }
    #[expect(
        clippy::needless_return,
        reason = "the project explicit-return policy applies to emitted trait method bodies"
    )]
    fn stream_id(&self) -> &StreamId {
        return &self.board;
    }
}
#[derive(ModelState)]
/// Reconstructed single-emission state for the modeled command.
struct State {
    /// Whether the modeled abandonment fact was already evolved.
    #[model(default)]
    emitted: bool,
}
#[derive(ModelOutput)]
/// Modeled decision state used to enforce single emission.
struct Decision {
    /// Whether a fact was already emitted.
    emitted: bool,
}
#[derive(ModelOutput)]
/// Fully provenance-consuming view used to build the opaque publication.
struct View {
    /// Authoritative board stream receiving the order fact.
    board: StreamId,
    /// Cleared task claim for the terminal transition.
    claim: Option<TaskClaim>,
    /// Whether this modeled execution emitted its single fact.
    emitted: bool,
    /// Resulting complete strict board order.
    order: Vec<TaskId>,
    /// Resulting terminal task status.
    status: TaskStatus,
    /// Addressed task identity.
    task: TaskId,
    /// Addressed task stream receiving the transition fact.
    task_stream: StreamId,
}
mapping! { AbandonmentStateDecision: State.emitted => Decision.emitted using copy; }
mapping! { AbandonmentFactBoard: Command.board => Fact.board using clone; }
mapping! { AbandonmentFactClaim: Command.plan => Fact.claim using clear_claim; }
mapping! { AbandonmentFactOrder: Command.plan => Fact.order using remaining_order; }
mapping! { AbandonmentFactStatus: Command.plan => Fact.status using abandoned_status; }
mapping! { AbandonmentFactTask: Command.plan => Fact.task using planned_task; }
mapping! { AbandonmentFactTaskStream: Command.task_stream => Fact.task_stream using clone; }
mapping! { AbandonmentFactEmitted: Decision.emitted => Fact.emitted using try emit_once, error = CommandError; }
mapping! { AbandonmentViewBoard: Fact.board => View.board using clone; }
mapping! { AbandonmentViewClaim: Fact.claim => View.claim using clone; }
mapping! { AbandonmentViewEmitted: Fact.emitted => View.emitted using copy; }
mapping! { AbandonmentViewOrder: Fact.order => View.order using clone; }
mapping! { AbandonmentViewStatus: Fact.status => View.status using copy; }
mapping! { AbandonmentViewTask: Fact.task => View.task using clone; }
mapping! { AbandonmentViewTaskStream: Fact.task_stream => View.task_stream using clone; }
#[expect(
    clippy::missing_trait_methods,
    reason = "the modeled abandonment command uses the trait defaults for related-stream discovery"
)]
impl ModelCommandLogic for Command {
    type Event = Fact;
    type State = State;
    #[expect(
        clippy::needless_return,
        clippy::question_mark_used,
        reason = "the checked single-emission mapping preserves its typed model error directly"
    )]
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if self.plan.status == TaskStatus::Abandoned {
            return Ok(ModeledEvents::none(
                "the complete abandonment is already durable",
            ));
        }
        let decision = Decision::model_builder()
            .emitted(AbandonmentStateDecision::apply(state.as_ref()))
            .build();
        return Ok(ModeledEvents::one(
            Fact::model_builder()
                .board(AbandonmentFactBoard::apply(self))
                .claim(AbandonmentFactClaim::apply(self))
                .emitted(AbandonmentFactEmitted::apply(decision.as_ref())?)
                .order(AbandonmentFactOrder::apply(self))
                .status(AbandonmentFactStatus::apply(self))
                .task(AbandonmentFactTask::apply(self))
                .task_stream(AbandonmentFactTaskStream::apply(self))
                .build(),
        ));
    }
    #[expect(
        clippy::needless_return,
        clippy::shadow_reuse,
        reason = "the evolved model state intentionally reuses the semantic state name after consuming its provenance wrapper"
    )]
    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.emitted = true;
        return Modeled::from_built(state);
    }
}

/// Derives the cleared claim required by abandonment.
#[expect(
    clippy::needless_return,
    clippy::single_call_fn,
    reason = "the checked mapping helper must accept the semantic plan field while producing the fixed terminal claim"
)]
fn clear_claim(_plan: &Plan) -> Option<TaskClaim> {
    return None;
}

/// Converts reconstructed emission state into one fresh emission marker.
#[expect(
    clippy::needless_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "the checked fallible mapping receives EventCore's borrowed boolean field and enforces the command's single-emission invariant"
)]
fn emit_once(value: &bool) -> Result<bool, CommandError> {
    if *value {
        return Err("tasks_modeled_abandonment_already_emitted".into());
    }
    return Ok(true);
}

/// Removes the addressed task while preserving all unrelated board order.
#[expect(
    clippy::needless_return,
    clippy::single_call_fn,
    reason = "the checked mapping requires one named strict-order derivation from the semantic plan"
)]
fn remaining_order(plan: &Plan) -> Vec<TaskId> {
    return plan
        .order
        .iter()
        .filter(|task| return *task != &plan.task)
        .cloned()
        .collect();
}

/// Derives the terminal status required by abandonment.
#[expect(
    clippy::needless_return,
    clippy::single_call_fn,
    reason = "the checked mapping helper must accept the semantic plan field while producing the fixed terminal status"
)]
const fn abandoned_status(_plan: &Plan) -> TaskStatus {
    return TaskStatus::Abandoned;
}

/// Copies the addressed task from the semantic plan into the modeled fact.
#[expect(
    clippy::needless_return,
    clippy::single_call_fn,
    reason = "the checked mapping requires one named task-provenance conversion and the project explicit-return policy keeps its exit visible"
)]
fn planned_task(plan: &Plan) -> TaskId {
    return plan.task.clone();
}

/// Rejects duplicate membership in one complete strict-order carrier.
#[expect(
    clippy::indexing_slicing,
    clippy::needless_return,
    reason = "enumerate guarantees that each inspected prefix endpoint is within the same slice, and the project explicit-return policy keeps the typed success exit visible"
)]
fn validate_strict_order(order: &[TaskId]) -> Result<(), TaskCommandError> {
    if order
        .iter()
        .enumerate()
        .any(|(index, task)| return order[..index].contains(task))
    {
        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
    }
    return Ok(());
}

/// Decides one abandonment from canonical facts.
#[expect(
    clippy::map_err_ignore,
    clippy::needless_return,
    clippy::pattern_type_mismatch,
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "the command-local chronological fold keeps every abandonment authority fact, validation, modeled decision, and opaque publication construction visible in one typed flow"
)]
pub fn decide(
    events: &[TaskEvent],
    request: &AbandonTask,
) -> Result<Option<TaskAbandonmentPublication>, TaskCommandError> {
    let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
        .map_err(|_| return TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", request.task.as_str()))
        .map_err(|_| return TaskCommandError::InvalidTaskStream)?;
    let mut exists = false;
    let mut order_observed = false;
    let mut status = None;
    let mut order = Vec::new();
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
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                exists = true;
                status = Some(created.task.status);
            }
            TaskEvent::TaskTransitioned(transitioned) if transitioned.stem == request.task => {
                if transitioned.stream_id != board && transitioned.stream_id != task_stream {
                    return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                        task: request.task.clone(),
                        stream: transitioned.stream_id.clone(),
                    });
                }
                if !exists {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                if status.is_some_and(|current| {
                    return matches!(current, TaskStatus::Done | TaskStatus::Abandoned)
                        && transitioned.status != current;
                }) {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                status = Some(transitioned.status);
            }
            TaskEvent::TaskPriorityChanged(changed) | TaskEvent::BoardReordered(changed) => {
                if changed.stream_id != board {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                validate_strict_order(&changed.order)?;
                order.clone_from(&changed.order);
                order_observed = true;
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                if repaired.stream_id != board {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                if let Some(repaired_order) = &repaired.order_change {
                    if repaired_order.stream_id != board {
                        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                    }
                    validate_strict_order(&repaired_order.order)?;
                    order.clone_from(&repaired_order.order);
                    order_observed = true;
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                if closed.stream_id != board {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                validate_strict_order(&closed.order)?;
                order.clone_from(&closed.order);
                order_observed = true;
                if closed.stems.contains(&request.task) {
                    if !exists {
                        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                    }
                    status = Some(TaskStatus::Done);
                }
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) if closed.stem == request.task => {
                if closed.stream_id != board && closed.stream_id != task_stream || !exists {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                status = Some(TaskStatus::Done);
            }
            TaskEvent::HistoricalTaskRemoved(removed) if removed.stem == request.task => {
                if removed.stream_id != board && removed.stream_id != task_stream || !exists {
                    return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
                }
                exists = false;
                status = None;
            }
            _ => {}
        }
    }
    let Some(current) = status else {
        return Err(TaskCommandError::TaskMissing {
            task: request.task.clone(),
        });
    };
    if !order_observed {
        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
    }
    if current == TaskStatus::Abandoned && order.contains(&request.task) {
        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
    }
    if current != TaskStatus::Abandoned
        && current != TaskStatus::Backlog
        && current != TaskStatus::InProgress
    {
        return Err(TaskCommandError::TaskAbandonmentNotOpen {
            task: request.task.clone(),
            status: current,
        });
    }
    if current != TaskStatus::Abandoned
        && order
            .iter()
            .filter(|task| return *task == &request.task)
            .count()
            != 1
    {
        return Err(TaskCommandError::TaskAbandonmentMalformedHistory);
    }
    let intent = Intent::model_builder()
        .board(board.clone())
        .plan(Plan {
            order,
            status: current,
            task: request.task.clone(),
        })
        .task_stream(task_stream)
        .build();
    let command = Command::model_builder()
        .board(AbandonmentIntentBoard::apply(intent.as_ref()))
        .plan(AbandonmentIntentPlan::apply(intent.as_ref()))
        .task_stream(AbandonmentIntentTaskStream::apply(intent.as_ref()))
        .build();
    let facts: Vec<Fact> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_| return TaskCommandError::ModeledTaskAbandonmentDecisionFailed)?
        .into();
    if facts.is_empty() {
        return Ok(None);
    }
    let [fact] = facts
        .try_into()
        .map_err(|_| return TaskCommandError::InvalidModeledTaskAbandonmentPublication)?;
    let view = View::model_builder()
        .board(AbandonmentViewBoard::apply(&fact))
        .claim(AbandonmentViewClaim::apply(&fact))
        .emitted(AbandonmentViewEmitted::apply(&fact))
        .order(AbandonmentViewOrder::apply(&fact))
        .status(AbandonmentViewStatus::apply(&fact))
        .task(AbandonmentViewTask::apply(&fact))
        .task_stream(AbandonmentViewTaskStream::apply(&fact))
        .build()
        .into_inner();
    if !view.emitted {
        return Err(TaskCommandError::InvalidModeledTaskAbandonmentPublication);
    }
    return TaskAbandonmentPublication::from_modeled_facts(
        view.task.clone(),
        TaskTransitioned::new(view.task_stream, view.task, view.status, view.claim),
        TaskOrder::new(view.board, view.order),
        [
            board,
            StreamId::try_new(format!("tiber:task:{}", request.task.as_str()))
                .map_err(|_| return TaskCommandError::InvalidTaskStream)?,
        ],
    )
    .map(Some);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the focused private model test fails loudly when its fixed semantic identities or modeled emission cannot be constructed"
    )]
    fn modeled_abandonment_derives_terminal_transition_and_target_free_order() {
        let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned()).expect("valid board");
        let target = TaskId::parse("20260816-test-modeled-target").expect("valid target");
        let other = TaskId::parse("20260816-test-modeled-other").expect("valid other");
        let trailing = TaskId::parse("20260816-test-modeled-trailing").expect("valid trailing");
        let intent = Intent::model_builder()
            .board(board)
            .plan(Plan {
                order: vec![other.clone(), target.clone(), trailing.clone()],
                status: TaskStatus::InProgress,
                task: target,
            })
            .task_stream(
                StreamId::try_new("tiber:task:20260816-test-modeled-target".to_owned())
                    .expect("valid task stream"),
            )
            .build();
        let command = Command::model_builder()
            .board(AbandonmentIntentBoard::apply(intent.as_ref()))
            .plan(AbandonmentIntentPlan::apply(intent.as_ref()))
            .task_stream(AbandonmentIntentTaskStream::apply(intent.as_ref()))
            .build();

        let facts: Vec<Fact> = CommandLogic::handle(&command, Modeled::default())
            .expect("modeled abandonment should decide")
            .into();
        let fact = facts.into_iter().next().expect("one modeled fact");

        assert_eq!(fact.status, TaskStatus::Abandoned);
        assert!(fact.claim.is_none());
        assert_eq!(fact.order, vec![other, trailing]);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the focused private model test fails loudly when its fixed semantic identities or modeled no-op cannot be constructed"
    )]
    fn modeled_abandonment_reconciles_complete_abandoned_status_without_an_event() {
        let reconciled_target =
            TaskId::parse("20260816-test-modeled-reconciled").expect("valid reconciled target");
        let reconciled_intent = Intent::model_builder()
            .board(StreamId::try_new(TASK_BOARD_STREAM.to_owned()).expect("valid board"))
            .plan(Plan {
                order: Vec::new(),
                status: TaskStatus::Abandoned,
                task: reconciled_target,
            })
            .task_stream(
                StreamId::try_new("tiber:task:20260816-test-modeled-reconciled".to_owned())
                    .expect("valid task stream"),
            )
            .build();
        let reconciled_command = Command::model_builder()
            .board(AbandonmentIntentBoard::apply(reconciled_intent.as_ref()))
            .plan(AbandonmentIntentPlan::apply(reconciled_intent.as_ref()))
            .task_stream(AbandonmentIntentTaskStream::apply(
                reconciled_intent.as_ref(),
            ))
            .build();
        let reconciled: Vec<Fact> = CommandLogic::handle(&reconciled_command, Modeled::default())
            .expect("modeled exact retry should reconcile")
            .into();
        assert!(reconciled.is_empty());
    }
}
