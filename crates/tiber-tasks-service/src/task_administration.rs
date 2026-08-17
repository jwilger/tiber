//! Modeled decisions for native task-board administration.

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{Task, TaskCreated, TaskEvent, TaskId, TaskOrder, TaskTitle};

use super::{TASK_BOARD_STREAM, TaskCommandError};
use crate::TaskCreationPublication;

/// Request to create one backlog task from owner-supplied title and adapter-assigned metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateTask {
    /// Stable adapter-supplied identity prefix.
    id_prefix: TaskId,
    /// Timestamp recorded in the durable task fact.
    recorded_at: String,
    /// Whether an identical durable request should reconcile as a retry.
    retry_identity: bool,
    /// Owner-supplied normalized task title.
    title: TaskTitle,
}

/// Closed result of one task-creation decision.
pub enum TaskCreationDecision {
    /// The stable creation identity is already durable; no publication is needed.
    AlreadyCreated(TaskId),
    /// One new modeled creation batch must be published.
    Publish(TaskCreationPublication),
}

impl CreateTask {
    /// Creates one semantic task-creation request.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic request constructor reads clearly as its final value"
    )]
    pub fn new(id_prefix: TaskId, recorded_at: String, title: TaskTitle) -> Self {
        Self {
            id_prefix,
            recorded_at,
            retry_identity: true,
            title,
        }
    }

    /// Creates an implicit owner request that must remain distinct from prior invocations.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic request constructor reads clearly as its final value"
    )]
    pub fn new_implicit(id_prefix: TaskId, recorded_at: String, title: TaskTitle) -> Self {
        Self {
            id_prefix,
            recorded_at,
            retry_identity: false,
            title,
        }
    }
}

/// Minimal retained task-board state needed to decide one creation.
struct CreationState {
    /// Current task titles indexed by their durable identities.
    current_tasks: BTreeMap<TaskId, TaskTitle>,
    /// Strict durable ordering of current tasks.
    order: Vec<TaskId>,
}

