//! Modeled decision for appending one unchecked acceptance criterion.

use super::TaskCommandError;
use crate::AcceptanceAddPublication;
use alloc::{string::String, vec::Vec};
use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskAcceptanceAdded, TaskEvent, TaskId};

/// One exact acceptance criterion to append to an existing task.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the request fields follow addressed task then exact criterion command flow"
)]
pub struct AddAcceptance {
    /// Addressed durable task identity.
    task: TaskId,
    /// Exact criterion to append.
    criterion: String,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the semantic request API presents construction before the retry-bound criterion accessor"
)]
impl AddAcceptance {
    /// Creates one semantic acceptance-add request.
    #[must_use]
    pub fn new(task: TaskId, criterion: String) -> Self {
        Self { task, criterion }
    }

    /// Returns the exact criterion bound to an ambiguous-publication retry.
    #[must_use]
    pub fn criterion(&self) -> &str {
        &self.criterion
    }
}

/// Minimal current task state needed to decide one acceptance addition.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the fold state follows existence before the current criterion collection used by reconciliation"
)]
struct AcceptanceState {
    /// Whether the addressed task currently exists.
    exists: bool,
    /// Current criterion text after ordered additions and removals.
    criteria: Vec<String>,
}

impl AcceptanceState {
    /// Folds only task existence and current criterion text for the addressed task.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        clippy::single_call_fn,
        clippy::wildcard_enum_match_arm,
        reason = "the command-local fold uses borrowed closed event patterns, typed propagation, one text projection, and deliberately ignores facts outside its minimal acceptance-add authority"
    )]
    fn fold(events: &[TaskEvent], task: &TaskId) -> Result<Self, TaskCommandError> {
        let streams = consistency_streams(task)?;
        let mut state = Self {
            exists: false,
            criteria: Vec::new(),
        };
        for event in events {
            match event {
                TaskEvent::TaskCreated(fact) if &fact.task.stem == task => {
                    require_stream(task, &fact.stream_id, &streams)?;
                    if state.exists {
                        return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() });
                    }
                    state.exists = true;
                    state.criteria = fact
                        .task
                        .acceptance
                        .iter()
                        .map(|item| item.text.clone())
                        .collect();
                }
                TaskEvent::TaskAcceptanceAdded(fact) if &fact.stem == task => {
                    require_stream(task, &fact.stream_id, &streams)?;
                    if !state.exists {
                        return Err(TaskCommandError::TaskMissing { task: task.clone() });
                    }
                    state.criteria.push(fact.item.text.clone());
                }
                TaskEvent::TaskAcceptanceRemoved(fact) if &fact.stem == task => {
                    require_stream(task, &fact.stream_id, &streams)?;
                    if !state.exists {
                        return Err(TaskCommandError::TaskMissing { task: task.clone() });
                    }
                    if fact.index >= state.criteria.len() {
                        return Err(TaskCommandError::HistoryAcceptanceItemMissing {
                            task: task.clone(),
                            index: fact.index,
                        });
                    }
                    let _: String = state.criteria.remove(fact.index);
                }
                TaskEvent::HistoricalTaskRemoved(fact) if &fact.stem == task => {
                    require_stream(task, &fact.stream_id, &streams)?;
                    state.exists = false;
                    state.criteria.clear();
                }
                _ => {}
            }
        }
        if !state.exists {
            return Err(TaskCommandError::TaskMissing { task: task.clone() });
        }
        Ok(state)
    }
}

#[derive(ModelInput)]
/// Checked origin values supplied to the modeled acceptance-add command.
struct AddIntent {
    /// Exact criterion to append.
    #[model(origin)]
    criterion: String,
    /// Addressed task stream.
    #[model(origin)]
    stream: StreamId,
    /// Addressed task identity.
    #[model(origin)]
    task: TaskId,
}

#[derive(ModelCommand)]
/// Checked `EventCore` command for one acceptance addition.
struct ModeledAddAcceptance {
    /// Exact criterion to append.
    criterion: String,
    /// Addressed task stream.
    #[stream]
    stream: StreamId,
    /// Addressed task identity.
    task: TaskId,
}

mapping! { AcceptanceAddIntentCriterion: AddIntent.criterion => ModeledAddAcceptance.criterion using clone; }
mapping! { AcceptanceAddIntentStream: AddIntent.stream => ModeledAddAcceptance.stream using clone; }
mapping! { AcceptanceAddIntentTask: AddIntent.task => ModeledAddAcceptance.task using clone; }

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact carrying the acceptance-add intent and emission marker.
struct ModeledAcceptanceAdded {
    /// Exact criterion emitted by the command.
    criterion: String,
    /// Exactly-once emission marker.
    emitted: bool,
    /// Addressed task stream.
    stream: StreamId,
    /// Addressed task identity.
    task: TaskId,
}

#[expect(
    clippy::implicit_return,
    reason = "the stable modeled event name and addressed stream are each their complete result"
)]
impl Event for ModeledAcceptanceAdded {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskAcceptanceAdded"
    }
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[derive(ModelOutput)]
/// Provenance-consuming view used to construct the opaque publication.
struct AddedView {
    /// Exact modeled criterion.
    criterion: String,
    /// Exactly-once emission marker.
    emitted: bool,
    /// Addressed task stream.
    stream: StreamId,
    /// Addressed task identity.
    task: TaskId,
}

