//! Modeled validation and deterministic repair of native task-board facts.

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec,
    vec::Vec,
};

use eventcore::{
    CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput, ModelState, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents},
};
use eventcore_types::StreamId;
use serde::{Deserialize, Serialize};
use tiber_tasks_core::{
    TaskEvent, TaskId, TaskLinksChanged, TaskOrder, TaskStatus, TaskValidationRepaired,
    ValidationRepair,
};

use super::{
    TaskValidationPublication,
    command::{TASK_BOARD_STREAM, TaskCommandError},
};

/// One dependency reference whose target is absent from current task history.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct DanglingTaskLink {
    /// Link field containing the invalid reference.
    pub field: &'static str,
    /// Missing target identity.
    pub target: TaskId,
    /// Task containing the invalid reference.
    pub task: TaskId,
}

/// Closed validation result containing safe repairs and report-only findings.
#[derive(Debug)]
#[non_exhaustive]
pub struct TaskValidationDecision {
    /// Dangling references that require owner resolution.
    pub dangling_links: Vec<DanglingTaskLink>,
    /// Dependency cycles represented by their stable member identities.
    pub dependency_cycles: Vec<Vec<TaskId>>,
    /// Safe modeled repair, absent when no safe mutation is required.
    pub publication: Option<TaskValidationPublication>,
}

#[derive(Clone, Debug, Default)]
/// Minimal board state needed by the validation decision.
struct BoardState {
    /// Latest complete open-board order.
    order: Vec<TaskId>,
    /// Current task state indexed by durable identity.
    tasks: BTreeMap<TaskId, CurrentTask>,
}

#[derive(Clone, Debug)]
/// Task fields needed to validate links, lifecycle, and order membership.
struct CurrentTask {
    /// Current prerequisite identities.
    blocked_by: Vec<TaskId>,
    /// Current dependent identities.
    blocks: Vec<TaskId>,
    /// Current durable lifecycle status.
    status: TaskStatus,
}

#[derive(Clone, Debug)]
/// Deterministic safe repair derived from canonical board facts.
struct RepairPlan {
    /// Complete reciprocal link replacements.
    link_changes: Vec<TaskLinksChanged>,
    /// Complete repaired board order when membership drift exists.
    order_change: Option<TaskOrder>,
    /// Stable observations describing every deterministic change.
    repairs: Vec<ValidationRepair>,
    /// Canonical board stream receiving the repair fact.
    stream: StreamId,
}

#[derive(ModelInput)]
/// Provenance-bearing validation input.
struct ValidationIntent {
    /// Complete pure repair plan.
    #[model(origin)]
    plan: RepairPlan,
}

#[derive(ModelCommand)]
/// Checked validation command for one board repair.
struct ValidationCommand {
    /// Complete pure repair plan.
    plan: RepairPlan,
    /// Canonical board stream receiving the modeled fact.
    #[stream]
    stream: StreamId,
}

mapping! { ValidationIntentPlan: ValidationIntent.plan => ValidationCommand.plan using clone; }
mapping! { ValidationIntentStream: ValidationIntent.plan => ValidationCommand.stream using repair_plan_stream; }

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
/// Internal modeled fact consumed into the durable repair event.
struct ValidationFact {
    /// Whether the single modeled fact was emitted.
    emitted: bool,
    /// Complete reciprocal link replacements.
    link_changes: Vec<TaskLinksChanged>,
    /// Complete repaired order when needed.
    order_change: Option<TaskOrder>,
    /// Stable repair observations.
    repairs: Vec<ValidationRepair>,
    /// Canonical board stream.
    stream: StreamId,
}

impl Event for ValidationFact {
    fn event_type_name() -> &'static str {
        "TiberModeledTaskBoardValidated"
    }

    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[derive(ModelState)]
/// Single-emission checked-model state.
struct ValidationState {
    /// Whether the modeled fact has already evolved.
    #[model(default)]
    emitted: bool,
}

#[derive(ModelOutput)]
/// Checked single-emission decision.
struct ValidationDecision {
    /// Whether the modeled fact has already evolved.
    emitted: bool,
}

