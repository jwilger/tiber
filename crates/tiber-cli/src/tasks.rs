//! Native Tiber Tasks command adapter.
//!
//! This module owns CLI parsing and rendering only. The Git-backed store owns
//! source selection, while the task service owns the immutable history replay.

use core::{error::Error, fmt};
use std::{ffi::OsString, io, path::Path};

use chrono::{SecondsFormat, Utc};
use eventcore_types::{BatchSize, StreamPattern};
use tiber_store_git::publication::{TiberEventPublisher, TiberPublicationError};
use tiber_store_git::{
    GitStoreError, TiberEventStore, TiberRevision, TransactionEventPage, TransactionHistoryError,
};
use tiber_tasks_core::{Subtask, Task, TaskCoreError, TaskEvent, TaskId, TaskStatus, TaskTitle};
use tiber_tasks_service::command::{
    AbandonTask, AcceptanceIndex, AddAcceptance, CheckAcceptance, CheckSubtaskOccurrence,
    CompleteTask, CreateTask, LinkBlockedBy, PrioritizeTask, RepairDuplicateSubtaskId, StartTask,
    SubtaskOccurrence, SubtaskReplacementId, TaskCommandError, TaskCreationDecision,
    UpdateTaskDetails, decide_abandon_task, decide_add_acceptance, decide_check_acceptance,
    decide_check_subtask_occurrence, decide_complete_task, decide_create_task,
    decide_link_blocked_by, decide_prioritize_task, decide_repair_duplicate_subtask_id,
    decide_start_task, decide_update_task_details,
};
use tiber_tasks_service::{TaskBoardProjection, TaskHistory, TaskProjectionError, TaskReference};
use tokio::runtime::Builder as RuntimeBuilder;
use uuid::Uuid;

/// Maximum number of durable facts fetched by one explicit `EventCore` page.
const TASK_HISTORY_PAGE_SIZE: usize = 64;

/// Complete grammar accepted after the `tiber tasks` command prefix.
#[expect(
    clippy::pub_with_shorthand,
    reason = "the parent command shell consumes the one canonical task grammar without widening it to the crate"
)]
pub(super) const TASKS_COMMAND_GRAMMAR: &str = "create [--id <stable-prefix>] <title> | update <ref> --title <title> --summary <summary> --context <context> | prioritize <ref> --before <ref> | link blocked-by <ref> <blocker-ref> | list [--status <backlog|in-progress|done|abandoned>] | show <ref> | search <query> | next | start <ref> | acceptance add <ref> <criterion> | acceptance check <ref> <one-based-index> | subtask check <ref> <one-based-occurrence> | subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id> | transition <ref> <done|abandoned>";

/// The exact durable stream patterns that comprise native Tiber Tasks history.
///
/// The preserved task history uses a closed union rather than every legacy
/// `tiber.domain_event` stream: repository initialization, board ordering, and
/// task facts. Other legacy workflow facts can share the event-type string and
/// must not be decoded as task facts.
const TASK_HISTORY_STREAM_PATTERNS: [&str; 3] = ["tiber:repository", "tiber:board", "tiber:task:*"];

#[expect(
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    reason = "the imperative CLI boundary uses typed railway propagation and direct-parent visibility to avoid widening parsing and projection failures"
)]
/// Parses one native `tiber tasks` invocation before repository state is accessed.
pub(super) fn parse(
    arguments: impl Iterator<Item = OsString>,
) -> Result<TaskCommand, TaskCliError> {
    let arguments = parse_arguments(arguments)?;
    let Some((subcommand, remaining)) = arguments.split_first() else {
        return Err(TaskCliError::MissingSubcommand);
    };
    parse_command(subcommand, remaining)
}

/// Executes one parsed native `tiber tasks` invocation against the current repository.
#[expect(
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    reason = "the imperative execution boundary uses typed propagation and direct-parent visibility after parsing completes"
)]
pub(super) fn run(repository: &Path, command: TaskCommand) -> Result<String, TaskCliError> {
    match command {
        TaskCommand::Help => Ok(help_output()),
        TaskCommand::Create { id_prefix, title } => create_task(repository, id_prefix, title),
        TaskCommand::Update {
            reference,
            title,
            summary,
            context,
        } => update_task(repository, &reference, title, summary, context),
        TaskCommand::AcceptanceCheck { reference, index } => {
            check_acceptance(repository, &reference, index)
        }
        TaskCommand::AcceptanceAdd {
            reference,
            criterion,
        } => add_acceptance(repository, &reference, criterion),
        TaskCommand::LinkBlockedBy { reference, blocker } => {
            link_blocked_by(repository, &reference, &blocker)
        }
        TaskCommand::Prioritize { reference, before } => {
            prioritize_task(repository, &reference, &before)
        }
        TaskCommand::SubtaskRepairDuplicate {
            reference,
            occurrence,
            replacement_id,
        } => repair_duplicate_subtask(repository, &reference, occurrence, replacement_id),
        TaskCommand::SubtaskCheck {
            reference,
            occurrence,
        } => check_subtask_occurrence(repository, &reference, occurrence),
        TaskCommand::Start { reference } => start_task(repository, &reference),
        TaskCommand::TransitionDone { reference } => complete_task(repository, &reference),
        TaskCommand::TransitionAbandoned { reference } => abandon_task(repository, &reference),
        TaskCommand::List(status) => {
            let projection = load_projection(repository)?;
            Ok(list_tasks(&projection, status))
        }
        TaskCommand::Show(reference) => {
            let projection = load_projection(repository)?;
            show_task(&projection, &reference)
        }
        TaskCommand::Search(query) => {
            let projection = load_projection(repository)?;
            Ok(search_tasks(&projection, &query))
        }
        TaskCommand::Next => {
            let projection = load_projection(repository)?;
            next_task(&projection)
        }
    }
}

