//! Native task-domain facts for Tiber.
//!
//! This crate owns the serializable `tiber.domain_event` vocabulary needed to
//! replay the retained Tiber task-history branch. It deliberately contains no
//! CLI, MCP, dashboard, Git-store, or aggregate surface. Future writes are
//! modeled as checked `EventCore` business commands with command-specific folds;
//! query-side projections remain outside this authority boundary.

#![forbid(unsafe_code)]
#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    clippy::implicit_return,
    clippy::missing_errors_doc,
    clippy::missing_inline_in_public_items,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    reason = "durable serde facts need stable public shapes and source order, and semantic parsing returns typed failures"
)]

use core::{error::Error, fmt};

use eventcore_types::{Event, StreamId};
use serde::{Deserialize, Serialize, de::Error as _};

/// Stable failures produced while parsing task-domain semantic values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCoreError {
    /// A task title contained no non-whitespace content.
    EmptyTaskTitle,
    /// A task identifier contained no non-whitespace content.
    EmptyTaskId,
    /// A task identifier contained a control character.
    InvalidTaskId,
    /// A task title contained a control character.
    InvalidTaskTitle,
}

impl TaskCoreError {
    /// Returns the stable external error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EmptyTaskId => "tasks_empty_task_id",
            Self::InvalidTaskId => "tasks_invalid_task_id",
            Self::EmptyTaskTitle => "tasks_empty_task_title",
            Self::InvalidTaskTitle => "tasks_invalid_task_title",
        }
    }
}

impl fmt::Display for TaskCoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "this leaf semantic parsing error deliberately exposes only its stable display code"
)]
impl Error for TaskCoreError {}

/// Collision-resistant durable task identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    /// Parses one task identity at the I/O boundary.
    pub fn parse(input: &str) -> Result<Self, TaskCoreError> {
        let value = input.trim();
        if value.is_empty() {
            return Err(TaskCoreError::EmptyTaskId);
        }
        if value.chars().any(char::is_control) {
            return Err(TaskCoreError::InvalidTaskId);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical durable task identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the serde boundary delegates construction to the semantic parser and has no distinct in-place representation"
)]
impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        Self::parse(&input).map_err(D::Error::custom)
    }
}

/// Human-readable task title with normalized whitespace at its boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TaskTitle(String);

impl TaskTitle {
    /// Parses one task title at the I/O boundary.
    pub fn parse(input: &str) -> Result<Self, TaskCoreError> {
        let value = input.trim();
        if value.is_empty() {
            return Err(TaskCoreError::EmptyTaskTitle);
        }
        if value.chars().any(char::is_control) {
            return Err(TaskCoreError::InvalidTaskTitle);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the normalized title.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Produces the readable stem component used when an adapter assigns IDs.
    #[must_use]
    pub fn file_stem(&self) -> String {
        let mut slug = String::new();
        let mut previous_was_separator = true;

        for character in self.0.chars().flat_map(char::to_lowercase) {
            match (character.is_ascii_alphanumeric(), previous_was_separator) {
                (true, _) => {
                    slug.push(character);
                    previous_was_separator = false;
                }
                (false, false) => {
                    slug.push('-');
                    previous_was_separator = true;
                }
                (false, true) => {}
            }
        }

        if slug.ends_with('-') {
            let _: Option<char> = slug.pop();
        }

        if slug.is_empty() {
            "task".to_owned()
        } else {
            slug
        }
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the serde boundary delegates construction to the semantic parser and has no distinct in-place representation"
)]
impl<'de> Deserialize<'de> for TaskTitle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        Self::parse(&input).map_err(D::Error::custom)
    }
}

/// The only durable task lifecycle states in the retained task history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    /// Work is queued in strict backlog order.
    Backlog,
    /// One task is active for implementation.
    InProgress,
    /// Work completed its required delivery lifecycle.
    Done,
    /// Work was deliberately declined or superseded.
    Abandoned,
}

/// The agent/session claim attached to an active task.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskClaim {
    /// Host that owns the claim.
    pub host: String,
    /// Tiber session that owns the claim.
    pub session: String,
}