#[derive(ModelOutput)]
/// Provenance-consuming view used to construct the opaque publication.
struct ValidationView {
    /// Whether the modeled fact was emitted.
    emitted: bool,
    /// Complete reciprocal link replacements.
    link_changes: Vec<TaskLinksChanged>,
    /// Complete repaired order when needed.
    order_change: Option<TaskOrder>,
    /// Stable repair observations.
    repairs: Vec<ValidationRepair>,
    /// Canonical board stream.
    stream: StreamId,
}

mapping! { ValidationStateDecision: ValidationState.emitted => ValidationDecision.emitted using copy; }
mapping! { ValidationFactEmitted: ValidationDecision.emitted => ValidationFact.emitted using invert; }
mapping! { ValidationFactLinks: ValidationCommand.plan => ValidationFact.link_changes using plan_links; }
mapping! { ValidationFactOrder: ValidationCommand.plan => ValidationFact.order_change using plan_order; }
mapping! { ValidationFactRepairs: ValidationCommand.plan => ValidationFact.repairs using plan_repairs; }
mapping! { ValidationFactStream: ValidationCommand.stream => ValidationFact.stream using clone; }
mapping! { ValidationViewLinks: ValidationFact.link_changes => ValidationView.link_changes using clone; }
mapping! { ValidationViewEmitted: ValidationFact.emitted => ValidationView.emitted using copy; }
mapping! { ValidationViewOrder: ValidationFact.order_change => ValidationView.order_change using clone; }
mapping! { ValidationViewRepairs: ValidationFact.repairs => ValidationView.repairs using clone; }
mapping! { ValidationViewStream: ValidationFact.stream => ValidationView.stream using clone; }

#[expect(
    clippy::missing_trait_methods,
    reason = "the EventCore command uses the trait's default stream selection and error hooks"
)]
impl ModelCommandLogic for ValidationCommand {
    type Event = ValidationFact;
    type State = ValidationState;

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, eventcore::CommandError> {
        let decision = ValidationDecision::model_builder()
            .emitted(ValidationStateDecision::apply(state.as_ref()))
            .build();
        if decision.as_ref().emitted {
            return Ok(ModeledEvents::none("validation repair already emitted"));
        }
        Ok(ModeledEvents::one(
            ValidationFact::model_builder()
                .emitted(ValidationFactEmitted::apply(decision.as_ref()))
                .link_changes(ValidationFactLinks::apply(self))
                .order_change(ValidationFactOrder::apply(self))
                .repairs(ValidationFactRepairs::apply(self))
                .stream(ValidationFactStream::apply(self))
                .build(),
        ))
    }

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        Modeled::from_built(ValidationState {
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
/// Copies planned reciprocal link changes into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_links(plan: &RepairPlan) -> Vec<TaskLinksChanged> {
    plan.link_changes.clone()
}
/// Copies the optional planned board order into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_order(plan: &RepairPlan) -> Option<TaskOrder> {
    plan.order_change.clone()
}
/// Copies stable repair observations into the modeled fact.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn plan_repairs(plan: &RepairPlan) -> Vec<ValidationRepair> {
    plan.repairs.clone()
}
/// Selects the canonical board stream from the repair plan.
#[expect(
    clippy::single_call_fn,
    reason = "the named mapping helper has one checked-model caller"
)]
fn repair_plan_stream(plan: &RepairPlan) -> StreamId {
    plan.stream.clone()
}

/// Decides safe task-board repairs and report-only graph findings from canonical facts.
///
/// # Errors
///
/// Returns a typed command error when retained history is malformed or the
/// checked model cannot close the exact repair publication.
#[inline]
pub fn decide(events: &[TaskEvent]) -> Result<TaskValidationDecision, TaskCommandError> {
    let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
        .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?;
    let state = fold(events)?;
    let dangling_links = dangling_links(&state);
    let dependency_cycles = dependency_cycles(&state);
    let plan = repair_plan(&state, board.clone());
    let publication = if plan.repairs.is_empty() {
        None
    } else {
        Some(modeled_publication(
            plan,
            consistency_streams(&state, board)?,
        )?)
    };
    Ok(TaskValidationDecision {
        dangling_links,
        dependency_cycles,
        publication,
    })
}

