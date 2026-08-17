//! Modeled decision for replacing one task's editable details.

use alloc::{string::String, vec::Vec};
use eventcore::{CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput, ModelState, mapping, model::{ModelCommandLogic, Modeled, ModeledEvents}};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{TaskDetailsUpdated, TaskEvent, TaskId, TaskTitle};
use super::TaskCommandError;
use crate::TaskDetailsPublication;

/// One exact replacement of the owner-editable task details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateTaskDetails { task: TaskId, title: TaskTitle, summary: String, context: String }
impl UpdateTaskDetails {
    /// Creates one semantic details-replacement request.
    #[must_use]
    pub fn new(task: TaskId, title: TaskTitle, summary: String, context: String) -> Self { Self { task, title, summary, context } }

    /// Returns the exact replacement title.
    #[must_use]
    pub const fn title(&self) -> &TaskTitle { &self.title }

    /// Returns the exact replacement summary.
    #[must_use]
    pub fn summary(&self) -> &str { &self.summary }

    /// Returns the exact replacement context.
    #[must_use]
    pub fn context(&self) -> &str { &self.context }
}

struct DetailsState {
    exists: bool,
    tags: Vec<String>,
    title: Option<TaskTitle>,
    summary: String,
    context: String,
}
impl DetailsState {
    fn fold(events: &[TaskEvent], task: &TaskId) -> Result<Self, TaskCommandError> {
        let mut state = Self {
            exists: false,
            tags: Vec::new(),
            title: None,
            summary: String::new(),
            context: String::new(),
        };
        let expected = task_stream(task)?;
        for event in events {
            match event {
                TaskEvent::TaskCreated(fact) if &fact.task.stem == task => {
                    require_stream(task, &fact.stream_id, &expected)?;
                    if state.exists { return Err(TaskCommandError::DuplicateTaskCreation { task: task.clone() }); }
                    state.exists = true;
                    state.tags.clone_from(&fact.task.tags);
                    state.title = Some(fact.task.title.clone());
                    state.summary.clone_from(&fact.task.summary);
                    state.context.clone_from(&fact.task.context);
                }
                TaskEvent::TaskDetailsUpdated(fact) if &fact.stem == task => {
                    require_stream(task, &fact.stream_id, &expected)?;
                    if !state.exists { return Err(TaskCommandError::TaskMissing { task: task.clone() }); }
                    state.tags.clone_from(&fact.tags);
                    state.title = Some(fact.title.clone());
                    state.summary.clone_from(&fact.summary);
                    state.context.clone_from(&fact.context);
                }
                TaskEvent::HistoricalTaskRemoved(fact) if &fact.stem == task => {
                    require_stream(task, &fact.stream_id, &expected)?;
                    if !state.exists { return Err(TaskCommandError::TaskMissing { task: task.clone() }); }
                    state.exists = false;
                }
                _ => {}
            }
        }
        if !state.exists { return Err(TaskCommandError::TaskMissing { task: task.clone() }); }
        Ok(state)
    }
}