/// A completely parsed native task operation.
#[expect(
    clippy::pub_with_shorthand,
    reason = "the parent command shell owns repository acquisition after this module's parse boundary"
)]
pub(super) enum TaskCommand {
    /// Renders the supported native task grammar without accessing repository state.
    Help,
    /// Creates one new backlog task through the native signed authority.
    Create {
        /// Stable retry identity supplied by the caller when reconciling creation.
        id_prefix: Option<TaskId>,
        /// Parsed owner-facing task title.
        title: TaskTitle,
    },
    /// Replaces the title, summary, and context of one existing task.
    Update {
        /// Parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// Exact replacement title.
        title: TaskTitle,
        /// Exact replacement summary.
        summary: String,
        /// Exact replacement decision context.
        context: String,
    },
    /// Activates one strict-next eligible backlog task through the signed native write boundary.
    Start {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
    },
    /// Sets one current acceptance item checked through the signed native write boundary.
    AcceptanceCheck {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// The parsed human-facing one-based acceptance position.
        index: AcceptanceIndex,
    },
    /// Appends one unchecked acceptance criterion through the signed native write boundary.
    AcceptanceAdd {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// Exact owner-supplied criterion.
        criterion: String,
    },
    /// Establishes one reciprocal blocked-by dependency through the signed native write boundary.
    LinkBlockedBy {
        /// The task that becomes blocked.
        reference: TaskReference,
        /// The task that blocks the target.
        blocker: TaskReference,
    },
    /// Moves one open task immediately before another in strict board order.
    Prioritize {
        /// Task to move.
        reference: TaskReference,
        /// Task that must immediately follow the moved task.
        before: TaskReference,
    },
    /// Corrects one exact malformed duplicate subtask identity through the signed native write boundary.
    SubtaskRepairDuplicate {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// The parsed human-facing one-based immutable subtask occurrence.
        occurrence: SubtaskOccurrence,
        /// The validated replacement identity for that exact occurrence.
        replacement_id: SubtaskReplacementId,
    },
    /// Checks one exact current subtask occurrence through the signed native write boundary.
    SubtaskCheck {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// The parsed human-facing one-based immutable subtask occurrence.
        occurrence: SubtaskOccurrence,
    },
    /// Completes one current task through the signed native write boundary.
    TransitionDone {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
    },
    /// Abandons one current open task.
    TransitionAbandoned {
        /// Parsed task reference resolved after canonical history is read.
        reference: TaskReference,
    },
    /// Lists all current tasks, optionally restricted to one status.
    List(Option<TaskStatus>),
    /// Resolves one durable task reference.
    Show(TaskReference),
    /// Searches current user-facing task text.
    Search(String),
    /// Selects the task to continue under the one-active-task workflow policy.
    Next,
}

#[expect(
    clippy::pub_with_shorthand,
    reason = "the parent command shell renders the one repository-independent nested help response"
)]
/// Returns static task help when a parsed invocation needs no repository state.
pub(super) fn context_free_output(command: &TaskCommand) -> Option<String> {
    if matches!(command, TaskCommand::Help) {
        return Some(help_output());
    }
    None
}

/// Renders the canonical native task grammar for explicit help.
fn help_output() -> String {
    format!("usage: tiber tasks <{TASKS_COMMAND_GRAMMAR}>\n")
}

/// Stable typed failures from the read-only task adapter.
#[derive(Debug)]
#[expect(
    clippy::pub_with_shorthand,
    reason = "this adapter error is visible only to the direct command shell that renders its stable code"
)]
pub(super) enum TaskCliError {
    /// No nested task operation was supplied.
    MissingSubcommand,
    /// The supplied nested operation is unsupported.
    UnknownSubcommand,
    /// A nested operation received an invalid argument shape.
    InvalidArguments,
    /// An operating-system argument could not be represented as UTF-8 text.
    InvalidArgumentEncoding,
    /// The requested task status is not one of Tiber's durable states.
    InvalidStatus,
    /// A search query contained no meaningful text.
    EmptySearchQuery,
    /// A semantic task value was rejected at the CLI boundary.
    Core(TaskCoreError),
    /// The read-only Git snapshot could not be prepared.
    Store(GitStoreError),
    /// Tiber could not construct or page the committed task transaction history.
    History(TransactionHistoryError),
    /// The committed task facts could not be folded into the query projection.
    Projection(TaskProjectionError),
    /// A narrow task-command decision rejected the canonical task facts or request.
    Command(TaskCommandError),
    /// The signed one-shot task publication could not be confirmed.
    Publication(TiberPublicationError),
    /// Task creation failed while retaining the exact stable retry intent.
    CreationPublication {
        /// Stable identity prefix that must be reused for reconciliation.
        id_prefix: TaskId,
        /// Canonical title bound to that creation intent.
        title: TaskTitle,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// Task-details publication failed while retaining its exact retry intent.
    UpdatePublication {
        /// Durable task identity bound to the retry.
        task: TaskId,
        /// Exact replacement title bound to the retry.
        title: TaskTitle,
        /// Exact replacement summary bound to the retry.
        summary: String,
        /// Exact replacement context bound to the retry.
        context: String,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// Acceptance-add publication failed while retaining its exact retry intent.
    AcceptanceAddPublication {
        /// Durable task identity bound to the retry.
        task: TaskId,
        /// Exact criterion bound to the retry.
        criterion: String,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// Dependency-link publication failed while retaining its exact retry intent.
    DependencyLinkPublication {
        /// Task that becomes blocked.
        task: TaskId,
        /// Task that blocks the target.
        blocker: TaskId,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// Priority publication failed while retaining both resolved retry endpoints.
    TaskPriorityPublication {
        /// Task moved by the exact retry intent.
        task: TaskId,
        /// Anchor that must immediately follow the moved task.
        before: TaskId,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// Abandonment publication failed while retaining the resolved retry target.
    TaskAbandonmentPublication {
        /// Durable task identity bound to the exact retry.
        task: TaskId,
        /// Underlying typed publication outcome.
        source: Box<TiberPublicationError>,
    },
    /// The CLI could not create its bounded local async runtime for `EventCore` append work.
    Runtime(io::Error),
    /// A static native task stream pattern was rejected by the `EventCore` boundary.
    StreamPattern,
    /// The task projection resolved an ID that it then could not retrieve.
    ProjectionTaskMissing,
}

impl TaskCliError {
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "matching the borrowed typed error retains its causal source without cloning it"
    )]
    /// Returns a stable owner-facing error code without exposing Git stderr.
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::MissingSubcommand => "tiber_tasks_subcommand_required",
            Self::UnknownSubcommand => "tiber_tasks_unknown_subcommand",
            Self::InvalidArguments => "tiber_tasks_invalid_arguments",
            Self::InvalidArgumentEncoding => "tiber_tasks_invalid_argument_encoding",
            Self::InvalidStatus => "tiber_tasks_invalid_status",
            Self::EmptySearchQuery => "tiber_tasks_search_query_required",
            Self::Core(error) => error.code(),
            Self::Store(error) => error.code(),
            Self::History(_) => "tiber_tasks_history_read_failed",
            Self::Projection(error) => error.code(),
            Self::Command(error) => error.code(),
            Self::Publication(error) => error.code(),
            Self::CreationPublication { source, .. }
            | Self::UpdatePublication { source, .. }
            | Self::AcceptanceAddPublication { source, .. }
            | Self::DependencyLinkPublication { source, .. }
            | Self::TaskPriorityPublication { source, .. }
            | Self::TaskAbandonmentPublication { source, .. } => source.code(),
            Self::Runtime(_) => "tiber_tasks_runtime_unavailable",
            Self::StreamPattern => "tiber_tasks_stream_pattern_invalid",
            Self::ProjectionTaskMissing => "tiber_tasks_projection_task_missing",
        }
    }

    /// Reports whether this failure is an invalid command grammar rather than a repository read failure.
    pub(super) const fn is_usage_error(&self) -> bool {
        matches!(
            self,
            Self::MissingSubcommand
                | Self::UnknownSubcommand
                | Self::InvalidArguments
                | Self::InvalidArgumentEncoding
                | Self::InvalidStatus
                | Self::EmptySearchQuery
                | Self::Core(_)
                | Self::Command(
                    TaskCommandError::InvalidAcceptanceIndex
                        | TaskCommandError::InvalidSubtaskOccurrence
                        | TaskCommandError::InvalidSubtaskReplacementId
                )
                | Self::Projection(TaskProjectionError::InvalidTaskReference)
        )
    }

    /// Renders safe owner recovery text for bounded activation failures.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "matching the borrowed command error keeps activation recovery rendering non-owning at the CLI boundary"
    )]
    fn activation_error_message(&self) -> Option<String> {
        match self {
            Self::Command(TaskCommandError::TaskActivationNotBacklog { task, status }) => {
                Some(format!(
                    "task `{}` is currently `{}`, not `backlog`; reload with `tiber tasks show {}` before retrying",
                    task.as_str(),
                    status_name(*status),
                    task.as_str()
                ))
            }
            Self::Command(TaskCommandError::TaskActivationActiveTask { active_task }) => {
                Some(format!(
                    "task `{}` is already active; continue it or inspect `tiber tasks next` before starting another task",
                    active_task.as_str()
                ))
            }
            Self::Command(TaskCommandError::MultipleActiveTasks { active_tasks }) => {
                Some(format!(
                    "multiple tasks are active ({}); reload with `tiber tasks list --status in-progress` before retrying",
                    task_ids(active_tasks)
                ))
            }
            Self::Command(TaskCommandError::TaskActivationBlocked { task, blocker }) => {
                Some(format!(
                    "task `{}` is blocked by `{}`; reload with `tiber tasks show {}` before retrying",
                    task.as_str(),
                    blocker.as_str(),
                    task.as_str()
                ))
            }
            Self::Command(TaskCommandError::TaskActivationNotNextEligible { task, next }) => {
                Some(format!(
                    "task `{}` is not the next eligible task; run `tiber tasks start {}` or inspect `tiber tasks next` before retrying",
                    task.as_str(),
                    next.as_str()
                ))
            }
            Self::Command(TaskCommandError::TaskActivationOrderDrift { task }) => Some(format!(
                "task `{}` has invalid board ordering; reload with `tiber tasks show {}` before retrying",
                task.as_str(),
                task.as_str()
            )),
            Self::Command(TaskCommandError::TaskActivationMalformedHistory) => Some(
                "the authoritative task history cannot safely decide activation; reload with `tiber tasks next` before retrying".to_owned(),
            ),
            Self::Command(_)
            | Self::MissingSubcommand
            | Self::UnknownSubcommand
            | Self::InvalidArguments
            | Self::InvalidArgumentEncoding
            | Self::InvalidStatus
            | Self::EmptySearchQuery
            | Self::Core(_)
            | Self::Store(_)
            | Self::History(_)
            | Self::Projection(_)
            | Self::Publication(_)
            | Self::CreationPublication { .. }
            | Self::UpdatePublication { .. }
            | Self::AcceptanceAddPublication { .. }
            | Self::DependencyLinkPublication { .. }
            | Self::TaskPriorityPublication { .. }
            | Self::TaskAbandonmentPublication { .. }
            | Self::Runtime(_)
            | Self::StreamPattern
            | Self::ProjectionTaskMissing => None,
        }
    }
}