/// Folds canonical facts into validation's minimal decision state.
#[expect(
    clippy::match_same_arms,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    clippy::too_many_lines,
    reason = "the validation fold reads a borrowed non-exhaustive event vocabulary at one bounded boundary; known irrelevant facts are explicit and the wildcard preserves forward compatibility"
)]
fn fold(events: &[TaskEvent]) -> Result<BoardState, TaskCommandError> {
    let board = StreamId::try_new(TASK_BOARD_STREAM.to_owned())
        .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?;
    let mut state = BoardState::default();
    for event in events {
        match event {
            TaskEvent::TaskCreated(created) => {
                if !valid_task_stream(&board, &created.task.stem, &created.stream_id)?
                    || state
                        .tasks
                        .insert(
                            created.task.stem.clone(),
                            CurrentTask {
                                blocked_by: created.task.blocked_by.clone(),
                                blocks: created.task.blocks.clone(),
                                status: created.task.status,
                            },
                        )
                        .is_some()
                {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
            }
            TaskEvent::TaskTransitioned(changed) => {
                if !valid_task_stream(&board, &changed.stem, &changed.stream_id)? {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                let task = state
                    .tasks
                    .get_mut(&changed.stem)
                    .ok_or(TaskCommandError::TaskValidationMalformedHistory)?;
                if !allowed_transition(task.status, changed.status) {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                task.status = changed.status;
            }
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                if order.stream_id != board {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                state.order.clone_from(&order.order);
            }
            TaskEvent::TaskLinksChanged(changed) => {
                apply_links(&mut state, &board, changed)?;
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                if repaired.stream_id != board {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                for changed in &repaired.link_changes {
                    if changed.stream_id != board {
                        return Err(TaskCommandError::TaskValidationMalformedHistory);
                    }
                    apply_links(&mut state, &board, changed)?;
                }
                if let Some(order) = repaired.order_change.as_ref() {
                    if order.stream_id != board {
                        return Err(TaskCommandError::TaskValidationMalformedHistory);
                    }
                    state.order.clone_from(&order.order);
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                if closed.stream_id != board {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                for task in &closed.stems {
                    let current = state
                        .tasks
                        .get_mut(task)
                        .ok_or(TaskCommandError::TaskValidationMalformedHistory)?;
                    current.status = TaskStatus::Done;
                }
                state.order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) => {
                if !valid_task_stream(&board, &closed.stem, &closed.stream_id)? {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
                let task = state
                    .tasks
                    .get_mut(&closed.stem)
                    .ok_or(TaskCommandError::TaskValidationMalformedHistory)?;
                task.status = TaskStatus::Done;
            }
            TaskEvent::HistoricalTaskRemoved(removed) => {
                if !valid_task_stream(&board, &removed.stem, &removed.stream_id)?
                    || state.tasks.remove(&removed.stem).is_none()
                {
                    return Err(TaskCommandError::TaskValidationMalformedHistory);
                }
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
            _ => {}
        }
    }
    Ok(state)
}

/// Applies one complete link replacement to current validation state.
fn apply_links(
    state: &mut BoardState,
    board: &StreamId,
    changed: &TaskLinksChanged,
) -> Result<(), TaskCommandError> {
    if !valid_task_stream(board, &changed.stem, &changed.stream_id)? {
        return Err(TaskCommandError::TaskValidationMalformedHistory);
    }
    let task = state
        .tasks
        .get_mut(&changed.stem)
        .ok_or(TaskCommandError::TaskValidationMalformedHistory)?;
    task.blocked_by.clone_from(&changed.blocked_by);
    task.blocks.clone_from(&changed.blocks);
    Ok(())
}

/// Returns whether a task fact belongs to the board or its exact task stream.
fn valid_task_stream(
    board: &StreamId,
    task: &TaskId,
    stream: &StreamId,
) -> Result<bool, TaskCommandError> {
    let task_stream = StreamId::try_new(format!("tiber:task:{}", task.as_str()))
        .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?;
    Ok(stream == board || stream == &task_stream)
}

/// Returns whether one retained lifecycle transition is semantically legal.
#[expect(
    clippy::single_call_fn,
    reason = "the validation fold has one lifecycle validation helper"
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

/// Derives safe reciprocal-link and open-board membership repairs.
#[expect(
    clippy::single_call_fn,
    reason = "the pure validation decision derives one complete repair plan"
)]
fn repair_plan(state: &BoardState, board: StreamId) -> RepairPlan {
    let repaired = normalized_tasks(state);
    let mut link_changes = Vec::new();
    let mut repairs = Vec::new();
    for (task_id, task) in &repaired {
        let Some(original) = state.tasks.get(task_id) else {
            continue;
        };
        if original.blocks != task.blocks || original.blocked_by != task.blocked_by {
            for target in task
                .blocks
                .iter()
                .filter(|target| !original.blocks.contains(*target))
            {
                repairs.push(ValidationRepair::reciprocal_link_added(
                    task_id.clone(),
                    String::from("blocks"),
                    target.clone(),
                ));
            }
            for target in task
                .blocked_by
                .iter()
                .filter(|target| !original.blocked_by.contains(*target))
            {
                repairs.push(ValidationRepair::reciprocal_link_added(
                    task_id.clone(),
                    String::from("blocked_by"),
                    target.clone(),
                ));
            }
            link_changes.push(TaskLinksChanged::new(
                board.clone(),
                task_id.clone(),
                task.blocks.clone(),
                task.blocked_by.clone(),
            ));
        }
    }
    let open = state
        .tasks
        .iter()
        .filter(|&(_task_id, task)| {
            matches!(task.status, TaskStatus::Backlog | TaskStatus::InProgress)
        })
        .map(|(task, _)| task.clone())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut order = state
        .order
        .iter()
        .filter(|task| open.contains(*task) && seen.insert((*task).clone()))
        .cloned()
        .collect::<Vec<_>>();
    order.extend(open.iter().filter(|task| !seen.contains(*task)).cloned());
    let order_change = (order != state.order).then(|| {
        for task in order.iter().filter(|task| !state.order.contains(*task)) {
            repairs.push(ValidationRepair::board_entry_added(task.clone()));
        }
        let mut retained =
            order
                .iter()
                .fold(BTreeMap::<TaskId, usize>::new(), |mut counts, task| {
                    let count = counts.entry(task.clone()).or_default();
                    *count = count.saturating_add(1);
                    counts
                });
        for task in &state.order {
            let count = retained.entry(task.clone()).or_default();
            if *count == 0 {
                repairs.push(ValidationRepair::board_entry_removed(task.clone()));
            } else {
                *count = count.saturating_sub(1);
            }
        }
        TaskOrder::new(board.clone(), order)
    });
    RepairPlan {
        link_changes,
        order_change,
        repairs,
        stream: board,
    }
}

/// Normalizes known reciprocal edges without inventing dangling targets.
fn normalized_tasks(state: &BoardState) -> BTreeMap<TaskId, CurrentTask> {
    let mut repaired = state.tasks.clone();
    for (task_id, task) in &state.tasks {
        for target in &task.blocks {
            if let Some(blocked) = repaired.get_mut(target)
                && !blocked.blocked_by.contains(task_id)
            {
                blocked.blocked_by.push(task_id.clone());
            }
        }
        for target in &task.blocked_by {
            if let Some(blocker) = repaired.get_mut(target)
                && !blocker.blocks.contains(task_id)
            {
                blocker.blocks.push(task_id.clone());
            }
        }
    }
    repaired
}

/// Reports dependency targets absent from current task history.
#[expect(
    clippy::single_call_fn,
    reason = "validation computes dangling findings once per decision"
)]
fn dangling_links(state: &BoardState) -> Vec<DanglingTaskLink> {
    let mut findings = Vec::new();
    for (task_id, task) in &state.tasks {
        for target in &task.blocks {
            if !state.tasks.contains_key(target) {
                findings.push(DanglingTaskLink {
                    field: "blocks",
                    target: target.clone(),
                    task: task_id.clone(),
                });
            }
        }
        for target in &task.blocked_by {
            if !state.tasks.contains_key(target) {
                findings.push(DanglingTaskLink {
                    field: "blocked_by",
                    target: target.clone(),
                    task: task_id.clone(),
                });
            }
        }
    }
    findings
}

/// Reports cycles from the normalized union of both reciprocal link fields.
#[expect(
    clippy::single_call_fn,
    reason = "validation computes cycle findings once per decision"
)]
fn dependency_cycles(state: &BoardState) -> Vec<Vec<TaskId>> {
    /// Visits one normalized dependency path and records stable cycle members.
    fn visit(
        task: &TaskId,
        state: &BoardState,
        visiting: &mut Vec<TaskId>,
        complete: &mut BTreeSet<TaskId>,
        cycles: &mut BTreeSet<Vec<TaskId>>,
    ) {
        if let Some(start) = visiting.iter().position(|current| current == task) {
            let mut cycle = visiting.get(start..).unwrap_or_default().to_vec();
            cycle.sort();
            cycles.insert(cycle);
            return;
        }
        if complete.contains(task) {
            return;
        }
        visiting.push(task.clone());
        if let Some(current) = state.tasks.get(task) {
            for blocker in &current.blocked_by {
                if state.tasks.contains_key(blocker) {
                    visit(blocker, state, visiting, complete, cycles);
                }
            }
        }
        visiting.pop();
        complete.insert(task.clone());
    }
    let normalized = BoardState {
        order: state.order.clone(),
        tasks: normalized_tasks(state),
    };
    let mut complete = BTreeSet::new();
    let mut cycles = BTreeSet::new();
    for task in normalized.tasks.keys() {
        visit(
            task,
            &normalized,
            &mut Vec::new(),
            &mut complete,
            &mut cycles,
        );
    }
    cycles.into_iter().collect()
}

/// Derives the exact board and current task streams read by validation.
#[expect(
    clippy::single_call_fn,
    reason = "one opaque validation publication consumes this exact fence"
)]
fn consistency_streams(
    state: &BoardState,
    board: StreamId,
) -> Result<Vec<StreamId>, TaskCommandError> {
    let mut streams = vec![board];
    for task in state.tasks.keys() {
        streams.push(
            StreamId::try_new(format!("tiber:task:{}", task.as_str()))
                .map_err(|_invalid_stream| TaskCommandError::InvalidTaskStream)?,
        );
    }
    Ok(streams)
}

