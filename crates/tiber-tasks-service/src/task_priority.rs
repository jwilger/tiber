//! Modeled decision for moving one task immediately before another.

use super::TaskCommandError;
use crate::TaskPriorityPublication;
use alloc::{collections::BTreeSet, vec::Vec};
use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskEvent, TaskId, TaskOrder, TaskStatus};

/// One request to move `task` immediately before `before`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrioritizeTask {
    /// Task that must immediately follow the moved task.
    before: TaskId,
    /// Task being moved.
    task: TaskId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the semantic request API presents construction, moved task, then anchor in command-flow order"
)]
impl PrioritizeTask {
    /// Creates one semantic strict-priority request.
    #[must_use]
    pub const fn new(task: TaskId, before: TaskId) -> Self {
        Self { before, task }
    }

    /// Returns the task being moved.
    #[must_use]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the task that must immediately follow the moved task.
    #[must_use]
    pub const fn before(&self) -> &TaskId {
        &self.before
    }
}

/// Minimal lifecycle and strict-order state needed by one priority decision.
struct PriorityState {
    /// Current lifecycle of the anchor, when it exists.
    before_status: Option<TaskStatus>,
    /// Latest complete strict board order.
    order: Vec<TaskId>,
    /// Current lifecycle of the moved task, when it exists.
    task_status: Option<TaskStatus>,
}

#[derive(ModelInput)]
/// Checked origin values supplied to the modeled priority command.
struct PriorityIntent {
    /// Anchor that must immediately follow the moved task.
    #[model(origin)]
    before: TaskId,
    /// Complete current strict board order.
    #[model(origin)]
    current_order: Vec<TaskId>,
    /// Canonical board stream receiving the fact.
    #[model(origin)]
    stream: StreamId,
    /// Task being moved.
    #[model(origin)]
    task: TaskId,
}

#[derive(ModelCommand)]
/// Checked `EventCore` command deriving one strict board movement.
struct ModeledPrioritizeTask {
    /// Anchor that must immediately follow the moved task.
    before: TaskId,
    /// Complete current strict board order.
    current_order: Vec<TaskId>,
    /// Canonical board stream receiving the fact.
    #[stream]
    stream: StreamId,
    /// Task being moved.
    task: TaskId,
}

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact carrying the complete derived priority order.
struct ModeledTaskPrioritized {
    /// Exactly-once emission marker.
    emitted: bool,
    /// Complete resulting strict board order.
    order: Vec<TaskId>,
    /// Canonical board stream receiving the fact.
    stream: StreamId,
}

#[derive(ModelState)]
/// Minimal modeled state enforcing at most one emission per execution.
struct PriorityModelState {
    /// Whether this modeled command has already emitted its fact.
    #[model(default)]
    emitted: bool,
}

#[derive(ModelOutput)]
/// Provenance-bearing emission decision.
struct PriorityDecision {
    /// Whether this modeled command has already emitted its fact.
    emitted: bool,
}

#[derive(ModelOutput)]
/// Provenance-consuming view used to construct the opaque publication.
struct PriorityView {
    /// Exactly-once emission marker.
    emitted: bool,
    /// Complete modeled strict board order.
    order: Vec<TaskId>,
    /// Canonical board stream receiving the fact.
    stream: StreamId,
}

#[expect(
    clippy::implicit_return,
    reason = "the modeled event name and addressed stream are each their complete result"
)]
impl Event for ModeledTaskPrioritized {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskPrioritized"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "EventCore supplies static stream discovery for the checked modeled command"
)]
impl ModelCommandLogic for ModeledPrioritizeTask {
    type Event = ModeledTaskPrioritized;
    type State = PriorityModelState;

    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the modeled decision directly returns its no-op or one provenance-bearing fact while preserving the emission guard"
    )]
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let order = derive_priority_order(&self.current_order, &self.task, &self.before);
        if order == self.current_order {
            return Ok(ModeledEvents::none("task already has requested priority"));
        }
        let decision = PriorityDecision::model_builder()
            .emitted(PriorityStateDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledTaskPrioritized::model_builder()
                .emitted(PriorityFactEmitted::apply(decision.as_ref())?)
                .order(PriorityFactOrder::apply((self, self, self)))
                .stream(PriorityFactStream::apply(self))
                .build(),
        ))
    }

    #[expect(
        clippy::implicit_return,
        clippy::shadow_reuse,
        reason = "the evolved modeled state intentionally reuses the consumed binding before returning its rebuilt value"
    )]
    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.emitted = true;
        Modeled::from_built(state)
    }
}