impl fmt::Display for TaskCliError {
    #[expect(
        clippy::pattern_type_mismatch,
        clippy::too_many_lines,
        reason = "matching the borrowed typed error keeps formatting non-owning at the CLI boundary"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(message) = self.activation_error_message() {
            return f.write_str(&message);
        }
        let message = match self {
            Self::MissingSubcommand => "a task subcommand is required",
            Self::UnknownSubcommand => "the task subcommand is not supported",
            Self::InvalidArguments => "the task subcommand arguments are invalid",
            Self::InvalidArgumentEncoding => "task arguments must be valid UTF-8",
            Self::InvalidStatus => "status must be backlog, in-progress, done, or abandoned",
            Self::EmptySearchQuery => "search query must not be empty",
            Self::Core(_) => "task input is invalid",
            Self::Store(_) => "the authoritative Tiber task snapshot could not be opened",
            Self::History(_) => "the authoritative Tiber task history could not be read",
            Self::Projection(TaskProjectionError::InvalidTaskReference) => {
                "task reference is invalid"
            }
            Self::Command(TaskCommandError::InvalidAcceptanceIndex) => {
                "acceptance index must be a positive integer"
            }
            Self::Command(TaskCommandError::InvalidSubtaskOccurrence) => {
                "subtask occurrence must be a positive integer"
            }
            Self::Command(TaskCommandError::InvalidSubtaskReplacementId) => {
                "replacement subtask ID must be non-empty and contain no control characters"
            }
            Self::Command(TaskCommandError::TaskNotInProgress { task, status }) => {
                return write!(
                    f,
                    "task `{}` is currently `{}`, not `in-progress`; reload with `tiber tasks show {}` before retrying",
                    task.as_str(),
                    status_name(*status),
                    task.as_str()
                );
            }
            Self::Command(TaskCommandError::SubtaskOccurrenceMissing { task, occurrence }) => {
                return write!(
                    f,
                    "subtask occurrence {} does not exist for task `{}`; reload with `tiber tasks show {}` before choosing an occurrence",
                    occurrence.zero_based_value().saturating_add(1),
                    task.as_str(),
                    task.as_str()
                );
            }
            Self::Command(TaskCommandError::AcceptanceItemUnchecked { task, index }) => {
                let displayed_index = index.zero_based_value().saturating_add(1);
                return write!(
                    f,
                    "task `{}` cannot transition to done because acceptance item {} is unchecked; run `tiber tasks acceptance check {} {}` before retrying",
                    task.as_str(),
                    displayed_index,
                    task.as_str(),
                    displayed_index
                );
            }
            Self::Command(TaskCommandError::DependencySelfLink { task }) => {
                return write!(f, "task `{}` cannot be blocked by itself", task.as_str());
            }
            Self::Command(TaskCommandError::TaskPrioritySelfReference { task }) => {
                return write!(
                    f,
                    "task `{}` cannot be prioritized before itself",
                    task.as_str()
                );
            }
            Self::Command(TaskCommandError::TaskPriorityEndpointNotOpen { task, status }) => {
                return write!(
                    f,
                    "task `{}` is currently `{}`, so it cannot define open-board priority",
                    task.as_str(),
                    status_name(*status)
                );
            }
            Self::Command(TaskCommandError::TaskAbandonmentNotBacklog { task, status }) => {
                return write!(
                    f,
                    "task `{}` is currently `{}`; abandonment requires `backlog`",
                    task.as_str(),
                    status_name(*status)
                );
            }
            Self::Command(TaskCommandError::SubtaskOccurrenceUnchecked { task, occurrence }) => {
                let displayed_occurrence = occurrence.zero_based_value().saturating_add(1);
                return write!(
                    f,
                    "task `{}` cannot transition to done because subtask {} is unchecked; run `tiber tasks subtask check {} {}` before retrying",
                    task.as_str(),
                    displayed_occurrence,
                    task.as_str(),
                    displayed_occurrence
                );
            }
            Self::Command(_) => {
                "the authoritative Tiber task history could not decide that task change"
            }
            Self::Publication(TiberPublicationError::AuthorityChanged) => {
                "the task authority changed before this update could be published; reload with `tiber tasks show <ref>` before retrying"
            }
            Self::Publication(TiberPublicationError::Conflict) => {
                "the task update conflicted with another authority change; reload with `tiber tasks show <ref>` before retrying"
            }
            Self::Publication(TiberPublicationError::Ambiguous) => {
                "the task update may already have been published; reload with `tiber tasks show <ref>` before retrying"
            }
            Self::Publication(_) => "the authoritative Tiber task update could not be published",
            Self::CreationPublication {
                id_prefix,
                title,
                source,
            } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    let quoted_id_prefix = quote_shell_argument(id_prefix.as_str());
                    let quoted_title = quote_shell_argument(title.as_str());
                    return write!(
                        f,
                        "task creation may already be durable; retry exactly: `tiber tasks create --id {quoted_id_prefix} {quoted_title}`"
                    );
                }
                "the authoritative Tiber task creation could not be published"
            }
            Self::UpdatePublication {
                task,
                title,
                summary,
                context,
                source,
            } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    return write!(
                        f,
                        "task update may already be durable; retry exactly: `tiber tasks update {} --title {} --summary {} --context {}`",
                        quote_shell_argument(task.as_str()),
                        quote_shell_argument(title.as_str()),
                        quote_shell_argument(summary),
                        quote_shell_argument(context)
                    );
                }
                "the authoritative Tiber task update could not be published"
            }
            Self::AcceptanceAddPublication {
                task,
                criterion,
                source,
            } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    return write!(
                        f,
                        "acceptance addition may already be durable; retry exactly: `tiber tasks acceptance add {} {}`",
                        quote_shell_argument(task.as_str()),
                        quote_shell_argument(criterion)
                    );
                }
                "the authoritative Tiber acceptance addition could not be published"
            }
            Self::DependencyLinkPublication {
                task,
                blocker,
                source,
            } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    return write!(
                        f,
                        "dependency link may already be durable; retry exactly: `tiber tasks link blocked-by {} {}`",
                        quote_shell_argument(task.as_str()),
                        quote_shell_argument(blocker.as_str())
                    );
                }
                "the authoritative Tiber dependency link could not be published"
            }
            Self::TaskPriorityPublication {
                task,
                before,
                source,
            } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    return write!(
                        f,
                        "priority change may already be durable; retry exactly: `tiber tasks prioritize {} --before {}`",
                        quote_shell_argument(task.as_str()),
                        quote_shell_argument(before.as_str())
                    );
                }
                "the authoritative Tiber priority change could not be published"
            }
            Self::TaskAbandonmentPublication { task, source } => {
                if matches!(source.as_ref(), TiberPublicationError::Ambiguous) {
                    return write!(
                        f,
                        "task abandonment may already be durable; retry exactly: `tiber tasks transition {} abandoned`",
                        quote_shell_argument(task.as_str())
                    );
                }
                "the authoritative Tiber task abandonment could not be published"
            }
            Self::Runtime(_) => "the local task runtime could not be started",
            Self::StreamPattern => "the native task stream selection is invalid",
            Self::Projection(TaskProjectionError::TaskReferenceMissing { reference }) => {
                return write!(f, "no task matches reference `{}`", reference.as_str());
            }
            Self::Projection(TaskProjectionError::TaskReferenceAmbiguous {
                reference,
                matches,
            }) => {
                let mut message = format!(
                    "task reference `{}` is ambiguous; matching task IDs: ",
                    reference.as_str()
                );
                append_joined(&mut message, matches.iter().map(TaskId::as_str));
                return f.write_str(&message);
            }
            Self::Projection(_) => "the authoritative Tiber task history could not be projected",
            Self::ProjectionTaskMissing => {
                "the authoritative Tiber task projection is inconsistent"
            }
        };
        f.write_str(message)
    }
}

