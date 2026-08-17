//! Modeled decision for one reciprocal blocked-by dependency.

use super::TaskCommandError;
use crate::DependencyLinkPublication;
use alloc::vec::Vec;
use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskEvent, TaskId, TaskLinksChanged};

/// One request to make `task` blocked by `blocker`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the request fields follow blocked task then blocker in the public command grammar"
)]
pub struct LinkBlockedBy {
    /// Task that becomes blocked.
    task: TaskId,
    /// Task that blocks the target.
    blocker: TaskId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    reason = "the semantic request API follows construction then target and blocker retry accessors"
)]
impl LinkBlockedBy {
    /// Creates one semantic reciprocal dependency request.
    #[must_use]
    pub fn new(task: TaskId, blocker: TaskId) -> Self {
        Self { task, blocker }
    }

    /// Returns the blocked task identity retained for an exact retry.
    #[must_use]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the blocking task identity retained for an exact retry.
    #[must_use]
    pub const fn blocker(&self) -> &TaskId {
        &self.blocker
    }
}

#[derive(Default)]
/// Minimal current dependency fields for one endpoint.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the fold state keeps existence before the two complete dependency collections it guards"
)]
struct EndpointState {
    /// Whether the endpoint currently exists.
    exists: bool,
    /// Tasks blocked by this endpoint.
    blocks: Vec<TaskId>,
    /// Tasks that block this endpoint.
    blocked_by: Vec<TaskId>,
}

/// Minimal current state for both dependency endpoints.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the state follows blocked task then blocker to mirror the command grammar"
)]
struct LinkState {
    /// Task that becomes blocked.
    task: EndpointState,
    /// Task that blocks the target.
    blocker: EndpointState,
}

impl LinkState {
    /// Folds only endpoint existence and complete current dependency fields.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        clippy::single_call_fn,
        clippy::wildcard_enum_match_arm,
        reason = "the command-local fold uses borrowed closed event patterns, typed propagation, and a mutually exclusive relevant-endpoint repair branch while ignoring unrelated facts"
    )]
    fn fold(events: &[TaskEvent], request: &LinkBlockedBy) -> Result<Self, TaskCommandError> {
        let streams = consistency_streams(&request.task, &request.blocker)?;
        let mut state = Self {
            task: EndpointState::default(),
            blocker: EndpointState::default(),
        };
        for event in events {
            match event {
                TaskEvent::TaskCreated(fact) if fact.task.stem == request.task => {
                    require_stream(&request.task, &fact.stream_id, &streams)?;
                    state.task = EndpointState {
                        exists: true,
                        blocks: fact.task.blocks.clone(),
                        blocked_by: fact.task.blocked_by.clone(),
                    };
                }
                TaskEvent::TaskCreated(fact) if fact.task.stem == request.blocker => {
                    require_stream(&request.blocker, &fact.stream_id, &streams)?;
                    state.blocker = EndpointState {
                        exists: true,
                        blocks: fact.task.blocks.clone(),
                        blocked_by: fact.task.blocked_by.clone(),
                    };
                }
                TaskEvent::TaskLinksChanged(fact) if fact.stem == request.task => {
                    require_stream(&request.task, &fact.stream_id, &streams)?;
                    state.task.blocks.clone_from(&fact.blocks);
                    state.task.blocked_by.clone_from(&fact.blocked_by);
                }
                TaskEvent::TaskLinksChanged(fact) if fact.stem == request.blocker => {
                    require_stream(&request.blocker, &fact.stream_id, &streams)?;
                    state.blocker.blocks.clone_from(&fact.blocks);
                    state.blocker.blocked_by.clone_from(&fact.blocked_by);
                }
                TaskEvent::TaskValidationRepaired(repaired) => {
                    if repaired.stream_id != streams[0] {
                        return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
                            task: request.task.clone(),
                            stream: repaired.stream_id.clone(),
                        });
                    }
                    for fact in &repaired.link_changes {
                        if fact.stem == request.task {
                            require_stream(&request.task, &fact.stream_id, &streams)?;
                            state.task.blocks.clone_from(&fact.blocks);
                            state.task.blocked_by.clone_from(&fact.blocked_by);
                        } else {
                            if fact.stem != request.blocker {
                                continue;
                            }
                            require_stream(&request.blocker, &fact.stream_id, &streams)?;
                            state.blocker.blocks.clone_from(&fact.blocks);
                            state.blocker.blocked_by.clone_from(&fact.blocked_by);
                        }
                    }
                }
                TaskEvent::HistoricalTaskRemoved(fact) if fact.stem == request.task => {
                    require_stream(&request.task, &fact.stream_id, &streams)?;
                    state.task = EndpointState::default();
                }
                TaskEvent::HistoricalTaskRemoved(fact) if fact.stem == request.blocker => {
                    require_stream(&request.blocker, &fact.stream_id, &streams)?;
                    state.blocker = EndpointState::default();
                }
                _ => {}
            }
        }
        if !state.task.exists {
            return Err(TaskCommandError::TaskMissing {
                task: request.task.clone(),
            });
        }
        if !state.blocker.exists {
            return Err(TaskCommandError::TaskMissing {
                task: request.blocker.clone(),
            });
        }
        Ok(state)
    }
}