impl CreationState {
    /// Decides whether the request is already durable or needs one publication.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the retry-identity predicates remain clearest as compact iterator expressions"
    )]
    fn decide(&self, request: &CreateTask) -> Result<TaskCreationDecision, TaskCommandError> {
        if request.retry_identity {
            let retry_base = format!(
                "{}-{}",
                request.id_prefix.as_str(),
                request.title.file_stem()
            );
            if let Some((task_id, _title)) = self.current_tasks.iter().find(|&(task_id, title)| {
                let suffix = task_id
                    .as_str()
                    .strip_prefix(&retry_base)
                    .and_then(|remainder| remainder.strip_prefix('-'))
                    .and_then(|text| text.parse::<usize>().ok().map(|number| (text, number)));
                title == &request.title
                    && (task_id.as_str() == retry_base
                        || suffix.is_some_and(|(text, number)| {
                            number >= 2 && number.to_string() == text
                        }))
            }) {
                return Ok(TaskCreationDecision::AlreadyCreated(task_id.clone()));
            }
        }
        let board_stream = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
            .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
        let task_id = self.final_task_id(request)?;
        let task = Task::new_backlog(
            task_id.clone(),
            request.title.clone(),
            request.recorded_at.clone(),
        );
        let mut order = self.order.clone();
        order.retain(|existing| existing != &task_id);
        order.push(task_id);
        modeled_creation_publication(&board_stream, task, order).map(TaskCreationDecision::Publish)
    }

    /// Allocates the first collision-free identity for the requested title.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the collision predicate is clearest as a compact iterator expression"
    )]
    fn final_task_id(&self, request: &CreateTask) -> Result<TaskId, TaskCommandError> {
        let base = request.title.file_stem();
        let mut nickname = base.clone();
        let mut suffix: usize = 2;
        while self
            .current_tasks
            .keys()
            .any(|id| id.as_str().ends_with(&format!("-{nickname}")))
        {
            nickname = format!("{base}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        let task_id = TaskId::parse(&format!("{}-{nickname}", request.id_prefix.as_str()))
            .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
        StreamId::try_new(format!("tiber:task:{}", task_id.as_str()))
            .map_err(|_source| TaskCommandError::InvalidTaskStream)?;
        Ok(task_id)
    }

    /// Folds canonical task facts into the minimal creation authority.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the named fold keeps history reconstruction distinct from the final creation decision"
    )]
    fn fold(events: &[TaskEvent]) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            current_tasks: BTreeMap::new(),
            order: Vec::new(),
        };
        for event in events {
            state.fold_event(event)?;
        }
        Ok(state)
    }

    /// Applies one canonical fact to the minimal creation authority.
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "match ergonomics preserve borrowed event payloads without the conflicting explicit-reference patterns"
    )]
    fn fold_event(&mut self, event: &TaskEvent) -> Result<(), TaskCommandError> {
        match event {
            TaskEvent::TaskCreated(created) => {
                validate_task_fact_stream(&created.task.stem, &created.stream_id)?;
                let previous = self
                    .current_tasks
                    .insert(created.task.stem.clone(), created.task.title.clone());
                if previous.is_some() {
                    return Err(TaskCommandError::DuplicateTaskCreation {
                        task: created.task.stem.clone(),
                    });
                }
                Ok(())
            }
            TaskEvent::HistoricalTaskRemoved(removed) => {
                validate_task_fact_stream(&removed.stem, &removed.stream_id)?;
                if self.current_tasks.remove(&removed.stem).is_none() {
                    return Err(TaskCommandError::TaskMissing {
                        task: removed.stem.clone(),
                    });
                }
                self.order.retain(|task| task != &removed.stem);
                Ok(())
            }
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                self.replace_order(&order.stream_id, &order.order)
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                self.replace_order(&closed.stream_id, &closed.order)
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                if repaired.stream_id.as_ref() != TASK_BOARD_STREAM {
                    return Err(TaskCommandError::TaskCreationMalformedHistory);
                }
                if let Some(order) = repaired.order_change.as_ref() {
                    if order.stream_id != repaired.stream_id {
                        return Err(TaskCommandError::TaskCreationMalformedHistory);
                    }
                    self.replace_order(&order.stream_id, &order.order)?;
                }
                Ok(())
            }
            TaskEvent::RepositoryInitialized(_)
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
            | TaskEvent::HistoricalTaskStatePublished(_) => Ok(()),
            // `TaskEvent` is non-exhaustive. A future fact must receive an
            // explicit creation-authority decision before this fold may
            // consume or ignore it.
            _ => Err(TaskCommandError::UnsupportedTaskEvent),
        }
    }

    /// Replaces the retained board order after validating its authority and uniqueness.
    #[expect(
        clippy::implicit_return,
        reason = "the validated order replacement returns its unit success directly"
    )]
    fn replace_order(
        &mut self,
        stream: &StreamId,
        order: &[TaskId],
    ) -> Result<(), TaskCommandError> {
        if stream.as_ref() != TASK_BOARD_STREAM || has_duplicate_order(order) {
            return Err(TaskCommandError::TaskCreationMalformedHistory);
        }
        self.order = order.to_vec();
        Ok(())
    }
}

/// Checked input shape that carries every field into the modeled command.
#[derive(ModelInput)]
struct CreateTaskIntent {
    /// Board stream receiving the modeled facts.
    #[model(origin)]
    board_stream: StreamId,
    /// Complete durable task order after creation.
    #[model(origin)]
    order: Vec<TaskId>,
    /// Newly created backlog task.
    #[model(origin)]
    task: Task,
}

/// `EventCore` command that emits one modeled task-creation fact.
#[derive(ModelCommand)]
struct ModeledCreateTask {
    /// Board stream receiving the modeled fact.
    #[stream]
    board_stream: StreamId,
    /// Complete durable task order after creation.
    order: Vec<TaskId>,
    /// Newly created backlog task.
    task: Task,
}