#[expect(
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "the CLI error exposes only its useful causal source; legacy Error defaults add no diagnostic value"
)]
impl Error for TaskCliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::History(error) => Some(error),
            Self::Projection(error) => Some(error),
            Self::Command(error) => Some(error),
            Self::Publication(error) => Some(error),
            Self::CreationPublication { source, .. }
            | Self::UpdatePublication { source, .. }
            | Self::AcceptanceAddPublication { source, .. }
            | Self::DependencyLinkPublication { source, .. }
            | Self::TaskPriorityPublication { source, .. }
            | Self::TaskAbandonmentPublication { source, .. } => Some(source.as_ref()),
            Self::Runtime(error) => Some(error),
            Self::Core(error) => Some(error),
            Self::MissingSubcommand
            | Self::UnknownSubcommand
            | Self::InvalidArguments
            | Self::InvalidArgumentEncoding
            | Self::InvalidStatus
            | Self::EmptySearchQuery
            | Self::StreamPattern
            | Self::ProjectionTaskMissing => None,
        }
    }
}

/// Quotes one argument for the exact POSIX-shell retry diagnostic.
fn quote_shell_argument(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

#[expect(
    clippy::cognitive_complexity,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::too_many_lines,
    reason = "the closed CLI grammar keeps all semantic alternatives visible in one slice-pattern parser with typed propagation"
)]
/// Validates the nested command grammar before accessing any repository state.
fn parse_command(subcommand: &str, arguments: &[String]) -> Result<TaskCommand, TaskCliError> {
    match subcommand {
        "-h" | "--help" if arguments.is_empty() => Ok(TaskCommand::Help),
        "create" => match arguments {
            [flag, id_prefix, title @ ..] if flag == "--id" && !title.is_empty() => {
                Ok(TaskCommand::Create {
                    id_prefix: Some(TaskId::parse(id_prefix).map_err(TaskCliError::Core)?),
                    title: TaskTitle::parse(&title.join(" ")).map_err(TaskCliError::Core)?,
                })
            }
            [flag] if flag == "--id" => Err(TaskCliError::InvalidArguments),
            [flag, _id_prefix] if flag == "--id" => Err(TaskCliError::InvalidArguments),
            title if !title.is_empty() => Ok(TaskCommand::Create {
                id_prefix: None,
                title: TaskTitle::parse(&title.join(" ")).map_err(TaskCliError::Core)?,
            }),
            _ => Err(TaskCliError::InvalidArguments),
        },
        "start" => match arguments {
            [reference] => Ok(TaskCommand::Start {
                reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
            }),
            _ => Err(TaskCliError::InvalidArguments),
        },
        "update" => match arguments {
            [
                reference,
                title_flag,
                title,
                summary_flag,
                summary,
                context_flag,
                context,
            ] if title_flag == "--title"
                && summary_flag == "--summary"
                && context_flag == "--context" =>
            {
                Ok(TaskCommand::Update {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    title: TaskTitle::parse(title).map_err(TaskCliError::Core)?,
                    summary: summary.clone(),
                    context: context.clone(),
                })
            }
            _ => Err(TaskCliError::InvalidArguments),
        },
        "link" => match arguments {
            [kind, reference, blocker] if kind == "blocked-by" => Ok(TaskCommand::LinkBlockedBy {
                reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                blocker: TaskReference::parse(blocker).map_err(TaskCliError::Projection)?,
            }),
            _ => Err(TaskCliError::InvalidArguments),
        },
        "prioritize" => match arguments {
            [reference, flag, before] if flag == "--before" => Ok(TaskCommand::Prioritize {
                reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                before: TaskReference::parse(before).map_err(TaskCliError::Projection)?,
            }),
            _ => Err(TaskCliError::InvalidArguments),
        },
        "acceptance" => match arguments {
            [operation, reference, criterion] if operation == "add" => {
                Ok(TaskCommand::AcceptanceAdd {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    criterion: criterion.clone(),
                })
            }
            [operation, reference, index] if operation == "check" => {
                Ok(TaskCommand::AcceptanceCheck {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    index: AcceptanceIndex::parse_one_based(index)
                        .map_err(TaskCliError::Command)?,
                })
            }
            _ => Err(TaskCliError::InvalidArguments),
        },
        "subtask" => match arguments {
            [operation, reference, occurrence] if operation == "check" => {
                Ok(TaskCommand::SubtaskCheck {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    occurrence: SubtaskOccurrence::parse_one_based(occurrence)
                        .map_err(TaskCliError::Command)?,
                })
            }
            [operation, reference, occurrence, replacement_id]
                if operation == "repair-duplicate" =>
            {
                Ok(TaskCommand::SubtaskRepairDuplicate {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    occurrence: SubtaskOccurrence::parse_one_based(occurrence)
                        .map_err(TaskCliError::Command)?,
                    replacement_id: SubtaskReplacementId::parse(replacement_id)
                        .map_err(TaskCliError::Command)?,
                })
            }
            _ => Err(TaskCliError::InvalidArguments),
        },
        "list" => match arguments {
            [] => Ok(TaskCommand::List(None)),
            [flag, requested_status] if flag == "--status" => {
                Ok(TaskCommand::List(Some(parse_status(requested_status)?)))
            }
            _ => Err(TaskCliError::InvalidArguments),
        },
        "show" => match arguments {
            [reference] => Ok(TaskCommand::Show(
                TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
            )),
            _ => Err(TaskCliError::InvalidArguments),
        },
        "search" => {
            let query = arguments.join(" ");
            if query.trim().is_empty() {
                Err(TaskCliError::EmptySearchQuery)
            } else {
                Ok(TaskCommand::Search(query))
            }
        }
        "next" if arguments.is_empty() => Ok(TaskCommand::Next),
        "next" => Err(TaskCliError::InvalidArguments),
        "transition" => match arguments {
            [reference, status] if status == "done" => Ok(TaskCommand::TransitionDone {
                reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
            }),
            [reference, status] if status == "abandoned" => Ok(TaskCommand::TransitionAbandoned {
                reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
            }),
            _ => Err(TaskCliError::InvalidArguments),
        },
        _ => Err(TaskCliError::UnknownSubcommand),
    }
}