/// Folds only addressed lifecycle and complete strict-order facts.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "the command-local chronological fold keeps its bounded lifecycle and order authority explicit while preserving typed propagation and borrowed facts"
)]
fn fold_state(
    events: &[TaskEvent],
    request: &PrioritizeTask,
) -> Result<PriorityState, TaskCommandError> {
    let streams = consistency_streams(&request.task, &request.before)?;
    let mut state = PriorityState {
        before_status: None,
        order: Vec::new(),
        task_status: None,
    };
    for event in events {
        let replacement = match event {
            TaskEvent::TaskPriorityChanged(fact) | TaskEvent::BoardReordered(fact) => {
                Some((&fact.stream_id, &fact.order))
            }
            TaskEvent::TasksClosedFromCommitTrailers(fact) => Some((&fact.stream_id, &fact.order)),
            TaskEvent::TaskValidationRepaired(fact) => {
                if let Some(order) = fact.order_change.as_ref() {
                    if fact.stream_id.as_ref() != "tiber:board" || order.stream_id != fact.stream_id
                    {
                        return Err(TaskCommandError::TaskPriorityMalformedHistory);
                    }
                    Some((&order.stream_id, &order.order))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((stream, replacement)) = replacement {
            if stream.as_ref() != "tiber:board"
                || replacement.iter().collect::<BTreeSet<_>>().len() != replacement.len()
            {
                return Err(TaskCommandError::TaskPriorityMalformedHistory);
            }
            state.order.clone_from(replacement);
        }
        match event {
            TaskEvent::TaskCreated(fact)
                if fact.task.stem == request.task || fact.task.stem == request.before =>
            {
                require_endpoint_stream(&fact.task.stem, &fact.stream_id, &streams)?;
                let status = endpoint_status_mut(&mut state, &fact.task.stem, request);
                if status.is_some() {
                    return Err(TaskCommandError::DuplicateTaskCreation {
                        task: fact.task.stem.clone(),
                    });
                }
                *status = Some(fact.task.status);
            }
            TaskEvent::TaskTransitioned(fact)
                if fact.stem == request.task || fact.stem == request.before =>
            {
                require_endpoint_stream(&fact.stem, &fact.stream_id, &streams)?;
                let status = endpoint_status_mut(&mut state, &fact.stem, request);
                if status.is_none() {
                    return Err(TaskCommandError::TaskMissing {
                        task: fact.stem.clone(),
                    });
                }
                *status = Some(fact.status);
            }
            TaskEvent::TasksClosedFromCommitTrailers(fact) => {
                if fact.stream_id.as_ref() != "tiber:board" {
                    return Err(TaskCommandError::TaskPriorityMalformedHistory);
                }
                for task in [&request.task, &request.before] {
                    if fact.stems.contains(task) {
                        let status = endpoint_status_mut(&mut state, task, request);
                        if status.is_none() {
                            return Err(TaskCommandError::TaskMissing { task: task.clone() });
                        }
                        *status = Some(TaskStatus::Done);
                    }
                }
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(fact)
                if fact.stem == request.task || fact.stem == request.before =>
            {
                require_endpoint_stream(&fact.stem, &fact.stream_id, &streams)?;
                let status = endpoint_status_mut(&mut state, &fact.stem, request);
                if status.is_none() {
                    return Err(TaskCommandError::TaskMissing {
                        task: fact.stem.clone(),
                    });
                }
                *status = Some(TaskStatus::Done);
            }
            TaskEvent::HistoricalTaskRemoved(fact)
                if fact.stem == request.task || fact.stem == request.before =>
            {
                require_endpoint_stream(&fact.stem, &fact.stream_id, &streams)?;
                let status = endpoint_status_mut(&mut state, &fact.stem, request);
                if status.take().is_none() {
                    return Err(TaskCommandError::TaskMissing {
                        task: fact.stem.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(state)
}

/// Selects the retained lifecycle slot for one addressed endpoint.
#[expect(
    clippy::implicit_return,
    reason = "the endpoint-role selection is clearest as one final conditional expression"
)]
fn endpoint_status_mut<'state>(
    state: &'state mut PriorityState,
    task: &TaskId,
    request: &PrioritizeTask,
) -> &'state mut Option<TaskStatus> {
    if task == &request.task {
        &mut state.task_status
    } else {
        &mut state.before_status
    }
}

/// Requires an addressed lifecycle fact to come from the board or its own task stream.
#[expect(
    clippy::implicit_return,
    reason = "the compact stream-ownership predicate returns its typed result directly"
)]
fn require_endpoint_stream(
    task: &TaskId,
    stream: &StreamId,
    expected: &[StreamId; 3],
) -> Result<(), TaskCommandError> {
    let expected_stream = if task.as_str() == expected[1].as_ref().trim_start_matches("tiber:task:")
    {
        &expected[1]
    } else {
        &expected[2]
    };
    if stream == &expected[0] || stream == expected_stream {
        Ok(())
    } else {
        Err(TaskCommandError::TargetTaskFactUnexpectedStream {
            task: task.clone(),
            stream: stream.clone(),
        })
    }
}

mapping! { PriorityIntentBefore: PriorityIntent.before => ModeledPrioritizeTask.before using clone; }
mapping! { PriorityIntentCurrentOrder: PriorityIntent.current_order => ModeledPrioritizeTask.current_order using clone; }
mapping! { PriorityIntentStream: PriorityIntent.stream => ModeledPrioritizeTask.stream using clone; }
mapping! { PriorityIntentTask: PriorityIntent.task => ModeledPrioritizeTask.task using clone; }

mapping! { PriorityStateDecision: PriorityModelState.emitted => PriorityDecision.emitted using copy; }
mapping! { PriorityFactEmitted: PriorityDecision.emitted => ModeledTaskPrioritized.emitted using try emit_once, error = CommandError; }
mapping! {
    PriorityFactOrder:
        (ModeledPrioritizeTask.current_order, ModeledPrioritizeTask.task, ModeledPrioritizeTask.before)
        => ModeledTaskPrioritized.order
        using derive_priority_order;
}
mapping! { PriorityFactStream: ModeledPrioritizeTask.stream => ModeledTaskPrioritized.stream using clone; }
mapping! { PriorityViewEmitted: ModeledTaskPrioritized.emitted => PriorityView.emitted using copy; }
mapping! { PriorityViewOrder: ModeledTaskPrioritized.order => PriorityView.order using clone; }
mapping! { PriorityViewStream: ModeledTaskPrioritized.stream => PriorityView.stream using clone; }

/// Permits exactly the first modeled priority emission.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore's checked mapping requires a borrowed scalar conversion and direct typed result"
)]
fn emit_once(emitted: &bool) -> Result<bool, CommandError> {
    if *emitted {
        return Err("tasks_modeled_priority_already_emitted".into());
    }
    Ok(true)
}

/// Derives the complete strict order for the modeled move-before intent.
#[expect(
    clippy::implicit_return,
    reason = "the pure order transformation reads clearly as its final vector"
)]
fn derive_priority_order(current_order: &[TaskId], task: &TaskId, before: &TaskId) -> Vec<TaskId> {
    let mut order = current_order.to_vec();
    order.retain(|entry| entry != task);
    let Some(before_index) = order.iter().position(|entry| entry == before) else {
        return current_order.to_vec();
    };
    order.insert(before_index, task.clone());
    order
}