/// One user-visible acceptance criterion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ChecklistItem {
    /// Whether the criterion has been satisfied.
    pub checked: bool,
    /// Human-readable criterion.
    pub text: String,
}

/// A named implementation subtask and its prerequisites.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Subtask {
    /// Stable subtask identifier within its task.
    pub id: String,
    /// Whether the subtask has been satisfied.
    pub checked: bool,
    /// Human-readable implementation step.
    pub title: String,
    /// Prerequisite subtask identifiers.
    pub after: Vec<String>,
}

/// An immutable task-history note.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskNote {
    /// Recorded time as persisted by the task service.
    pub date: String,
    /// Note content.
    pub text: String,
}

/// One deterministic repair fact emitted by task validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "repair", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValidationRepair {
    /// A missing reciprocal task link was added.
    ReciprocalLinkAdded {
        /// Task receiving the repaired link.
        task: TaskId,
        /// Link field that was repaired.
        field: String,
        /// Linked task identity.
        target: TaskId,
    },
    /// A missing task was added to the board order.
    BoardEntryAdded {
        /// Added task identity.
        task: TaskId,
    },
    /// A stale task was removed from the board order.
    BoardEntryRemoved {
        /// Removed task identity.
        task: TaskId,
    },
}

/// Authoritative durable state established by task facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Task {
    /// Stable task identity.
    pub stem: TaskId,
    /// Current lifecycle state.
    pub status: TaskStatus,
    /// User-facing title.
    pub title: TaskTitle,
    /// Prerequisite task identities.
    pub blocked_by: Vec<TaskId>,
    /// Task identities this task blocks.
    pub blocks: Vec<TaskId>,
    /// User-supplied classification tags.
    pub tags: Vec<String>,
    /// Associated pull-request or merge-request URL.
    pub pr_mr_url: Option<String>,
    /// Associated pull-request or merge-request status.
    pub pr_mr_status: Option<String>,
    /// Current active-task claim.
    pub claim: Option<TaskClaim>,
    /// Concise work summary.
    pub summary: String,
    /// Decision context and rationale.
    pub context: String,
    /// Acceptance criteria.
    pub acceptance: Vec<ChecklistItem>,
    /// Implementation subtasks.
    pub subtasks: Vec<Subtask>,
    /// Immutable task-history notes.
    pub notes: Vec<TaskNote>,
    /// Original durable creation timestamp.
    pub committed_at: String,
}

impl Task {
    /// Creates the canonical empty backlog state for one newly admitted task.
    #[must_use]
    pub fn new_backlog(stem: TaskId, title: TaskTitle, committed_at: String) -> Self {
        Self {
            stem,
            status: TaskStatus::Backlog,
            title,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            tags: Vec::new(),
            pr_mr_url: None,
            pr_mr_status: None,
            claim: None,
            summary: String::new(),
            context: String::new(),
            acceptance: Vec::new(),
            subtasks: Vec::new(),
            notes: Vec::new(),
            committed_at,
        }
    }
}

/// Durable source-event payload for repository initialization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RepositoryInitialized {
    /// Stream receiving the initialization fact.
    pub stream_id: StreamId,
}

/// Durable source-event payload for task creation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskCreated {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Complete initial task state.
    pub task: Box<Task>,
}

impl TaskCreated {
    /// Creates one named task-creation fact payload.
    #[must_use]
    pub fn new(stream_id: StreamId, task: Task) -> Self {
        Self {
            stream_id,
            task: Box::new(task),
        }
    }
}

/// Durable source-event payload for a lifecycle transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskTransitioned {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task whose state changed.
    pub stem: TaskId,
    /// New lifecycle state.
    pub status: TaskStatus,
    /// Claim associated with the state.
    pub claim: Option<TaskClaim>,
}

impl TaskTransitioned {
    /// Creates one named lifecycle transition fact payload.
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        stem: TaskId,
        status: TaskStatus,
        claim: Option<TaskClaim>,
    ) -> Self {
        Self {
            stream_id,
            stem,
            status,
            claim,
        }
    }
}

/// Durable source-event payload for a strict board ordering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskOrder {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Complete strict order of open tasks.
    pub order: Vec<TaskId>,
}