/// Decides and publishes one unchecked acceptance criterion.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative CLI boundary preserves typed railway propagation across history, projection, modeled decision, and signed publication"
)]
fn add_acceptance(
    repository: &Path,
    reference: &TaskReference,
    criterion: String,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = AddAcceptance::new(task.clone(), criterion);
    let Some(publication) =
        decide_add_acceptance(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!("criterion already added for {}\n", task.as_str()));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_acceptance_add(publication))
        .map_err(|source| TaskCliError::AcceptanceAddPublication {
            task: task.clone(),
            criterion: request.criterion().to_owned(),
            source: Box::new(source),
        })?;
    Ok(format!(
        "added acceptance criterion for {} at {}\n",
        task.as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one reciprocal blocked-by dependency.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative CLI boundary preserves typed propagation across history, projection, modeled decision, and signed publication"
)]
fn link_blocked_by(
    repository: &Path,
    reference: &TaskReference,
    blocker_reference: &TaskReference,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let blocker = projection
        .resolve_task_reference(blocker_reference)
        .map_err(TaskCliError::Projection)?;
    let request = LinkBlockedBy::new(task.clone(), blocker.clone());
    let Some(publication) =
        decide_link_blocked_by(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!(
            "{} already blocked by {}\n",
            task.as_str(),
            blocker.as_str()
        ));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_dependency_link(publication))
        .map_err(|source| TaskCliError::DependencyLinkPublication {
            task,
            blocker,
            source: Box::new(source),
        })?;
    Ok(format!(
        "linked {} blocked by {} at {}\n",
        request.task().as_str(),
        request.blocker().as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one strict board-priority movement.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative CLI boundary preserves typed propagation across canonical history, modeled decision, exact fences, and signed publication"
)]
fn prioritize_task(
    repository: &Path,
    reference: &TaskReference,
    before_reference: &TaskReference,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let before = projection
        .resolve_task_reference(before_reference)
        .map_err(TaskCliError::Projection)?;
    let request = PrioritizeTask::new(task.clone(), before.clone());
    let Some(publication) =
        decide_prioritize_task(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!(
            "{} already prioritized before {}\n",
            task.as_str(),
            before.as_str()
        ));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_priority(publication))
        .map_err(|source| TaskCliError::TaskPriorityPublication {
            task: task.clone(),
            before: before.clone(),
            source: Box::new(source),
        })?;
    Ok(format!(
        "prioritized {} before {} at {}\n",
        request.task().as_str(),
        request.before().as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one task abandonment.
#[expect(
    clippy::question_mark_used,
    reason = "the command boundary preserves typed history, projection, decision, runtime, and publication failures through one railway"
)]
fn abandon_task(repository: &Path, reference: &TaskReference) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = AbandonTask::new(task.clone());
    let Some(publication) =
        decide_abandon_task(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!("{} already abandoned\n", task.as_str()));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_abandonment(publication))
        .map_err(|source| TaskCliError::TaskAbandonmentPublication {
            task: task.clone(),
            source: Box::new(source),
        })?;
    Ok(format!(
        "abandoned {} at {}\n",
        request.task().as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one native backlog task creation.
#[expect(
    clippy::question_mark_used,
    reason = "the creation boundary sequences canonical history, modeled decision, exact revision fence, and signed publication with typed failures"
)]
fn create_task(
    repository: &Path,
    requested_id_prefix: Option<TaskId>,
    title: TaskTitle,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let (generated_id_prefix, recorded_at) = creation_metadata()?;
    let has_retry_identity = requested_id_prefix.is_some();
    let id_prefix = requested_id_prefix.unwrap_or(generated_id_prefix);
    let request = if has_retry_identity {
        CreateTask::new(id_prefix.clone(), recorded_at, title.clone())
    } else {
        CreateTask::new_implicit(id_prefix.clone(), recorded_at, title.clone())
    };
    let decision = decide_create_task(history.events(), &request).map_err(TaskCliError::Command)?;
    let publication = match decision {
        TaskCreationDecision::Publish(publication) => publication,
        TaskCreationDecision::AlreadyCreated(task_id) => {
            return Ok(format!("already created {}\n", task_id.as_str()));
        }
    };
    let task_id = publication.task_id().clone();
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_creation(publication))
        .map_err(|source| TaskCliError::CreationPublication {
            id_prefix,
            title,
            source: Box::new(source),
        })?;
    Ok(format!(
        "created {} at {}\n",
        task_id.as_str(),
        outcome.revision().as_str()
    ))
}