/// Decides one strict priority movement from canonical task history.
///
/// # Errors
///
/// Returns a typed failure when the durable order is malformed or the checked
/// model cannot produce one closed publication.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    reason = "the command boundary sequences narrow history validation, checked-model invocation, and opaque publication construction with typed propagation"
)]
pub fn decide_prioritize_task(
    events: &[TaskEvent],
    request: &PrioritizeTask,
) -> Result<Option<TaskPriorityPublication>, TaskCommandError> {
    if request.task == request.before {
        return Err(TaskCommandError::TaskPrioritySelfReference {
            task: request.task.clone(),
        });
    }
    let state = fold_state(events, request)?;
    for (task, status) in [
        (&request.task, state.task_status),
        (&request.before, state.before_status),
    ] {
        let Some(current_status) = status else {
            return Err(TaskCommandError::TaskMissing { task: task.clone() });
        };
        if !matches!(current_status, TaskStatus::Backlog | TaskStatus::InProgress) {
            return Err(TaskCommandError::TaskPriorityEndpointNotOpen {
                task: task.clone(),
                status: current_status,
            });
        }
    }
    if state
        .order
        .iter()
        .filter(|task| *task == &request.task)
        .count()
        != 1
        || state
            .order
            .iter()
            .filter(|task| *task == &request.before)
            .count()
            != 1
    {
        return Err(TaskCommandError::TaskPriorityMalformedHistory);
    }
    let streams = consistency_streams(&request.task, &request.before)?;
    let intent = PriorityIntent::model_builder()
        .before(request.before.clone())
        .current_order(state.order)
        .stream(streams[0].clone())
        .task(request.task.clone())
        .build();
    let command = ModeledPrioritizeTask::model_builder()
        .before(PriorityIntentBefore::apply(intent.as_ref()))
        .current_order(PriorityIntentCurrentOrder::apply(intent.as_ref()))
        .stream(PriorityIntentStream::apply(intent.as_ref()))
        .task(PriorityIntentTask::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledTaskPrioritized> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledTaskPriorityDecisionFailed)?
        .into();
    let event = match events.as_slice() {
        [] => return Ok(None),
        [event] => event,
        _ => return Err(TaskCommandError::InvalidModeledTaskPriorityPublication),
    };
    let view = PriorityView::model_builder()
        .emitted(PriorityViewEmitted::apply(event))
        .order(PriorityViewOrder::apply(event))
        .stream(PriorityViewStream::apply(event))
        .build()
        .into_inner();
    if !view.emitted {
        return Err(TaskCommandError::InvalidModeledTaskPriorityPublication);
    }
    TaskPriorityPublication::from_modeled_fact(TaskOrder::new(view.stream, view.order), streams)
        .map(Some)
}