#[derive(ModelInput)]
/// Checked origin values supplied to the modeled dependency command.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the modeled intent follows board, blocked target, then blocker command flow"
)]
struct LinkIntent {
    /// Board stream receiving reciprocal dependency facts.
    #[model(origin)]
    board: StreamId,
    /// Task that becomes blocked.
    #[model(origin)]
    task: TaskId,
    /// Complete current tasks blocked by the target.
    #[model(origin)]
    task_blocks: Vec<TaskId>,
    /// Complete current blockers of the target.
    #[model(origin)]
    task_blocked_by: Vec<TaskId>,
    /// Task that blocks the target.
    #[model(origin)]
    blocker: TaskId,
    /// Complete current tasks blocked by the blocker.
    #[model(origin)]
    blocker_blocks: Vec<TaskId>,
    /// Complete current blockers of the blocker.
    #[model(origin)]
    blocker_blocked_by: Vec<TaskId>,
}

#[derive(ModelCommand)]
/// Checked `EventCore` command for one reciprocal dependency.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the modeled command follows board, blocked target, then blocker command flow"
)]
struct ModeledLinkBlockedBy {
    /// Board stream receiving reciprocal dependency facts.
    #[stream]
    board: StreamId,
    /// Task that becomes blocked.
    task: TaskId,
    /// Complete tasks blocked by the target.
    task_blocks: Vec<TaskId>,
    /// Complete blockers of the target including the requested blocker.
    task_blocked_by: Vec<TaskId>,
    /// Task that blocks the target.
    blocker: TaskId,
    /// Complete tasks blocked by the blocker including the target.
    blocker_blocks: Vec<TaskId>,
    /// Complete blockers of the blocker.
    blocker_blocked_by: Vec<TaskId>,
}

mapping! { DependencyLinkIntentBoard: LinkIntent.board => ModeledLinkBlockedBy.board using clone; }
mapping! { DependencyLinkIntentTask: LinkIntent.task => ModeledLinkBlockedBy.task using clone; }
mapping! { DependencyLinkIntentTaskBlocks: LinkIntent.task_blocks => ModeledLinkBlockedBy.task_blocks using clone; }
mapping! { DependencyLinkIntentTaskBlockedBy: LinkIntent.task_blocked_by => ModeledLinkBlockedBy.task_blocked_by using clone; }
mapping! { DependencyLinkIntentBlocker: LinkIntent.blocker => ModeledLinkBlockedBy.blocker using clone; }
mapping! { DependencyLinkIntentBlockerBlocks: LinkIntent.blocker_blocks => ModeledLinkBlockedBy.blocker_blocks using clone; }
mapping! { DependencyLinkIntentBlockerBlockedBy: LinkIntent.blocker_blocked_by => ModeledLinkBlockedBy.blocker_blocked_by using clone; }

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact carrying both complete reciprocal replacements.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the modeled fact follows board, blocked target, blocker, then emission provenance"
)]
struct ModeledDependencyLinked {
    /// Board stream receiving reciprocal dependency facts.
    board: StreamId,
    /// Task that becomes blocked.
    task: TaskId,
    /// Complete tasks blocked by the target.
    task_blocks: Vec<TaskId>,
    /// Complete blockers of the target.
    task_blocked_by: Vec<TaskId>,
    /// Task that blocks the target.
    blocker: TaskId,
    /// Complete tasks blocked by the blocker.
    blocker_blocks: Vec<TaskId>,
    /// Complete blockers of the blocker.
    blocker_blocked_by: Vec<TaskId>,
    /// Exactly-once modeled emission marker.
    emitted: bool,
}

#[expect(
    clippy::implicit_return,
    reason = "the stable modeled event name and addressed board stream are each their complete result"
)]
impl Event for ModeledDependencyLinked {
    fn event_type_name() -> &'static str {
        "TiberModeledDependencyLinked"
    }

    fn stream_id(&self) -> &StreamId {
        &self.board
    }
}

#[derive(ModelState)]
/// Minimal modeled state enforcing one emission per execution.
struct LinkModelState {
    /// Whether the modeled fact has already been emitted.
    #[model(default)]
    emitted: bool,
}