/// Supplies only the metadata required by the currently proven creation contract.
fn creation_metadata() -> Result<(TaskId, String), TaskCliError> {
    TaskId::parse(&format!("00000000-{}", Uuid::new_v4().simple()))
        .map(|prefix| {
            (
                prefix,
                Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            )
        })
        .map_err(TaskCliError::Core)
}

/// Decides and publishes one exact replacement of editable task details.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative update boundary sequences typed history, decision, publication, and retry failures"
)]
fn update_task(
    repository: &Path,
    reference: &TaskReference,
    title: TaskTitle,
    summary: String,
    context: String,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = UpdateTaskDetails::new(task.clone(), title, summary, context);
    let Some(publication) =
        decide_update_task_details(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!("{} already updated\n", task.as_str()));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_details(publication))
        .map_err(|source| TaskCliError::UpdatePublication {
            task: task.clone(),
            title: request.title().clone(),
            summary: request.summary().to_owned(),
            context: request.context().to_owned(),
            source: Box::new(source),
        })?;
    Ok(format!(
        "updated {} at {}\n",
        task.as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one bounded task activation through the native signed boundary.
#[expect(
    clippy::question_mark_used,
    reason = "the command sequences canonical read, narrow activation decision, fixed revision fence, and one closed modeled fact with typed recovery"
)]
fn start_task(repository: &Path, reference: &TaskReference) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = StartTask::new(task.clone());
    let Some(publication) =
        decide_start_task(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!("{} already in progress\n", task.as_str()));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_activation(publication))
        .map_err(TaskCliError::Publication)?;
    Ok(format!(
        "activated {} at {}\n",
        task.as_str(),
        outcome.revision().as_str()
    ))
}

/// Parses OS arguments once at the command boundary.
fn parse_arguments(arguments: impl Iterator<Item = OsString>) -> Result<Vec<String>, TaskCliError> {
    arguments
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_argument| TaskCliError::InvalidArgumentEncoding)
        })
        .collect()
}

#[expect(
    clippy::question_mark_used,
    reason = "the read adapter preserves the exact typed store and projection failure in its causal chain"
)]
/// Opens one immutable authoritative snapshot and folds all committed task facts.
fn load_projection(repository: &Path) -> Result<TaskBoardProjection, TaskCliError> {
    let (history, _revision) = load_history_and_revision(repository)?;
    TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)
}

/// Reads one authoritative task snapshot and retains its exact signed revision for a command.
#[expect(
    clippy::question_mark_used,
    reason = "the command boundary preserves a matching immutable history and revision through typed source failures"
)]
fn load_history_and_revision(
    repository: &Path,
) -> Result<(TaskHistory, TiberRevision), TaskCliError> {
    let store = TiberEventStore::open(repository).map_err(TaskCliError::Store)?;
    let revision = store.revision().clone();
    let history = read_history(&store)?;
    Ok((history, revision))
}