#[derive(ModelInput)]
struct DetailsIntent { #[model(origin)] context: String, #[model(origin)] stream: StreamId, #[model(origin)] summary: String, #[model(origin)] tags: Vec<String>, #[model(origin)] task: TaskId, #[model(origin)] title: TaskTitle }
#[derive(ModelCommand)]
struct ModeledUpdateDetails { context: String, #[stream] stream: StreamId, summary: String, tags: Vec<String>, task: TaskId, title: TaskTitle }
mapping! { IntentContext: DetailsIntent.context => ModeledUpdateDetails.context using clone; }
mapping! { IntentStream: DetailsIntent.stream => ModeledUpdateDetails.stream using clone; }
mapping! { IntentSummary: DetailsIntent.summary => ModeledUpdateDetails.summary using clone; }
mapping! { IntentTags: DetailsIntent.tags => ModeledUpdateDetails.tags using clone; }
mapping! { IntentTask: DetailsIntent.task => ModeledUpdateDetails.task using clone; }
mapping! { IntentTitle: DetailsIntent.title => ModeledUpdateDetails.title using clone; }

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
struct ModeledDetailsUpdated { context: String, emitted: bool, stream: StreamId, summary: String, tags: Vec<String>, task: TaskId, title: TaskTitle }
impl Event for ModeledDetailsUpdated {
    fn event_type_name() -> &'static str { "TiberModeledTaskDetailsUpdated" }
    fn stream_id(&self) -> &StreamId { &self.stream }
}
#[derive(ModelOutput)]
struct DetailsView { context: String, emitted: bool, stream: StreamId, summary: String, tags: Vec<String>, task: TaskId, title: TaskTitle }
mapping! { ViewContext: ModeledDetailsUpdated.context => DetailsView.context using clone; }
mapping! { ViewEmitted: ModeledDetailsUpdated.emitted => DetailsView.emitted using copy; }
mapping! { ViewStream: ModeledDetailsUpdated.stream => DetailsView.stream using clone; }
mapping! { ViewSummary: ModeledDetailsUpdated.summary => DetailsView.summary using clone; }
mapping! { ViewTags: ModeledDetailsUpdated.tags => DetailsView.tags using clone; }
mapping! { ViewTask: ModeledDetailsUpdated.task => DetailsView.task using clone; }
mapping! { ViewTitle: ModeledDetailsUpdated.title => DetailsView.title using clone; }
impl DetailsView {
    fn from_event(event: &ModeledDetailsUpdated) -> Self {
        Self::model_builder().context(ViewContext::apply(event)).emitted(ViewEmitted::apply(event)).stream(ViewStream::apply(event))
            .summary(ViewSummary::apply(event)).tags(ViewTags::apply(event)).task(ViewTask::apply(event)).title(ViewTitle::apply(event)).build().into_inner()
    }
}

#[derive(ModelState)]
struct DetailsModelState { #[model(default)] emitted: bool }
#[derive(ModelOutput)]
struct DetailsDecision { emitted: bool }
mapping! { StateDecision: DetailsModelState.emitted => DetailsDecision.emitted using copy; }
mapping! { FactContext: ModeledUpdateDetails.context => ModeledDetailsUpdated.context using clone; }
mapping! { FactStream: ModeledUpdateDetails.stream => ModeledDetailsUpdated.stream using clone; }
mapping! { FactSummary: ModeledUpdateDetails.summary => ModeledDetailsUpdated.summary using clone; }
mapping! { FactTags: ModeledUpdateDetails.tags => ModeledDetailsUpdated.tags using clone; }
mapping! { FactTask: ModeledUpdateDetails.task => ModeledDetailsUpdated.task using clone; }
mapping! { FactTitle: ModeledUpdateDetails.title => ModeledDetailsUpdated.title using clone; }
mapping! { FactEmitted: DetailsDecision.emitted => ModeledDetailsUpdated.emitted using try emit_once, error = CommandError; }
impl ModelCommandLogic for ModeledUpdateDetails {
    type Event = ModeledDetailsUpdated; type State = DetailsModelState;
    fn decide(&self, state: Modeled<Self::State>) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = DetailsDecision::model_builder().emitted(StateDecision::apply(state.as_ref())).build();
        Ok(ModeledEvents::one(ModeledDetailsUpdated::model_builder().context(FactContext::apply(self)).emitted(FactEmitted::apply(decision.as_ref())?)
            .stream(FactStream::apply(self)).summary(FactSummary::apply(self)).tags(FactTags::apply(self)).task(FactTask::apply(self)).title(FactTitle::apply(self)).build()))
    }
    fn evolve(&self, state: Modeled<Self::State>, _event: &Self::Event) -> Modeled<Self::State> { let mut state = state.into_inner(); state.emitted = true; Modeled::from_built(state) }
}
fn emit_once(emitted: &bool) -> Result<bool, CommandError> { if *emitted { return Err("tasks_modeled_details_already_emitted".into()); } Ok(true) }

/// Returns the opaque modeled publication for one details replacement.
pub fn decide_update_task_details(events: &[TaskEvent], request: &UpdateTaskDetails) -> Result<Option<TaskDetailsPublication>, TaskCommandError> {
    let state = DetailsState::fold(events, &request.task)?; let stream = task_stream(&request.task)?;
    if state.title.as_ref() == Some(&request.title)
        && state.summary == request.summary
        && state.context == request.context
    {
        return Ok(None);
    }
    let intent = DetailsIntent::model_builder().context(request.context.clone()).stream(stream.clone()).summary(request.summary.clone()).tags(state.tags).task(request.task.clone()).title(request.title.clone()).build();
    let command = ModeledUpdateDetails::model_builder().context(IntentContext::apply(intent.as_ref())).stream(IntentStream::apply(intent.as_ref())).summary(IntentSummary::apply(intent.as_ref())).tags(IntentTags::apply(intent.as_ref())).task(IntentTask::apply(intent.as_ref())).title(IntentTitle::apply(intent.as_ref())).build();
    let events: Vec<ModeledDetailsUpdated> = CommandLogic::handle(&command, Modeled::default()).map_err(|_source| TaskCommandError::ModeledTaskDetailsDecisionFailed)?.into();
    let [event] = events.try_into().map_err(|_events| TaskCommandError::InvalidModeledTaskDetailsPublication)?; let view = DetailsView::from_event(&event);
    if !view.emitted { return Err(TaskCommandError::InvalidModeledTaskDetailsPublication); }
    TaskDetailsPublication::from_modeled_fact(TaskDetailsUpdated::new(view.stream.clone(), view.task, view.title, view.tags, view.summary, view.context), stream).map(Some)
}
fn task_stream(task: &TaskId) -> Result<StreamId, TaskCommandError> { StreamId::try_new(format!("tiber:task:{}", task.as_str())).map_err(|_source| TaskCommandError::InvalidTaskStream) }
fn require_stream(task: &TaskId, actual: &StreamId, expected: &StreamId) -> Result<(), TaskCommandError> { if actual.as_ref() == "tiber:board" || actual == expected { return Ok(()); } Err(TaskCommandError::TargetTaskFactUnexpectedStream { task: task.clone(), stream: actual.clone() }) }