#[derive(ModelOutput)]
/// Decision view consumed by the emission guard.
struct LinkDecision {
    /// Whether the modeled fact has already been emitted.
    emitted: bool,
}

mapping! { DependencyLinkStateDecision: LinkModelState.emitted => LinkDecision.emitted using copy; }
mapping! { DependencyLinkFactBoard: ModeledLinkBlockedBy.board => ModeledDependencyLinked.board using clone; }
mapping! { DependencyLinkFactTask: ModeledLinkBlockedBy.task => ModeledDependencyLinked.task using clone; }
mapping! { DependencyLinkFactTaskBlocks: ModeledLinkBlockedBy.task_blocks => ModeledDependencyLinked.task_blocks using clone; }
mapping! { DependencyLinkFactTaskBlockedBy: ModeledLinkBlockedBy.task_blocked_by => ModeledDependencyLinked.task_blocked_by using clone; }
mapping! { DependencyLinkFactBlocker: ModeledLinkBlockedBy.blocker => ModeledDependencyLinked.blocker using clone; }
mapping! { DependencyLinkFactBlockerBlocks: ModeledLinkBlockedBy.blocker_blocks => ModeledDependencyLinked.blocker_blocks using clone; }
mapping! { DependencyLinkFactBlockerBlockedBy: ModeledLinkBlockedBy.blocker_blocked_by => ModeledDependencyLinked.blocker_blocked_by using clone; }
mapping! { DependencyLinkFactEmitted: LinkDecision.emitted => ModeledDependencyLinked.emitted using try emit_once, error = CommandError; }

#[expect(
    clippy::missing_trait_methods,
    reason = "EventCore supplies static stream discovery for the checked modeled command"
)]
impl ModelCommandLogic for ModeledLinkBlockedBy {
    type Event = ModeledDependencyLinked;
    type State = LinkModelState;

    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the modeled decision directly constructs its one fact while preserving the checked emission-guard failure"
    )]
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = LinkDecision::model_builder()
            .emitted(DependencyLinkStateDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledDependencyLinked::model_builder()
                .board(DependencyLinkFactBoard::apply(self))
                .task(DependencyLinkFactTask::apply(self))
                .task_blocks(DependencyLinkFactTaskBlocks::apply(self))
                .task_blocked_by(DependencyLinkFactTaskBlockedBy::apply(self))
                .blocker(DependencyLinkFactBlocker::apply(self))
                .blocker_blocks(DependencyLinkFactBlockerBlocks::apply(self))
                .blocker_blocked_by(DependencyLinkFactBlockerBlockedBy::apply(self))
                .emitted(DependencyLinkFactEmitted::apply(decision.as_ref())?)
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

#[derive(ModelOutput)]
/// Provenance-consuming view used to construct the opaque publication.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the view fields retain modeled command flow from board and target through blocker before the emission marker"
)]
struct LinkedView {
    /// Board stream receiving reciprocal dependency facts.
    board: StreamId,
    /// Task that becomes blocked.
    task: TaskId,
    /// Complete tasks blocked by the target.
    task_blocks: Vec<TaskId>,
    /// Complete blockers of the target.
    task_blocked_by: Vec<TaskId>,
    /// Task that blocks the target.
    blocker: TaskId,
    /// Complete tasks blocked by the blocker.
    blocker_blocks: Vec<TaskId>,
    /// Complete blockers of the blocker.
    blocker_blocked_by: Vec<TaskId>,
    /// Exactly-once modeled emission marker.
    emitted: bool,
}

/// Permits exactly the first modeled dependency-link emission.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore's checked mapping requires this named borrowed scalar conversion for the one-use emission guard"
)]
fn emit_once(emitted: &bool) -> Result<bool, CommandError> {
    if *emitted {
        return Err("tasks_modeled_dependency_link_already_emitted".into());
    }
    Ok(true)
}

mapping! { DependencyLinkViewBoard: ModeledDependencyLinked.board => LinkedView.board using clone; }
mapping! { DependencyLinkViewTask: ModeledDependencyLinked.task => LinkedView.task using clone; }
mapping! { DependencyLinkViewTaskBlocks: ModeledDependencyLinked.task_blocks => LinkedView.task_blocks using clone; }
mapping! { DependencyLinkViewTaskBlockedBy: ModeledDependencyLinked.task_blocked_by => LinkedView.task_blocked_by using clone; }
mapping! { DependencyLinkViewBlocker: ModeledDependencyLinked.blocker => LinkedView.blocker using clone; }
mapping! { DependencyLinkViewBlockerBlocks: ModeledDependencyLinked.blocker_blocks => LinkedView.blocker_blocks using clone; }
mapping! { DependencyLinkViewBlockerBlockedBy: ModeledDependencyLinked.blocker_blocked_by => LinkedView.blocker_blocked_by using clone; }
mapping! { DependencyLinkViewEmitted: ModeledDependencyLinked.emitted => LinkedView.emitted using copy; }