/// Decides and publishes one current acceptance check through the native signed write boundary.
#[expect(
    clippy::question_mark_used,
    reason = "the command sequences canonical read, narrow pure decision, exact revision fence, and one bounded EventCore append with typed recovery"
)]
fn check_acceptance(
    repository: &Path,
    reference: &TaskReference,
    index: AcceptanceIndex,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = CheckAcceptance::new(task.clone(), index);
    let displayed_index = index.zero_based_value().saturating_add(1);
    let Some(publication) =
        decide_check_acceptance(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!(
            "acceptance {} already checked for {}\n",
            displayed_index,
            task.as_str()
        ));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_acceptance_check(publication))
        .map_err(TaskCliError::Publication)?;
    Ok(format!(
        "checked acceptance {} for {} at {}\n",
        displayed_index,
        task.as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one exact duplicate-subtask identity correction through the native signed boundary.
#[expect(
    clippy::question_mark_used,
    reason = "the command sequences canonical read, exact preimage selection, narrow pure decision, fixed revision fence, and one bounded append with typed recovery"
)]
fn repair_duplicate_subtask(
    repository: &Path,
    reference: &TaskReference,
    occurrence: SubtaskOccurrence,
    replacement_id: SubtaskReplacementId,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let expected =
        corrected_subtask_preimage(history.events(), &task, occurrence, replacement_id.as_str())
            .or_else(|| {
                projection
                    .task(&task)
                    .and_then(|current| current.subtasks.get(occurrence.zero_based_value()))
                    .cloned()
            })
            .ok_or_else(|| {
                TaskCliError::Command(TaskCommandError::SubtaskOccurrenceMissing {
                    task: task.clone(),
                    occurrence,
                })
            })?;
    let request = RepairDuplicateSubtaskId::new(task.clone(), occurrence, expected, replacement_id);
    let displayed_occurrence = occurrence.zero_based_value().saturating_add(1);
    let Some(publication) = decide_repair_duplicate_subtask_id(history.events(), &request)
        .map_err(TaskCliError::Command)?
    else {
        return Ok(format!(
            "duplicate subtask {} already corrected for {}\n",
            displayed_occurrence,
            task.as_str()
        ));
    };
    let old_id = publication.corrected_fact().expected.id.clone();
    let new_id = publication.corrected_fact().replacement_id.clone();
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_subtask_id_correction(publication))
        .map_err(TaskCliError::Publication)?;
    Ok(format!(
        "corrected duplicate subtask {} for {}: {} -> {} at {}\n",
        displayed_occurrence,
        task.as_str(),
        old_id,
        new_id,
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one exact subtask-occurrence check through the native signed boundary.
#[expect(
    clippy::question_mark_used,
    reason = "the command sequences canonical read, immutable occurrence selection, narrow pure decision, fixed revision fence, and one bounded append with typed recovery"
)]
fn check_subtask_occurrence(
    repository: &Path,
    reference: &TaskReference,
    occurrence: SubtaskOccurrence,
) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let expected = projection
        .task(&task)
        .and_then(|current| current.subtasks.get(occurrence.zero_based_value()))
        .cloned()
        .ok_or_else(|| {
            TaskCliError::Command(TaskCommandError::SubtaskOccurrenceMissing {
                task: task.clone(),
                occurrence,
            })
        })?;
    let request = CheckSubtaskOccurrence::new(task.clone(), occurrence, expected);
    let displayed_occurrence = occurrence.zero_based_value().saturating_add(1);
    let Some(publication) = decide_check_subtask_occurrence(history.events(), &request)
        .map_err(TaskCliError::Command)?
    else {
        return Ok(format!(
            "subtask {} already checked for {}\n",
            displayed_occurrence,
            task.as_str()
        ));
    };
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_subtask_occurrence_check(publication))
        .map_err(TaskCliError::Publication)?;
    Ok(format!(
        "checked subtask {} for {} at {}\n",
        displayed_occurrence,
        task.as_str(),
        outcome.revision().as_str()
    ))
}

/// Decides and publishes one terminal task completion through the native signed boundary.
#[expect(
    clippy::question_mark_used,
    reason = "the command sequences canonical read, narrow completion decision, fixed revision fence, and one closed modeled batch with typed recovery"
)]
fn complete_task(repository: &Path, reference: &TaskReference) -> Result<String, TaskCliError> {
    let (history, revision) = load_history_and_revision(repository)?;
    let projection = TaskBoardProjection::replay(&history).map_err(TaskCliError::Projection)?;
    let task = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let request = CompleteTask::new(task.clone());
    let Some(publication) =
        decide_complete_task(history.events(), &request).map_err(TaskCliError::Command)?
    else {
        return Ok(format!("{} already done\n", task.as_str()));
    };
    let transitioned = publication.transitioned_fact().is_some();
    let mut publisher =
        TiberEventPublisher::open_at(repository, &revision).map_err(TaskCliError::Publication)?;
    let runtime = RuntimeBuilder::new_current_thread()
        .build()
        .map_err(TaskCliError::Runtime)?;
    let outcome = runtime
        .block_on(publisher.publish_task_completion(publication))
        .map_err(TaskCliError::Publication)?;
    if transitioned {
        Ok(format!(
            "transitioned {} to done at {}\n",
            task.as_str(),
            outcome.revision().as_str()
        ))
    } else {
        Ok(format!(
            "reconciled completed task {} board entries at {}\n",
            task.as_str(),
            outcome.revision().as_str()
        ))
    }
}

/// Finds the original preimage for an already-published exact correction retry.
///
/// The command state revalidates this historical preimage and current replay
/// before returning its idempotent no-op. This lookup only lets a retry name
/// the same prior intent after its visible occurrence has already become the
/// replacement ID; it cannot bypass the pure command's durable checks. A task
/// removal or creation bounds the lookup to the current task lifetime.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "the retry-only lookup keeps the exact event-pattern preimage selection isolated from the CLI effect boundary and deliberately ignores unrelated future durable facts"
)]
fn corrected_subtask_preimage(
    events: &[TaskEvent],
    task: &TaskId,
    occurrence: SubtaskOccurrence,
    replacement_id: &str,
) -> Option<Subtask> {
    for event in events.iter().rev() {
        match event {
            TaskEvent::TaskSubtaskIdCorrected(corrected)
                if &corrected.stem == task
                    && corrected.index == occurrence.zero_based_value()
                    && corrected.replacement_id == replacement_id =>
            {
                return Some(corrected.expected.clone());
            }
            TaskEvent::HistoricalTaskRemoved(removed) if &removed.stem == task => return None,
            TaskEvent::TaskCreated(created) if &created.task.stem == task => return None,
            _ => {}
        }
    }
    None
}

#[expect(
    clippy::question_mark_used,
    reason = "the bounded transaction-page loop preserves the exact typed store failure while constructing the immutable task history"
)]
/// Reads the explicit task-stream union in immutable transaction and envelope order.
///
/// The store validates every selected `TaskEvent` payload before paging. It
/// rejects a selected transaction graph without one causal replay sequence,
/// rather than sorting independent `EventCore` projection cursors. This
/// deliberately excludes unrelated legacy workflow streams that share
/// `tiber.domain_event`.
fn read_history(store: &TiberEventStore) -> Result<TaskHistory, TaskCliError> {
    let stream_patterns = task_history_stream_patterns()?;
    let reader = store
        .verified_transaction_reader::<TaskEvent>(&stream_patterns)
        .map_err(TaskCliError::History)?;
    let mut facts = Vec::new();
    let mut page = TransactionEventPage::first(BatchSize::new(TASK_HISTORY_PAGE_SIZE));
    loop {
        let events = reader
            .read_page(page)
            .map_err(|error| TaskCliError::History(TransactionHistoryError::from(error)))?;
        let Some(next_page) = page.next_from_results(&events) else {
            break;
        };
        facts.extend(events);
        page = next_page;
    }
    Ok(TaskHistory::from_ordered_events(facts))
}

#[expect(
    clippy::question_mark_used,
    reason = "static EventCore stream patterns are parsed once into one typed closed stream union and retain a stable boundary failure"
)]
/// Builds the exact typed stream union for native task history.
fn task_history_stream_patterns()
-> Result<[StreamPattern; TASK_HISTORY_STREAM_PATTERNS.len()], TaskCliError> {
    let [repository, board, task_namespace] = TASK_HISTORY_STREAM_PATTERNS;
    Ok([
        task_history_stream_pattern(repository)?,
        task_history_stream_pattern(board)?,
        task_history_stream_pattern(task_namespace)?,
    ])
}