mapping! { AcceptanceAddViewCriterion: ModeledAcceptanceAdded.criterion => AddedView.criterion using clone; }
mapping! { AcceptanceAddViewEmitted: ModeledAcceptanceAdded.emitted => AddedView.emitted using copy; }
mapping! { AcceptanceAddViewStream: ModeledAcceptanceAdded.stream => AddedView.stream using clone; }
mapping! { AcceptanceAddViewTask: ModeledAcceptanceAdded.task => AddedView.task using clone; }

#[derive(ModelState)]
/// Minimal modeled state enforcing one emission per command execution.
struct AddModelState {
    /// Whether the fact has already been emitted.
    #[model(default)]
    emitted: bool,
}
#[derive(ModelOutput)]
/// Decision view consumed by the emission guard.
struct AddDecision {
    /// Whether the fact has already been emitted.
    emitted: bool,
}
mapping! { AcceptanceAddStateDecision: AddModelState.emitted => AddDecision.emitted using copy; }
mapping! { AcceptanceAddFactCriterion: ModeledAddAcceptance.criterion => ModeledAcceptanceAdded.criterion using clone; }
mapping! { AcceptanceAddFactStream: ModeledAddAcceptance.stream => ModeledAcceptanceAdded.stream using clone; }
mapping! { AcceptanceAddFactTask: ModeledAddAcceptance.task => ModeledAcceptanceAdded.task using clone; }
mapping! { AcceptanceAddFactEmitted: AddDecision.emitted => ModeledAcceptanceAdded.emitted using try emit_once, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "EventCore supplies static stream discovery for the checked modeled command"
)]
impl ModelCommandLogic for ModeledAddAcceptance {
    type Event = ModeledAcceptanceAdded;
    type State = AddModelState;

    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the modeled decision directly constructs its one fact while preserving the checked emission-guard failure"
    )]
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = AddDecision::model_builder()
            .emitted(AcceptanceAddStateDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledAcceptanceAdded::model_builder()
                .criterion(AcceptanceAddFactCriterion::apply(self))
                .emitted(AcceptanceAddFactEmitted::apply(decision.as_ref())?)
                .stream(AcceptanceAddFactStream::apply(self))
                .task(AcceptanceAddFactTask::apply(self))
                .build(),
        ))
    }

    #[expect(
        clippy::implicit_return,
        clippy::shadow_reuse,
        reason = "the evolved modeled state intentionally reuses the consumed state binding before returning its rebuilt value"
    )]
    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.emitted = true;
        Modeled::from_built(state)
    }
}

/// Permits exactly the first modeled acceptance-add emission.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore's checked mapping requires this named borrowed scalar conversion for the one-use emission guard"
)]
fn emit_once(emitted: &bool) -> Result<bool, CommandError> {
    if *emitted {
        return Err("tasks_modeled_acceptance_add_already_emitted".into());
    }
    Ok(true)
}

/// Decides one acceptance addition, reconciling an exact durable retry.
///
/// # Errors
///
/// Returns a typed command failure when canonical history cannot authorize the
/// addition or the checked model cannot produce its closed publication.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    reason = "the command boundary sequences its narrow fold, exact-current-state reconciliation, and checked model with typed propagation, then reuses the conventional events binding for modeled output"
)]
pub fn decide_add_acceptance(
    events: &[TaskEvent],
    request: &AddAcceptance,
) -> Result<Option<AcceptanceAddPublication>, TaskCommandError> {
    let state = AcceptanceState::fold(events, &request.task)?;
    if state
        .criteria
        .iter()
        .any(|criterion| criterion == &request.criterion)
    {
        return Ok(None);
    }
    let streams = consistency_streams(&request.task)?;
    let intent = AddIntent::model_builder()
        .criterion(request.criterion.clone())
        .stream(streams[1].clone())
        .task(request.task.clone())
        .build();
    let command = ModeledAddAcceptance::model_builder()
        .criterion(AcceptanceAddIntentCriterion::apply(intent.as_ref()))
        .stream(AcceptanceAddIntentStream::apply(intent.as_ref()))
        .task(AcceptanceAddIntentTask::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledAcceptanceAdded> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledAcceptanceAddDecisionFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| TaskCommandError::InvalidModeledAcceptanceAddPublication)?;
    let view = AddedView::model_builder()
        .criterion(AcceptanceAddViewCriterion::apply(&event))
        .emitted(AcceptanceAddViewEmitted::apply(&event))
        .stream(AcceptanceAddViewStream::apply(&event))
        .task(AcceptanceAddViewTask::apply(&event))
        .build()
        .into_inner();
    if !view.emitted {
        return Err(TaskCommandError::InvalidModeledAcceptanceAddPublication);
    }
    AcceptanceAddPublication::from_modeled_fact(
        TaskAcceptanceAdded::new(view.stream, view.task, view.criterion),
        streams,
    )
    .map(Some)
}

/// Derives the board and addressed-task streams fencing this decision.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the exact two-stream fence is constructed once with typed stream validation"
)]
fn consistency_streams(task: &TaskId) -> Result<[StreamId; 2], TaskCommandError> {
    let board = StreamId::try_new("tiber:board".to_owned())
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    Ok([board, task_stream])
}

/// Rejects an addressed fact outside the command's exact consistency fence.
#[expect(
    clippy::implicit_return,
    reason = "the compact authority predicate returns its typed success or failure directly"
)]
fn require_stream(
    task: &TaskId,
    stream: &StreamId,
    expected: &[StreamId; 2],
) -> Result<(), TaskCommandError> {
    if expected.contains(stream) {
        Ok(())
    } else {
        Err(TaskCommandError::TargetTaskFactUnexpectedStream {
            task: task.clone(),
            stream: stream.clone(),
        })
    }
}