/// Runs the checked model and closes its fully consumed output into a token.
#[expect(
    clippy::single_call_fn,
    reason = "one validation decision crosses this checked-model boundary"
)]
fn modeled_publication(
    plan: RepairPlan,
    streams: Vec<StreamId>,
) -> Result<TaskValidationPublication, TaskCommandError> {
    let intent = ValidationIntent::model_builder().plan(plan).build();
    let command = ValidationCommand::model_builder()
        .plan(ValidationIntentPlan::apply(intent.as_ref()))
        .stream(ValidationIntentStream::apply(intent.as_ref()))
        .build();
    let events: Vec<ValidationFact> = CommandLogic::handle(&command, Modeled::default())
        .map_err(|_model_error| TaskCommandError::ModeledTaskValidationDecisionFailed)?
        .into();
    let [event]: [ValidationFact; 1] = events
        .try_into()
        .map_err(|_fact_count| TaskCommandError::InvalidModeledTaskValidationPublication)?;
    let view = ValidationView::model_builder()
        .emitted(ValidationViewEmitted::apply(&event))
        .link_changes(ValidationViewLinks::apply(&event))
        .order_change(ValidationViewOrder::apply(&event))
        .repairs(ValidationViewRepairs::apply(&event))
        .stream(ValidationViewStream::apply(&event))
        .build();
    if !view.as_ref().emitted {
        return Err(TaskCommandError::InvalidModeledTaskValidationPublication);
    }
    TaskValidationPublication::from_modeled_fact(
        TaskValidationRepaired::new(
            view.as_ref().stream.clone(),
            view.as_ref().link_changes.clone(),
            view.as_ref().order_change.clone(),
            view.as_ref().repairs.clone(),
        ),
        streams,
    )
}
