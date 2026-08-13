//! Read-side task-board projection for native Tiber Tasks.
//!
//! This crate folds the preserved `tiber.domain_event` task vocabulary into a
//! query model. It deliberately provides no task write model: future
//! business-domain `EventCore` commands will each own a narrow decision fold,
//! while this projection remains a separate read model.

#![forbid(unsafe_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use core::{error::Error, fmt};

use tiber_tasks_core::{
    Task, TaskAcceptanceAdded, TaskAcceptanceChecked, TaskAcceptanceRemoved, TaskCreated,
    TaskDetailsUpdated, TaskEvent, TaskId, TaskLinksChanged, TaskOrder, TaskPullRequestChanged,
    TaskStatus, TaskSubtaskAdded, TaskSubtaskChecked, TaskTransitioned,
};

/// An immutable, globally ordered set of durable task facts.
///
/// The source adapter must preserve commit order across all included streams.
/// Each [`TaskEvent`] carries its own stream identity, so this boundary does
/// not enumerate or assume task streams: preserved historical
/// `tiber:task:*` streams are accepted whenever their facts appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskHistory {
    /// Source facts retained in their global durable order.
    ordered_events: Vec<TaskEvent>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the history API presents construction before inspection, which follows the query boundary's data flow rather than alphabetical ordering"
)]
impl TaskHistory {
    /// Captures task facts in their globally durable order.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the constructed immutable history is clearest as the function's final expression"
    )]
    pub fn from_ordered_events<Events>(events: Events) -> Self
    where
        Events: IntoIterator<Item = TaskEvent>,
    {
        Self {
            ordered_events: events.into_iter().collect(),
        }
    }

    /// Returns the immutable globally ordered source facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed immutable fact slice is clearest as a final expression"
    )]
    pub fn events(&self) -> &[TaskEvent] {
        &self.ordered_events
    }
}

/// A normalized user task reference accepted by task queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReference(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the reference API documents parsing before inspection to keep the user-input boundary legible"
)]
impl TaskReference {
    /// Parses a task reference at a user-input boundary.
    ///
    /// # Errors
    ///
    /// Returns [`TaskProjectionError::InvalidTaskReference`] when the input is
    /// empty, path-like, control-containing, or names a Markdown file.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the successful normalized reference and predicate closures are idiomatic final expressions"
    )]
    pub fn parse(input: &str) -> Result<Self, TaskProjectionError> {
        let value = input.trim();
        if value.is_empty()
            || value
                .get(value.len().saturating_sub(3)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
            || value.contains(['/', '\\'])
            || value.chars().any(char::is_control)
        {
            return Err(TaskProjectionError::InvalidTaskReference);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the normalized reference text.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed normalized reference text is clearest as a final expression"
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable failures emitted while replaying or querying task history.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "the closed stable-code vocabulary is grouped by input, replay, and workflow failure semantics rather than alphabetically, and callers may exhaustively handle its durable cases"
)]
pub enum TaskProjectionError {
    /// A task reference did not meet the task-query input grammar.
    InvalidTaskReference,
    /// A user reference matched no current task.
    TaskReferenceMissing {
        /// The normalized unresolved reference.
        reference: TaskReference,
    },
    /// A user reference matched more than one current task.
    TaskReferenceAmbiguous {
        /// The normalized ambiguous reference.
        reference: TaskReference,
        /// Current task identities sharing that reference.
        matches: Vec<TaskId>,
    },
    /// A history repeated a task's creation fact.
    DuplicateTaskCreation {
        /// The duplicated task identity.
        task: TaskId,
    },
    /// A task mutation appeared before its task creation fact.
    TaskMissing {
        /// The unknown task identity.
        task: TaskId,
    },
    /// A subtask mutation referenced an absent subtask.
    SubtaskMissing {
        /// The owning task identity.
        task: TaskId,
        /// The absent subtask identity.
        subtask: String,
    },
    /// An acceptance mutation referenced an absent list position.
    AcceptanceItemMissing {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based absent list position.
        index: usize,
    },
    /// Historical facts contain more than one active task.
    MultipleActiveTasks {
        /// Active task identities in stable identity order.
        active_tasks: Vec<TaskId>,
    },
    /// A newer task-fact vocabulary variant is not modeled by this projection.
    UnsupportedTaskEvent,
}

impl TaskProjectionError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed borrowed error-code mapping is clearest as a concise final match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidTaskReference => "tasks_invalid_task_reference",
            Self::TaskReferenceMissing { .. } => "tasks_task_reference_missing",
            Self::TaskReferenceAmbiguous { .. } => "tasks_task_reference_ambiguous",
            Self::DuplicateTaskCreation { .. } => "tasks_projection_duplicate_task_creation",
            Self::TaskMissing { .. } => "tasks_projection_task_missing",
            Self::SubtaskMissing { .. } => "tasks_projection_subtask_missing",
            Self::AcceptanceItemMissing { .. } => "tasks_projection_acceptance_item_missing",
            Self::MultipleActiveTasks { .. } => "tasks_projection_multiple_active_tasks",
            Self::UnsupportedTaskEvent => "tasks_projection_unsupported_task_event",
        }
    }
}