/// Decides one reciprocal dependency-link publication.
///
/// # Errors
///
/// Returns a typed command failure when canonical history cannot authorize the
/// dependency or the checked model cannot produce its closed publication.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    reason = "the command boundary sequences its narrow fold, reconciliation, and checked model with typed propagation, then reuses the conventional events binding for modeled output"
)]
pub fn decide_link_blocked_by(
    events: &[TaskEvent],
    request: &LinkBlockedBy,
) -> Result<Option<DependencyLinkPublication>, TaskCommandError> {
    if request.task == request.blocker {
        return Err(TaskCommandError::DependencySelfLink {
            task: request.task.clone(),
        });
    }
    let mut state = LinkState::fold(events, request)?;
    if state.task.blocked_by.contains(&request.blocker)
        && state.blocker.blocks.contains(&request.task)
    {
        return Ok(None);
    }
    if !state.task.blocked_by.contains(&request.blocker) {
        state.task.blocked_by.push(request.blocker.clone());
    }
    if !state.blocker.blocks.contains(&request.task) {
        state.blocker.blocks.push(request.task.clone());
    }
    let streams = consistency_streams(&request.task, &request.blocker)?;
    let intent = LinkIntent::model_builder()
        .board(streams[0].clone())
        .task(request.task.clone())
        .task_blocks(state.task.blocks)
        .task_blocked_by(state.task.blocked_by)
        .blocker(request.blocker.clone())
        .blocker_blocks(state.blocker.blocks)
        .blocker_blocked_by(state.blocker.blocked_by)
        .build();
    let command = ModeledLinkBlockedBy::model_builder()
        .board(DependencyLinkIntentBoard::apply(intent.as_ref()))
        .task(DependencyLinkIntentTask::apply(intent.as_ref()))
        .task_blocks(DependencyLinkIntentTaskBlocks::apply(intent.as_ref()))
        .task_blocked_by(DependencyLinkIntentTaskBlockedBy::apply(intent.as_ref()))
        .blocker(DependencyLinkIntentBlocker::apply(intent.as_ref()))
        .blocker_blocks(DependencyLinkIntentBlockerBlocks::apply(intent.as_ref()))
        .blocker_blocked_by(DependencyLinkIntentBlockerBlockedBy::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledDependencyLinked> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledDependencyLinkDecisionFailed)?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| TaskCommandError::InvalidModeledDependencyLinkPublication)?;
    let view = LinkedView::model_builder()
        .board(DependencyLinkViewBoard::apply(&event))
        .task(DependencyLinkViewTask::apply(&event))
        .task_blocks(DependencyLinkViewTaskBlocks::apply(&event))
        .task_blocked_by(DependencyLinkViewTaskBlockedBy::apply(&event))
        .blocker(DependencyLinkViewBlocker::apply(&event))
        .blocker_blocks(DependencyLinkViewBlockerBlocks::apply(&event))
        .blocker_blocked_by(DependencyLinkViewBlockerBlockedBy::apply(&event))
        .emitted(DependencyLinkViewEmitted::apply(&event))
        .build()
        .into_inner();
    if !view.emitted {
        return Err(TaskCommandError::InvalidModeledDependencyLinkPublication);
    }
    DependencyLinkPublication::from_modeled_facts(
        TaskLinksChanged::new(
            view.board.clone(),
            view.task,
            view.task_blocks,
            view.task_blocked_by,
        ),
        TaskLinksChanged::new(
            view.board,
            view.blocker,
            view.blocker_blocks,
            view.blocker_blocked_by,
        ),
        streams,
    )
    .map(Some)
}

/// Derives the board and both endpoint streams fencing this decision.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the exact three-stream fence is constructed once with typed stream validation"
)]
fn consistency_streams(task: &TaskId, blocker: &TaskId) -> Result<[StreamId; 3], TaskCommandError> {
    let board = StreamId::try_new("tiber:board".to_owned())
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let task_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    let blocker_stream = StreamId::try_new(format!("tiber:task:{}", blocker.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    Ok([board, task_stream, blocker_stream])
}

/// Rejects an endpoint fact outside the board or that endpoint's own stream.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the compact ownership predicate derives the endpoint stream and returns its typed result directly"
)]
fn require_stream(
    task: &TaskId,
    stream: &StreamId,
    expected: &[StreamId; 3],
) -> Result<(), TaskCommandError> {
    let own_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
    if stream == &expected[0] || stream == &own_stream {
        Ok(())
    } else {
        Err(TaskCommandError::TargetTaskFactUnexpectedStream {
            task: task.clone(),
            stream: stream.clone(),
        })
    }
}
