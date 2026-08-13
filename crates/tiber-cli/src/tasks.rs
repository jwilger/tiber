//! Native Tiber Tasks command adapter.
//!
//! This module owns CLI parsing and rendering only. The Git-backed store owns
//! source selection, while the task service owns the immutable history replay.

use core::{error::Error, fmt};
use std::{ffi::OsString, io, path::Path};

use eventcore_types::{BatchSize, StreamPattern};
use tiber_store_git::publication::{TiberEventPublisher, TiberPublicationError};
use tiber_store_git::{
    GitStoreError, TiberEventStore, TiberRevision, TransactionEventPage, TransactionHistoryError,
};
use tiber_tasks_core::{Task, TaskEvent, TaskId, TaskStatus};
use tiber_tasks_service::command::{
    AcceptanceIndex, CheckAcceptance, TaskCommandError, decide_check_acceptance,
};
use tiber_tasks_service::{TaskBoardProjection, TaskHistory, TaskProjectionError, TaskReference};
use tokio::runtime::Builder as RuntimeBuilder;

/// Maximum number of durable facts fetched by one explicit `EventCore` page.
const TASK_HISTORY_PAGE_SIZE: usize = 64;

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
/// Executes one native `tiber tasks` invocation against the current repository.
pub(super) fn run(
    repository: &Path,
    arguments: impl Iterator<Item = OsString>,
) -> Result<String, TaskCliError> {
    let arguments = parse_arguments(arguments)?;
    let Some((subcommand, remaining)) = arguments.split_first() else {
        return Err(TaskCliError::MissingSubcommand);
    };
    let command = parse_command(subcommand, remaining)?;
    match command {
        TaskCommand::AcceptanceCheck { reference, index } => {
            check_acceptance(repository, &reference, index)
        }
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

/// A completely parsed native read-only task query.
enum TaskCommand {
    /// Sets one current acceptance item checked through the signed native write boundary.
    AcceptanceCheck {
        /// The parsed task reference resolved after canonical history is read.
        reference: TaskReference,
        /// The parsed human-facing one-based acceptance position.
        index: AcceptanceIndex,
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
            Self::Store(error) => error.code(),
            Self::History(_) => "tiber_tasks_history_read_failed",
            Self::Projection(error) => error.code(),
            Self::Command(error) => error.code(),
            Self::Publication(error) => error.code(),
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
                | Self::Command(TaskCommandError::InvalidAcceptanceIndex)
                | Self::Projection(TaskProjectionError::InvalidTaskReference)
        )
    }
}

impl fmt::Display for TaskCliError {
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "matching the borrowed typed error keeps formatting non-owning at the CLI boundary"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingSubcommand => "a task subcommand is required",
            Self::UnknownSubcommand => "the task subcommand is not supported",
            Self::InvalidArguments => "the task subcommand arguments are invalid",
            Self::InvalidArgumentEncoding => "task arguments must be valid UTF-8",
            Self::InvalidStatus => "status must be backlog, in-progress, done, or abandoned",
            Self::EmptySearchQuery => "search query must not be empty",
            Self::Store(_) => "the authoritative Tiber task snapshot could not be opened",
            Self::History(_) => "the authoritative Tiber task history could not be read",
            Self::Projection(TaskProjectionError::InvalidTaskReference) => {
                "task reference is invalid"
            }
            Self::Command(TaskCommandError::InvalidAcceptanceIndex) => {
                "acceptance index must be a positive integer"
            }
            Self::Command(_) => {
                "the authoritative Tiber task history could not decide that acceptance change"
            }
            Self::Publication(_) => "the authoritative Tiber task update could not be published",
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
            Self::Runtime(error) => Some(error),
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

#[expect(
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    reason = "the closed CLI grammar uses slice patterns and typed propagation at its parse boundary"
)]
/// Validates the nested command grammar before accessing any repository state.
fn parse_command(subcommand: &str, arguments: &[String]) -> Result<TaskCommand, TaskCliError> {
    match subcommand {
        "acceptance" => match arguments {
            [operation, reference, index] if operation == "check" => {
                Ok(TaskCommand::AcceptanceCheck {
                    reference: TaskReference::parse(reference).map_err(TaskCliError::Projection)?,
                    index: AcceptanceIndex::parse_one_based(index)
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
        _ => Err(TaskCliError::UnknownSubcommand),
    }
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

/// Converts the closed task lifecycle type to the CLI's stable spelling.
const fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "backlog",
        TaskStatus::InProgress => "in-progress",
        TaskStatus::Done => "done",
        TaskStatus::Abandoned => "abandoned",
    }
}