mapping! { CreateTaskIntentToBoardStream:
CreateTaskIntent.board_stream => ModeledCreateTask.board_stream using clone; }
mapping! { CreateTaskIntentToOrder:
CreateTaskIntent.order => ModeledCreateTask.order using clone; }
mapping! { CreateTaskIntentToTask:
CreateTaskIntent.task => ModeledCreateTask.task using clone; }

/// Internal modeled fact from which the opaque durable publication is derived.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledTaskCreated {
    /// Closed creation-only state; this command never emits `false`.
    created: bool,
    /// Complete durable task order after creation.
    order: Vec<TaskId>,
    /// Board stream receiving the modeled fact.
    stream: StreamId,
    /// Newly created backlog task.
    task: Task,
}

impl Event for ModeledTaskCreated {
    #[expect(
        clippy::implicit_return,
        reason = "the stable event type name is the complete result"
    )]
    fn event_type_name() -> &'static str {
        "TiberModeledTaskCreated"
    }

    #[expect(
        clippy::implicit_return,
        reason = "the modeled event carries exactly one stream identity"
    )]
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

/// Query-shaped view that consumes every modeled creation fact field.
#[derive(ModelOutput)]
struct ModeledTaskCreatedView {
    /// Closed creation-only state.
    created: bool,
    /// Complete durable task order after creation.
    order: Vec<TaskId>,
    /// Board stream that received the modeled fact.
    stream: StreamId,
    /// Newly created backlog task.
    task: Task,
}

impl ModeledTaskCreatedView {
    /// Projects every modeled event field into the durable publication shape.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the named provenance projection keeps all modeled fact fields visibly consumed"
    )]
    fn from_event(event: &ModeledTaskCreated) -> Self {
        Self::model_builder()
            .created(ModeledTaskCreatedToViewCreated::apply(event))
            .order(ModeledTaskCreatedToViewOrder::apply(event))
            .stream(ModeledTaskCreatedToViewStream::apply(event))
            .task(ModeledTaskCreatedToViewTask::apply(event))
            .build()
            .into_inner()
    }
}

mapping! { ModeledTaskCreatedToViewCreated:
ModeledTaskCreated.created => ModeledTaskCreatedView.created using copy; }

mapping! { ModeledTaskCreatedToViewOrder:
ModeledTaskCreated.order => ModeledTaskCreatedView.order using clone; }
mapping! { ModeledTaskCreatedToViewStream:
ModeledTaskCreated.stream => ModeledTaskCreatedView.stream using clone; }
mapping! { ModeledTaskCreatedToViewTask:
ModeledTaskCreated.task => ModeledTaskCreatedView.task using clone; }

/// Minimal modeled state for exactly-once emission within one command execution.
#[derive(ModelState)]
struct ModeledCreateTaskState {
    /// Whether this modeled command already emitted its one fact.
    #[model(default)]
    emitted: bool,
}

/// Decision state consumed by the creation-only fact constructor.
#[derive(ModelOutput)]
struct ModeledCreateTaskDecision {
    /// Whether this modeled command already emitted its one fact.
    emitted: bool,
}

mapping! { ModeledCreateTaskStateToDecision:
ModeledCreateTaskState.emitted => ModeledCreateTaskDecision.emitted using copy; }
mapping! { ModeledCreateTaskToFactOrder:
ModeledCreateTask.order => ModeledTaskCreated.order using clone; }
mapping! { ModeledCreateTaskToFactStream:
ModeledCreateTask.board_stream => ModeledTaskCreated.stream using clone; }
mapping! { ModeledCreateTaskToFactTask:
ModeledCreateTask.task => ModeledTaskCreated.task using clone; }

mapping! { ModeledCreateTaskDecisionToFactCreated:
ModeledCreateTaskDecision.emitted => ModeledTaskCreated.created
using try create_once, error = CommandError; }