impl TaskOrder {
    /// Creates one named complete strict board-order fact payload.
    #[must_use]
    pub fn new(stream_id: StreamId, order: Vec<TaskId>) -> Self {
        Self { stream_id, order }
    }
}

/// Durable source-event payload for task dependency links.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskLinksChanged {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task whose links changed.
    pub stem: TaskId,
    /// Tasks blocked by this task.
    pub blocks: Vec<TaskId>,
    /// Tasks that block this task.
    pub blocked_by: Vec<TaskId>,
}

/// Durable source-event payload for adding one subtask.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskSubtaskAdded {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task receiving the subtask.
    pub stem: TaskId,
    /// Added subtask.
    pub subtask: Subtask,
}

/// Durable source-event payload for checking one subtask.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskSubtaskChecked {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task owning the subtask.
    pub stem: TaskId,
    /// Subtask identifier.
    pub subtask_id: String,
    /// Checked state.
    pub checked: bool,
}

/// Durable source-event payload for checking one exact subtask occurrence.
///
/// Unlike the retained identifier-only [`TaskSubtaskChecked`] fact, this fact
/// carries the complete unchecked preimage and its durable list position. It
/// therefore remains unambiguous when legacy history contains duplicate
/// subtask identifiers. The resulting check state is always `true`; callers
/// cannot use this vocabulary to clear a subtask.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskSubtaskOccurrenceChecked {
    /// Stream receiving the exact occurrence check fact.
    pub stream_id: StreamId,
    /// Task owning the checked subtask occurrence.
    pub stem: TaskId,
    /// Zero-based immutable occurrence position.
    pub index: usize,
    /// Exact current unchecked occurrence required before applying the check.
    pub expected: Subtask,
}

impl TaskSubtaskOccurrenceChecked {
    /// Creates one named, preconditioned exact subtask-occurrence check.
    #[must_use]
    pub fn new(stream_id: StreamId, stem: TaskId, index: usize, expected: Subtask) -> Self {
        Self {
            stream_id,
            stem,
            index,
            expected,
        }
    }
}

/// Durable source-event payload for correcting one malformed legacy subtask identity.
///
/// The insertion position and complete preimage deliberately make this a
/// one-occurrence correction rather than another ambiguous mutation by ID.
/// Existing task history remains immutable; this fact is appended only after
/// the command has fenced the board and task streams at one authority revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskSubtaskIdCorrected {
    /// Stream receiving the correction fact.
    pub stream_id: StreamId,
    /// Task owning the corrected subtask occurrence.
    pub stem: TaskId,
    /// Zero-based immutable insertion position of the corrected occurrence.
    pub index: usize,
    /// Exact current occurrence required before applying the correction.
    pub expected: Subtask,
    /// Replacement identity for the one corrected occurrence.
    pub replacement_id: String,
}

impl TaskSubtaskIdCorrected {
    /// Creates one named, preconditioned duplicate-subtask identity correction.
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        stem: TaskId,
        index: usize,
        expected: Subtask,
        replacement_id: String,
    ) -> Self {
        Self {
            stream_id,
            stem,
            index,
            expected,
            replacement_id,
        }
    }
}

/// Durable source-event payload for editable task details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskDetailsUpdated {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task whose details changed.
    pub stem: TaskId,
    /// Replacement title.
    pub title: TaskTitle,
    /// Replacement tags.
    pub tags: Vec<String>,
    /// Replacement summary.
    pub summary: String,
    /// Replacement decision context.
    pub context: String,
}

impl TaskDetailsUpdated {
    /// Creates one named complete task-details replacement fact.
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        stem: TaskId,
        title: TaskTitle,
        tags: Vec<String>,
        summary: String,
        context: String,
    ) -> Self {
        Self {
            stream_id,
            stem,
            title,
            tags,
            summary,
            context,
        }
    }
}

/// Historical source-event payload for a claim-only update.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HistoricalTaskClaimChanged {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task whose claim changed.
    pub stem: TaskId,
    /// Replacement claim.
    pub claim: Option<TaskClaim>,
}