/// Derives the canonical board and two distinct addressed-task consistency fences.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the exact three-stream fence is constructed once with typed stream validation"
)]
fn consistency_streams(task: &TaskId, before: &TaskId) -> Result<[StreamId; 3], TaskCommandError> {
    let board = StreamId::try_new("tiber:board".to_owned())
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let before_stream = StreamId::try_new(format!("tiber:task:{}", before.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    Ok([board, task_stream, before_stream])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the private model fixture fails loudly on invalid fixed IDs and returns the semantic ID directly"
    )]
    fn id(value: &str) -> TaskId {
        TaskId::parse(value).expect("test task ID should be valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the private model fixture fails loudly at each checked construction boundary and returns the modeled order directly"
    )]
    fn modeled_order(current_order: Vec<TaskId>, task: TaskId, before: TaskId) -> Vec<TaskId> {
        let stream =
            StreamId::try_new("tiber:board".to_owned()).expect("test board stream should be valid");
        let intent = PriorityIntent::model_builder()
            .before(before)
            .current_order(current_order)
            .stream(stream)
            .task(task)
            .build();
        let command = ModeledPrioritizeTask::model_builder()
            .before(PriorityIntentBefore::apply(intent.as_ref()))
            .current_order(PriorityIntentCurrentOrder::apply(intent.as_ref()))
            .stream(PriorityIntentStream::apply(intent.as_ref()))
            .task(PriorityIntentTask::apply(intent.as_ref()))
            .build();
        let events: Vec<ModeledTaskPrioritized> =
            CommandLogic::handle(&command, Modeled::default())
                .expect("modeled priority should decide")
                .into();
        let [event] = events.try_into().expect("one modeled priority fact");
        event.order
    }

    #[test]
    fn modeled_command_derives_output_from_task_anchor_and_current_order() {
        let first = id("20260816-model-first");
        let second = id("20260816-model-second");
        let third = id("20260816-model-third");

        assert_eq!(
            modeled_order(
                vec![first.clone(), second.clone(), third.clone()],
                third.clone(),
                first.clone(),
            ),
            vec![third.clone(), first.clone(), second.clone()]
        );
        assert_eq!(
            modeled_order(
                vec![first.clone(), second.clone(), third.clone()],
                second.clone(),
                first.clone(),
            ),
            vec![second.clone(), first.clone(), third.clone()]
        );
        assert_eq!(
            modeled_order(
                vec![first.clone(), second.clone(), third.clone()],
                third.clone(),
                second.clone(),
            ),
            vec![first.clone(), third.clone(), second.clone()]
        );
        assert_eq!(
            modeled_order(
                vec![second.clone(), first.clone(), third.clone()],
                third,
                first
            ),
            vec![
                second,
                id("20260816-model-third"),
                id("20260816-model-first")
            ]
        );
    }
}