impl fmt::Display for TaskProjectionError {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "formatting directly delegates to the stable code as its final expression"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "this leaf projection error has no cause, source, provider, or deprecated description beyond its stable display code"
)]
impl Error for TaskProjectionError {}

/// The complete query-side state folded from native task facts.
///
/// This is not an aggregate or a write authority. It contains broad task-board
/// state only because read queries need it; modeled task commands will fold
/// only the bounded facts required by their individual decisions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the projection fields follow replay state flow—initialization, task index, then board order—rather than alphabetical names"
)]
pub struct TaskBoardProjection {
    /// Whether the repository initialization fact has been observed.
    initialized: bool,
    /// Current task state indexed by durable identity.
    tasks: BTreeMap<TaskId, Task>,
    /// Strict open-board priority sequence from the latest order fact.
    priority_order: Vec<TaskId>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the public query methods and private fold helpers follow replay and query flow rather than alphabetical ordering"
)]
impl TaskBoardProjection {
    /// Replays one globally ordered history into a fresh query projection.
    ///
    /// # Errors
    ///
    /// Returns the first malformed-history error encountered while folding the
    /// supplied durable facts in their source order.
    #[expect(
        clippy::implicit_return,
        clippy::missing_inline_in_public_items,
        clippy::question_mark_used,
        reason = "replay is a nontrivial public fold where typed propagation and an expression final result are clearer than forced inlining or manual matches"
    )]
    pub fn replay(history: &TaskHistory) -> Result<Self, TaskProjectionError> {
        let mut projection = Self::default();
        for event in history.events() {
            projection.apply(event)?;
        }
        Ok(projection)
    }

    /// Folds one next durable fact into this query projection.
    ///
    /// Callers use this only with a source that preserves global fact order.
    /// It does not issue facts, make a task decision, or change the source
    /// event store.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the next fact is invalid relative to the
    /// state already folded into this read projection.
    #[expect(
        clippy::implicit_return,
        clippy::missing_inline_in_public_items,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the central ordered replay dispatcher needs typed propagation and borrowed event patterns; forcing inline expansion or manual matches would obscure its bounded read-model behavior"
    )]
    pub fn apply(&mut self, event: &TaskEvent) -> Result<(), TaskProjectionError> {
        match event {
            TaskEvent::RepositoryInitialized(_) => self.initialized = true,
            TaskEvent::TaskCreated(created) => self.apply_task_created(created)?,
            TaskEvent::TaskTransitioned(transitioned) => self.apply_transition(transitioned)?,
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                self.apply_order(order);
            }
            TaskEvent::TaskLinksChanged(changed) => self.apply_links_changed(changed)?,
            TaskEvent::TaskSubtaskAdded(added) => self.apply_subtask_added(added)?,
            TaskEvent::TaskSubtaskChecked(checked) => self.apply_subtask_checked(checked)?,
            TaskEvent::TaskDetailsUpdated(updated) => self.apply_details_updated(updated)?,
            TaskEvent::HistoricalTaskClaimChanged(changed) => {
                self.task_mut(&changed.stem)?
                    .claim
                    .clone_from(&changed.claim);
            }
            TaskEvent::TaskPullRequestChanged(changed) => {
                self.apply_pull_request_changed(changed)?;
            }
            TaskEvent::TaskAcceptanceAdded(added) => self.apply_acceptance_added(added)?,
            TaskEvent::TaskAcceptanceChecked(checked) => self.apply_acceptance_checked(checked)?,
            TaskEvent::TaskAcceptanceRemoved(removed) => self.apply_acceptance_removed(removed)?,
            TaskEvent::TaskNoteAdded(added) => {
                self.task_mut(&added.stem)?.notes.push(added.note.clone());
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                for changed in &repaired.link_changes {
                    self.apply_links_changed(changed)?;
                }
                if let Some(order) = &repaired.order_change {
                    self.apply_order(order);
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                for task_id in &closed.stems {
                    let task = self.task_mut(task_id)?;
                    task.status = TaskStatus::Done;
                    task.claim = None;
                }
                self.priority_order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) => {
                let task = self.task_mut(&closed.stem)?;
                task.status = TaskStatus::Done;
                task.claim = None;
            }
            TaskEvent::HistoricalTaskRemoved(removed) => {
                let _: Option<Task> = self.tasks.remove(&removed.stem);
            }
            // Historical publication notifications carry no query-side state.
            TaskEvent::HistoricalTaskStatePublished(_) => {}
            // `TaskEvent` is non-exhaustive so newer durable facts must never
            // silently produce an incomplete task-board view.
            _ => return Err(TaskProjectionError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Returns whether the repository initialization fact has been observed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the initialization predicate is clearest as a final field expression"
    )]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns all current tasks in stable task-identity order.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the ordered map iterator is clearest as a final borrowed expression"
    )]
    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Returns one current task by its durable identity.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the map lookup is clearest as a final borrowed expression"
    )]
    pub fn task(&self, task_id: &TaskId) -> Option<&Task> {
        self.tasks.get(task_id)
    }

    /// Returns the durable board priority sequence, including any stale IDs
    /// retained in historical facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable priority slice is clearest as a final borrowed expression"
    )]
    pub fn priority_order(&self) -> &[TaskId] {
        &self.priority_order
    }

    /// Returns current open tasks in strict board priority order.
    ///
    /// A stale historical board ID or a task later moved to a terminal state is
    /// omitted from this query view rather than being turned into an open task.
    /// Future modeled validation commands own repair of durable board-order
    /// inconsistency.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the ordered query pipeline and its predicates are clearer as expression returns"
    )]
    pub fn ordered_tasks(&self) -> Vec<&Task> {
        self.priority_order
            .iter()
            .filter_map(|task_id| self.tasks.get(task_id))
            .filter(|task| matches!(task.status, TaskStatus::Backlog | TaskStatus::InProgress))
            .collect()
    }

    /// Returns all current active tasks in stable identity order.
    ///
    /// The projection reports rather than hides multiple historical active
    /// tasks. Future task-transition commands will be their authority fence.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable active-task query pipeline is clearer as an expression return"
    )]
    pub fn active_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|task| task.status == TaskStatus::InProgress)
            .collect()
    }

    /// Returns the first backlog task in board order whose blockers are all
    /// known and done.
    ///
    /// It intentionally does not select an active task or treat a missing
    /// blocker as done. The caller can separately inspect [`Self::active_tasks`]
    /// to apply the one-active-task workflow policy.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the prioritized eligibility query and its predicates are clearer as expression returns"
    )]
    pub fn next_eligible_task(&self) -> Option<&Task> {
        self.priority_order
            .iter()
            .filter_map(|task_id| self.tasks.get(task_id))
            .find(|task| task.status == TaskStatus::Backlog && self.blockers_are_done(task))
    }

    /// Returns the task to continue under the one-active-task workflow policy.
    ///
    /// A sole in-progress task always takes precedence over a new eligible
    /// backlog task. Corrupt historical state with multiple active tasks is
    /// surfaced as a stable typed error rather than hidden by selection.
    ///
    /// # Errors
    ///
    /// Returns [`TaskProjectionError::MultipleActiveTasks`] when history has
    /// more than one active task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed active-task selection remains clearest as a borrowed slice match"
    )]
    pub fn next_actionable_task(&self) -> Result<Option<&Task>, TaskProjectionError> {
        let active_tasks = self.active_tasks();
        match active_tasks.as_slice() {
            [] => Ok(self.next_eligible_task()),
            [task] => Ok(Some(*task)),
            _ => Err(TaskProjectionError::MultipleActiveTasks {
                active_tasks: active_tasks
                    .into_iter()
                    .map(|task| task.stem.clone())
                    .collect(),
            }),
        }
    }

    /// Resolves a normalized task reference to one durable task identity.
    ///
    /// # Errors
    ///
    /// Returns a stable missing or ambiguity error when the normalized
    /// reference does not identify exactly one current task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the exact-match query keeps its predicate pipeline and borrowed slice match explicit"
    )]
    pub fn resolve_task_reference(
        &self,
        reference: &TaskReference,
    ) -> Result<TaskId, TaskProjectionError> {
        let matches = self
            .tasks
            .keys()
            .filter(|task_id| task_reference_matches(task_id.as_str(), reference.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => Ok(task_id.clone()),
            [] => Err(TaskProjectionError::TaskReferenceMissing {
                reference: reference.clone(),
            }),
            _ => Err(TaskProjectionError::TaskReferenceAmbiguous {
                reference: reference.clone(),
                matches,
            }),
        }
    }

    /// Parses and resolves a user-provided full stem, short ID, or nickname.
    ///
    /// # Errors
    ///
    /// Returns a parsing, missing, or ambiguity error when the input does not
    /// identify exactly one current task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the composed input boundary keeps its typed parse propagation explicit"
    )]
    pub fn resolve_task_ref(&self, input: &str) -> Result<TaskId, TaskProjectionError> {
        let reference = TaskReference::parse(input)?;
        self.resolve_task_reference(&reference)
    }

    /// Folds a creation fact after rejecting a duplicate durable identity.
    #[expect(
        clippy::implicit_return,
        reason = "the successful duplicate-checked insertion ends with the idiomatic unit result"
    )]
    fn apply_task_created(&mut self, created: &TaskCreated) -> Result<(), TaskProjectionError> {
        let task = (*created.task).clone();
        let task_id = task.stem.clone();
        if self.tasks.contains_key(&task_id) {
            return Err(TaskProjectionError::DuplicateTaskCreation { task: task_id });
        }
        let _: Option<Task> = self.tasks.insert(task_id, task);
        Ok(())
    }

    /// Folds a lifecycle/claim transition into an existing task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_transition(
        &mut self,
        transitioned: &TaskTransitioned,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&transitioned.stem)?;
        task.status = transitioned.status;
        task.claim.clone_from(&transitioned.claim);
        Ok(())
    }

    /// Replaces the query-side strict priority sequence.
    fn apply_order(&mut self, order: &TaskOrder) {
        self.priority_order.clone_from(&order.order);
    }

    /// Replaces one task's dependency-link fields.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_links_changed(
        &mut self,
        changed: &TaskLinksChanged,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&changed.stem)?;
        task.blocks.clone_from(&changed.blocks);
        task.blocked_by.clone_from(&changed.blocked_by);
        Ok(())
    }

    /// Appends a durable subtask to its owning task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_subtask_added(&mut self, added: &TaskSubtaskAdded) -> Result<(), TaskProjectionError> {
        self.task_mut(&added.stem)?
            .subtasks
            .push(added.subtask.clone());
        Ok(())
    }

    /// Changes the check state of an already-created subtask.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and compact option-to-error conversion"
    )]
    fn apply_subtask_checked(
        &mut self,
        checked: &TaskSubtaskChecked,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&checked.stem)?;
        let subtask = task
            .subtasks
            .iter_mut()
            .find(|subtask| subtask.id == checked.subtask_id)
            .ok_or_else(|| TaskProjectionError::SubtaskMissing {
                task: checked.stem.clone(),
                subtask: checked.subtask_id.clone(),
            })?;
        subtask.checked = checked.checked;
        Ok(())
    }

    /// Replaces mutable display and rationale fields for one task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_details_updated(
        &mut self,
        updated: &TaskDetailsUpdated,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&updated.stem)?;
        task.title.clone_from(&updated.title);
        task.tags.clone_from(&updated.tags);
        task.summary.clone_from(&updated.summary);
        task.context.clone_from(&updated.context);
        Ok(())
    }

    /// Replaces the pull-request metadata for one task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_pull_request_changed(
        &mut self,
        changed: &TaskPullRequestChanged,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&changed.stem)?;
        task.pr_mr_url.clone_from(&changed.url);
        task.pr_mr_status.clone_from(&changed.status);
        Ok(())
    }

    /// Appends a durable acceptance criterion to its owning task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_acceptance_added(
        &mut self,
        added: &TaskAcceptanceAdded,
    ) -> Result<(), TaskProjectionError> {
        self.task_mut(&added.stem)?
            .acceptance
            .push(added.item.clone());
        Ok(())
    }

    /// Changes the check state of one already-created acceptance criterion.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and compact option-to-error conversion"
    )]
    fn apply_acceptance_checked(
        &mut self,
        checked: &TaskAcceptanceChecked,
    ) -> Result<(), TaskProjectionError> {
        let item = self
            .task_mut(&checked.stem)?
            .acceptance
            .get_mut(checked.index)
            .ok_or_else(|| TaskProjectionError::AcceptanceItemMissing {
                task: checked.stem.clone(),
                index: checked.index,
            })?;
        item.checked = checked.checked;
        Ok(())
    }

    /// Removes one already-created acceptance criterion by durable position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_acceptance_removed(
        &mut self,
        removed: &TaskAcceptanceRemoved,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&removed.stem)?;
        if removed.index >= task.acceptance.len() {
            return Err(TaskProjectionError::AcceptanceItemMissing {
                task: removed.stem.clone(),
                index: removed.index,
            });
        }
        let _: tiber_tasks_core::ChecklistItem = task.acceptance.remove(removed.index);
        Ok(())
    }

    /// Borrows an existing task or reports the malformed historical sequence.
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed lookup and typed missing-task conversion are clearest as expressions"
    )]
    fn task_mut(&mut self, task_id: &TaskId) -> Result<&mut Task, TaskProjectionError> {
        self.tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskProjectionError::TaskMissing {
                task: task_id.clone(),
            })
    }

    /// Reports whether every dependency is currently present and done.
    #[expect(
        clippy::implicit_return,
        reason = "the all-blockers predicate is clearest as a final iterator expression"
    )]
    fn blockers_are_done(&self, task: &Task) -> bool {
        task.blocked_by.iter().all(|blocker_id| {
            self.tasks
                .get(blocker_id)
                .is_some_and(|blocker| blocker.status == TaskStatus::Done)
        })
    }
}

/// Reports whether a full stem supplies the requested full, short, or nickname reference.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "this named matcher keeps the three accepted reference grammars isolated from the projection query pipeline"
)]
fn task_reference_matches(task_id: &str, reference: &str) -> bool {
    if task_id == reference {
        return true;
    }
    let Some((date, after_date)) = task_id.split_once('-') else {
        return false;
    };
    let Some((code, nickname)) = after_date.split_once('-') else {
        return false;
    };
    format!("{date}-{code}") == reference || nickname == reference
}