/// Durable source-event payload for pull-request metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskPullRequestChanged {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task whose delivery metadata changed.
    pub stem: TaskId,
    /// Associated PR/MR URL.
    pub url: Option<String>,
    /// Associated PR/MR status.
    pub status: Option<String>,
}

/// Durable source-event payload for adding an acceptance item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskAcceptanceAdded {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task receiving the criterion.
    pub stem: TaskId,
    /// Added criterion.
    pub item: ChecklistItem,
}

/// Durable source-event payload for checking an acceptance item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskAcceptanceChecked {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task owning the criterion.
    pub stem: TaskId,
    /// Zero-based acceptance-item position.
    pub index: usize,
    /// Checked state.
    pub checked: bool,
}

impl TaskAcceptanceChecked {
    /// Creates one named acceptance-check fact payload.
    #[must_use]
    pub fn new(stream_id: StreamId, stem: TaskId, index: usize, checked: bool) -> Self {
        Self {
            stream_id,
            stem,
            index,
            checked,
        }
    }
}

/// Durable source-event payload for removing an acceptance item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskAcceptanceRemoved {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task owning the criterion.
    pub stem: TaskId,
    /// Zero-based acceptance-item position.
    pub index: usize,
}

/// Durable source-event payload for recording one task note.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskNoteAdded {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task receiving the note.
    pub stem: TaskId,
    /// Immutable note.
    pub note: TaskNote,
}

/// Durable source-event payload for task-board validation repairs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TaskValidationRepaired {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Reciprocal-link facts emitted by the repair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_changes: Vec<TaskLinksChanged>,
    /// Board-order fact emitted by the repair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_change: Option<TaskOrder>,
    /// Deterministic repair observations.
    pub repairs: Vec<ValidationRepair>,
}

/// Durable source-event payload for closing tasks from an accepted commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TasksClosedFromCommitTrailers {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Tasks closed by the commit intent.
    pub stems: Vec<TaskId>,
    /// Resulting strict order of open tasks.
    pub order: Vec<TaskId>,
}

/// Historical source-event payload for singular closure/removal facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HistoricalTaskStem {
    /// Stream receiving the fact.
    pub stream_id: StreamId,
    /// Task identified by the historical fact.
    pub stem: TaskId,
}

/// Historical source-event payload for a projection-notification fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HistoricalTaskStatePublished {
    /// Stream receiving the historical notification.
    pub stream_id: StreamId,
}

/// The retained task-history wire vocabulary.
///
/// The serde tag and [`Event::event_type_name`] are intentionally stable: the
/// Git `EventCore` store uses them to recover Tiber's existing task history. New
/// commands must emit named business facts and must not emit historical-only
/// variants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskEvent {
    /// The task repository was initialized.
    RepositoryInitialized(RepositoryInitialized),
    /// A task was created.
    TaskCreated(TaskCreated),
    /// A task lifecycle state changed.
    TaskTransitioned(TaskTransitioned),
    /// Strict backlog priority changed.
    TaskPriorityChanged(TaskOrder),
    /// Task dependency links changed.
    TaskLinksChanged(TaskLinksChanged),
    /// A subtask was added.
    TaskSubtaskAdded(TaskSubtaskAdded),
    /// A subtask check state changed.
    TaskSubtaskChecked(TaskSubtaskChecked),
    /// One exact subtask occurrence was checked.
    TaskSubtaskOccurrenceChecked(TaskSubtaskOccurrenceChecked),
    /// A malformed legacy subtask identity was corrected at one exact occurrence.
    TaskSubtaskIdCorrected(TaskSubtaskIdCorrected),
    /// Editable task details changed.
    TaskDetailsUpdated(TaskDetailsUpdated),
    /// Historical claim-only fact retained for event replay.
    #[serde(rename = "task_claim_changed")]
    HistoricalTaskClaimChanged(HistoricalTaskClaimChanged),
    /// Pull-request or merge-request metadata changed.
    TaskPullRequestChanged(TaskPullRequestChanged),
    /// An acceptance criterion was added.
    TaskAcceptanceAdded(TaskAcceptanceAdded),
    /// An acceptance criterion check state changed.
    TaskAcceptanceChecked(TaskAcceptanceChecked),
    /// An acceptance criterion was removed.
    TaskAcceptanceRemoved(TaskAcceptanceRemoved),
    /// A task note was recorded.
    TaskNoteAdded(TaskNoteAdded),
    /// Validation emitted deterministic repair facts.
    TaskValidationRepaired(TaskValidationRepaired),
    /// Tasks were closed from an accepted commit intent.
    TasksClosedFromCommitTrailers(TasksClosedFromCommitTrailers),
    /// Historical singular closure fact retained for event replay.
    #[serde(rename = "task_closed_from_trailer")]
    HistoricalTaskClosedFromTrailer(HistoricalTaskStem),
    /// Historical task-removal fact retained for event replay.
    #[serde(rename = "task_removed")]
    HistoricalTaskRemoved(HistoricalTaskStem),
    /// The complete board order changed.
    BoardReordered(TaskOrder),
    /// Historical projection notification retained for event replay only.
    #[serde(rename = "task_state_published")]
    HistoricalTaskStatePublished(HistoricalTaskStatePublished),
}