#[expect(
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::question_mark_used,
    reason = "EventCore provides stream discovery while modeled construction propagates its typed mapping failure"
)]
impl ModelCommandLogic for ModeledCreateTask {
    type Event = ModeledTaskCreated;
    type State = ModeledCreateTaskState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ModeledCreateTaskDecision::model_builder()
            .emitted(ModeledCreateTaskStateToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            ModeledTaskCreated::model_builder()
                .created(ModeledCreateTaskDecisionToFactCreated::apply(
                    decision.as_ref(),
                )?)
                .order(ModeledCreateTaskToFactOrder::apply(self))
                .stream(ModeledCreateTaskToFactStream::apply(self))
                .task(ModeledCreateTaskToFactTask::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        folded.emitted = true;
        Modeled::from_built(folded)
    }
}

/// Permits exactly the first modeled creation emission.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore's checked mapping names this one-use conversion as a function"
)]
fn create_once(emitted: &bool) -> Result<bool, CommandError> {
    if *emitted {
        return Err("tasks_modeled_creation_already_emitted".into());
    }
    Ok(true)
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
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the public decision boundary preserves the fold's typed history failure"
)]
pub fn decide_create_task(
    events: &[TaskEvent],
    request: &CreateTask,
) -> Result<TaskCreationDecision, TaskCommandError> {
    CreationState::fold(events)?.decide(request)
}

/// Returns whether a retained task order contains any duplicate identity.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the set-cardinality comparison is clearest as one predicate expression"
)]
fn has_duplicate_order(order: &[TaskId]) -> bool {
    order.iter().collect::<BTreeSet<_>>().len() != order.len()
}

/// Builds the opaque two-fact publication through `EventCore`'s checked model.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the named modeled boundary keeps EventCore provenance separate from task identity allocation while propagating typed model failures"
)]
fn modeled_creation_publication(
    board_stream: &StreamId,
    task: Task,
    order: Vec<TaskId>,
) -> Result<TaskCreationPublication, TaskCommandError> {
    let intent = CreateTaskIntent::model_builder()
        .board_stream(board_stream.clone())
        .order(order)
        .task(task)
        .build();
    let command = ModeledCreateTask::model_builder()
        .board_stream(CreateTaskIntentToBoardStream::apply(intent.as_ref()))
        .order(CreateTaskIntentToOrder::apply(intent.as_ref()))
        .task(CreateTaskIntentToTask::apply(intent.as_ref()))
        .build();
    let events: Vec<ModeledTaskCreated> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_source| TaskCommandError::ModeledTaskCreationDecisionFailed)?
        .into();
    let [event]: [ModeledTaskCreated; 1] = events
        .try_into()
        .map_err(|_events| TaskCommandError::InvalidModeledTaskCreationPublication)?;
    let view = ModeledTaskCreatedView::from_event(&event);
    if !view.created {
        return Err(TaskCommandError::InvalidModeledTaskCreationPublication);
    }
    TaskCreationPublication::from_modeled_facts(
        TaskCreated::new(view.stream.clone(), view.task),
        TaskOrder::new(view.stream.clone(), view.order),
        view.stream,
    )
}

/// Ensures a task-scoped historical fact arrived on the board or its own stream.
#[expect(
    clippy::implicit_return,
    reason = "the final unit success avoids the conflicting needless-return lint"
)]
fn validate_task_fact_stream(task: &TaskId, stream: &StreamId) -> Result<(), TaskCommandError> {
    let board_stream = match StreamId::try_new(TASK_BOARD_STREAM.to_owned()) {
        Ok(parsed_board_stream) => parsed_board_stream,
        Err(_source) => return Err(TaskCommandError::InvalidTaskStream),
    };
    let task_stream = match StreamId::try_new(format!("tiber:task:{}", task.as_str())) {
        Ok(parsed_task_stream) => parsed_task_stream,
        Err(_source) => return Err(TaskCommandError::InvalidTaskStream),
    };
    if stream != &board_stream && stream != &task_stream {
        return Err(TaskCommandError::TargetTaskFactUnexpectedStream {
            task: task.clone(),
            stream: stream.clone(),
        });
    }
    Ok(())
}