/// Parses one static task-history stream pattern at the CLI boundary.
fn task_history_stream_pattern(stream_pattern: &str) -> Result<StreamPattern, TaskCliError> {
    StreamPattern::try_new(stream_pattern.to_owned()).map_err(|_error| TaskCliError::StreamPattern)
}

/// Lists current task summaries, optionally constrained to a durable status.
fn list_tasks(projection: &TaskBoardProjection, status: Option<TaskStatus>) -> String {
    match status {
        None => render_task_summaries(projection.ordered_tasks()),
        Some(expected) => {
            render_task_summaries(projection.tasks().filter(|task| task.status == expected))
        }
    }
}

#[expect(
    clippy::question_mark_used,
    reason = "reference resolution retains its projection-specific typed failure at the CLI boundary"
)]
/// Resolves and displays one task's full current query state.
fn show_task(
    projection: &TaskBoardProjection,
    reference: &TaskReference,
) -> Result<String, TaskCliError> {
    let task_id = projection
        .resolve_task_reference(reference)
        .map_err(TaskCliError::Projection)?;
    let Some(task) = projection.task(&task_id) else {
        return Err(TaskCliError::ProjectionTaskMissing);
    };
    Ok(render_task(task))
}

/// Searches user-facing task text case-insensitively in stable task-ID order.
fn search_tasks(projection: &TaskBoardProjection, query: &str) -> String {
    let query = query.to_lowercase();
    render_task_summaries(projection.tasks().filter(|task| {
        task.title.as_str().to_lowercase().contains(&query)
            || task.summary.to_lowercase().contains(&query)
            || task.context.to_lowercase().contains(&query)
    }))
}

/// Displays the task to continue under the one-active-task workflow policy, if any.
fn next_task(projection: &TaskBoardProjection) -> Result<String, TaskCliError> {
    projection
        .next_actionable_task()
        .map(|task| task.map_or_else(String::new, |task| render_task_summaries([task])))
        .map_err(TaskCliError::Projection)
}

/// Parses one durable task status at the CLI boundary.
fn parse_status(input: &str) -> Result<TaskStatus, TaskCliError> {
    match input {
        "backlog" => Ok(TaskStatus::Backlog),
        "in-progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        "abandoned" => Ok(TaskStatus::Abandoned),
        _ => Err(TaskCliError::InvalidStatus),
    }
}

/// Renders a stable concise task row with its current lifecycle status.
fn render_task_summaries<'task>(tasks: impl IntoIterator<Item = &'task Task>) -> String {
    let mut output = String::new();
    for task in tasks {
        output.push_str(task.stem.as_str());
        output.push('\t');
        output.push_str(status_name(task.status));
        output.push('\t');
        output.push_str(task.title.as_str());
        output.push('\n');
    }
    output
}

/// Renders one complete task query without relying on a historical file layout.
fn render_task(task: &Task) -> String {
    let mut output = String::from("id: ");
    output.push_str(task.stem.as_str());
    output.push_str("\nstatus: ");
    output.push_str(status_name(task.status));
    output.push_str("\ntitle: ");
    output.push_str(task.title.as_str());
    if !task.committed_at.is_empty() {
        output.push_str("\ncommitted-at: ");
        output.push_str(&task.committed_at);
    }
    output.push_str("\nsummary: ");
    output.push_str(&task.summary);
    output.push_str("\ncontext: ");
    output.push_str(&task.context);
    output.push('\n');
    if !task.tags.is_empty() {
        output.push_str("tags: ");
        append_joined(&mut output, task.tags.iter().map(String::as_str));
        output.push('\n');
    }
    if !task.acceptance.is_empty() {
        output.push_str("acceptance:\n");
        for (index, item) in task.acceptance.iter().enumerate() {
            output.push_str(&index.saturating_add(1).to_string());
            output.push_str(". [");
            output.push_str(if item.checked { "x" } else { " " });
            output.push_str("] ");
            output.push_str(&item.text);
            output.push('\n');
        }
    }
    if !task.subtasks.is_empty() {
        output.push_str("subtasks:\n");
        for (index, subtask) in task.subtasks.iter().enumerate() {
            output.push_str(&index.saturating_add(1).to_string());
            output.push_str(". [");
            output.push_str(if subtask.checked { "x" } else { " " });
            output.push_str("] ");
            output.push_str(&subtask.id);
            output.push(' ');
            output.push_str(&subtask.title);
            if !subtask.after.is_empty() {
                output.push_str(" \u{2014} after: ");
                append_joined(&mut output, subtask.after.iter().map(String::as_str));
            }
            output.push('\n');
        }
    }
    if !task.blocked_by.is_empty() {
        output.push_str("blocked-by: ");
        append_joined(&mut output, task.blocked_by.iter().map(TaskId::as_str));
        output.push('\n');
    }
    if !task.blocks.is_empty() {
        output.push_str("blocks: ");
        append_joined(&mut output, task.blocks.iter().map(TaskId::as_str));
        output.push('\n');
    }
    output
}

/// Appends stable comma-separated text without allocating a temporary joined string.
fn append_joined<'item>(output: &mut String, items: impl IntoIterator<Item = &'item str>) {
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(item);
    }
}

/// Renders stable task identities for a safe activation-recovery diagnostic.
fn task_ids(tasks: &[TaskId]) -> String {
    let mut output = String::new();
    append_joined(&mut output, tasks.iter().map(TaskId::as_str));
    output
}

/// Converts the closed task lifecycle type to the CLI's stable spelling.
const fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "backlog",
        TaskStatus::InProgress => "in-progress",
        TaskStatus::Done => "done",
        TaskStatus::Abandoned => "abandoned",
    }
}

#[cfg(test)]
mod tests {
    use super::TaskCliError;
    use tiber_store_git::publication::TiberPublicationError;

    #[test]
    fn publication_failures_preserve_their_stable_codes_and_safe_recovery() {
        let cases = [
            (
                TiberPublicationError::AuthorityChanged,
                "tiber_store_publication_authority_changed",
                "the task authority changed before this update could be published; reload with `tiber tasks show <ref>` before retrying",
            ),
            (
                TiberPublicationError::Conflict,
                "tiber_store_publication_conflict",
                "the task update conflicted with another authority change; reload with `tiber tasks show <ref>` before retrying",
            ),
            (
                TiberPublicationError::Ambiguous,
                "tiber_store_publication_ambiguous",
                "the task update may already have been published; reload with `tiber tasks show <ref>` before retrying",
            ),
        ];

        for (publication, code, message) in cases {
            let error = TaskCliError::Publication(publication);
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), message);
        }
    }
}