impl TaskEvent {
    /// Returns the stream selected by the durable event fact.
    #[must_use]
    pub const fn stream_id_value(&self) -> &StreamId {
        match self {
            Self::RepositoryInitialized(RepositoryInitialized { stream_id })
            | Self::TaskCreated(TaskCreated { stream_id, .. })
            | Self::TaskTransitioned(TaskTransitioned { stream_id, .. })
            | Self::TaskPriorityChanged(TaskOrder { stream_id, .. })
            | Self::TaskLinksChanged(TaskLinksChanged { stream_id, .. })
            | Self::TaskSubtaskAdded(TaskSubtaskAdded { stream_id, .. })
            | Self::TaskSubtaskChecked(TaskSubtaskChecked { stream_id, .. })
            | Self::TaskSubtaskOccurrenceChecked(TaskSubtaskOccurrenceChecked {
                stream_id, ..
            })
            | Self::TaskSubtaskIdCorrected(TaskSubtaskIdCorrected { stream_id, .. })
            | Self::TaskDetailsUpdated(TaskDetailsUpdated { stream_id, .. })
            | Self::HistoricalTaskClaimChanged(HistoricalTaskClaimChanged { stream_id, .. })
            | Self::TaskPullRequestChanged(TaskPullRequestChanged { stream_id, .. })
            | Self::TaskAcceptanceAdded(TaskAcceptanceAdded { stream_id, .. })
            | Self::TaskAcceptanceChecked(TaskAcceptanceChecked { stream_id, .. })
            | Self::TaskAcceptanceRemoved(TaskAcceptanceRemoved { stream_id, .. })
            | Self::TaskNoteAdded(TaskNoteAdded { stream_id, .. })
            | Self::TaskValidationRepaired(TaskValidationRepaired { stream_id, .. })
            | Self::TasksClosedFromCommitTrailers(TasksClosedFromCommitTrailers {
                stream_id,
                ..
            })
            | Self::HistoricalTaskClosedFromTrailer(HistoricalTaskStem { stream_id, .. })
            | Self::HistoricalTaskRemoved(HistoricalTaskStem { stream_id, .. })
            | Self::BoardReordered(TaskOrder { stream_id, .. })
            | Self::HistoricalTaskStatePublished(HistoricalTaskStatePublished { stream_id }) => {
                stream_id
            }
        }
    }
}

impl Event for TaskEvent {
    fn stream_id(&self) -> &StreamId {
        self.stream_id_value()
    }

    fn event_type_name() -> &'static str {
        "tiber.domain_event"
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskCoreError, TaskTitle};

    #[test]
    fn task_titles_normalize_and_reject_invalid_boundary_values() {
        assert_eq!(
            TaskTitle::parse(" Define Tiber: v1! ").map(|title| title.file_stem()),
            Ok("define-tiber-v1".to_owned())
        );
        assert_eq!(TaskTitle::parse(" \n "), Err(TaskCoreError::EmptyTaskTitle));
        assert_eq!(
            TaskTitle::parse("Tiber\u{0000}"),
            Err(TaskCoreError::InvalidTaskTitle)
        );
    }
}
