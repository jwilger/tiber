#![forbid(unsafe_code)]
#![expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    clippy::exit,
    clippy::implicit_return,
    clippy::return_and_then,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::std_instead_of_core,
    reason = "the thin command adapter keeps lifecycle types beside the TUI shell and uses process exits, OS arguments, and one-shot dispatch helpers at the imperative boundary"
)]

mod tasks;

use std::{
    collections::HashSet,
    env, fs,
    io::Read as _,
    path::{Path, PathBuf},
    process,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::Duration,
};

use eventcore_types::{BatchSize, EventStoreError, StreamPattern};
use ratatui::crossterm::event::{self, Event};
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2};
use sha2::{Digest as _, Sha256};
use tiber_app_server::{
    AccountStatus, AppServerClient, AppServerConfig, OperationCancellation,
    TIBER_REPOSITORY_PROPOSAL_TOOL_NAME, TurnEvent, inspect_protocol_schema,
};
use tiber_repository_core::{
    ComponentScope, OwnerApprovalId, RepositoryAssignmentContext, RepositoryCapability,
    RepositoryContent, RepositoryDispatchOutcome, RepositoryError, RepositoryId,
    RepositoryMutationFailureCode, RepositoryMutationPolicy, RepositoryMutationPrecondition,
    RepositoryMutationProposal, RepositoryMutationProvenance, RepositoryPath,
    RepositoryReconciliationOutcome, RepositoryRetryability, RepositoryService as _, Sha256Digest,
    WritePrecondition,
};
use tiber_repository_linux::{
    LinuxRepositoryConfigurationError, LinuxRepositoryService, LinuxRepositoryServiceConfig,
};
use tiber_repository_service::{
    RepositoryMutationEvent, RepositoryMutationServiceError, RepositoryMutationStream,
    decide_approve_and_prepare_mutation, decide_cancel_mutation,
    decide_cancel_open_proposal_on_restart, decide_deny_mutation, decide_propose_mutation,
    decide_record_applied, decide_record_failed, decide_record_reconciled, decide_record_unknown,
    decide_repropose_mutation, recover_prepared_from_history,
};
use tiber_session_service::{
    AssistantText, AssistantTextError, PromptText, PromptTextError, SessionBinding, SessionEvent,
    SessionFact, SessionServiceError, decide_observe_inference, decide_request_inference,
    decide_start_session, decide_succeed_session, project_started_session, task_assignment_scope,
};
use tiber_store_git::{
    GitStoreError, TiberEventStore, TransactionEventPage, TransactionHistoryError,
    publication::{TiberEventPublisher, TiberPublicationError},
};
use tiber_tasks_core::TaskId;
use tiber_tasks_service::TaskBoardProjection;
use tiber_tui::{ComposerIntent, ConversationProjection, ProjectionEvent};
use tiber_workflow_core::{
    AgentId, AssignmentEpoch, AssignmentId, AttemptNumber, ContextReceiptId, DeadlineMilliseconds,
    EffectId, EffectObservation, EffectReceiptId, HarnessError, HarnessState, IdempotencyKey,
    InferEffect, PolicyDecisionId, SessionId, WorkflowId,
};
use tiber_workflow_service::{
    WorkflowEvent, WorkflowServiceError, WorkflowStream, decide_advance_workflow,
    decide_initialize_successor_workflow, decide_initialize_workflow, decide_record_observation,
    decide_request_next_effect,
};
use tokio::runtime::Builder as RuntimeBuilder;

/// Reviewed isolated app-server configuration template.
const ISOLATED_CONFIG: &str = include_str!("../../../config/app-server.toml");
/// Maximum time the shell waits before checking terminal input again.
const TUI_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Maximum observations applied before terminal input is polled again.
const MAX_OBSERVATIONS_PER_FRAME: usize = 16;
#[expect(
    clippy::print_stderr,
    reason = "a command-line adapter intentionally writes its result and diagnostics"
)]
fn main() {
    let mut arguments = env::args_os();
    let _executable = arguments.next();
    let Some(command) = arguments.next() else {
        run_tui();
        return;
    };
    match command.to_string_lossy().as_ref() {
        "app-server-probe" => run_schema_probe(arguments),
        "auth" => run_auth(arguments),
        "converse" => run_conversation(arguments),
        "session" => run_session(arguments),
        "tasks" => run_tasks(arguments),
        "-h" | "--help" => {
            if arguments.next().is_some() {
                eprintln!("explicit help accepts no further arguments");
                usage();
                process::exit(2);
            }
            print_help();
        }
        _ => {
            eprintln!("unknown command: {}", command.to_string_lossy());
            usage();
            process::exit(2);
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the session query boundary renders one stable owner-facing absence diagnostic"
)]
/// Queries the active durable conversation for this repository.
#[expect(
    clippy::print_stdout,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
fn run_session(mut arguments: impl Iterator<Item = std::ffi::OsString>) {
    let Some(operation) = arguments.next() else {
        eprintln!("unknown command: session");
        usage();
        process::exit(2);
    };
    if matches!(operation.to_string_lossy().as_ref(), "-h" | "--help") && arguments.next().is_none()
    {
        println!("usage: tiber session active");
        return;
    }
    if operation != "active" || arguments.next().is_some() {
        eprintln!("unknown command: session");
        usage();
        process::exit(2);
    }
    let repository = env::current_dir().unwrap_or_else(|_source| {
        eprintln!("tiber_session_repository_unavailable: current directory could not be read");
        process::exit(1);
    });
    match load_session_history(&repository) {
        Ok(Some((binding, events, workflow_state))) => {
            print_session_binding(&binding, &events, workflow_state);
            if let Err(error) = print_repository_receipts(&repository, &events) {
                eprintln!(
                    "{}: signed repository receipt could not be read",
                    error.code()
                );
                process::exit(1);
            }
        }
        Ok(None) => {
            eprintln!("tiber_session_not_found: this repository has no active Tiber session");
            process::exit(1);
        }
        Err(error) => {
            eprintln!("{}: signed Tiber authority could not be read", error.code());
            process::exit(1);
        }
    }
}

/// Reads at most the one proven active-session start fact from signed authority.
#[expect(
    clippy::type_complexity,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
fn load_session_history(
    repository: &Path,
) -> Result<Option<(SessionBinding, Vec<SessionEvent>, &'static str)>, SessionQueryError> {
    let store = TiberEventStore::open(repository).map_err(SessionQueryError::Store)?;
    let events = read_session_events(&store)?;
    let events = active_session_events(&events).to_vec();
    let Some(binding) = events
        .iter()
        .rev()
        .find_map(|event| project_started_session(event).ok())
    else {
        return Ok(None);
    };
    let workflow_state = load_latest_workflow_state(&store, &events)?;
    Ok(Some((binding, events, workflow_state)))
}

/// Reads every durable session event from the fixed conversation stream.
fn read_session_events(store: &TiberEventStore) -> Result<Vec<SessionEvent>, SessionQueryError> {
    if cfg!(debug_assertions)
        && let Some(path) = env::var_os("TIBER_TEST_SESSION_HISTORY_READS")
    {
        use std::io::Write as _;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(SessionQueryError::DebugSentinel)?;
        file.write_all(b"read\n")
            .map_err(SessionQueryError::DebugSentinel)?;
    }
    let pattern = StreamPattern::try_new("tiber:session:active".to_owned())
        .map_err(|_source| SessionQueryError::InvalidStream)?;
    let reader = store
        .verified_transaction_reader::<SessionEvent>(&[pattern])
        .map_err(SessionQueryError::History)?;
    let mut page = TransactionEventPage::first(BatchSize::new(128));
    let mut all = Vec::new();
    loop {
        let events = reader.read_page(page).map_err(SessionQueryError::Page)?;
        let next = page.next_from_results(&events);
        all.extend(events);
        let Some(next) = next else {
            break;
        };
        page = next;
    }
    Ok(all)
}

#[expect(
    clippy::indexing_slicing,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
/// Returns the events belonging to the latest durable session binding.
fn active_session_events(events: &[SessionEvent]) -> &[SessionEvent] {
    let boundary = events.iter().rposition(|event| {
        matches!(
            event.fact(),
            SessionFact::SessionStarted { .. } | SessionFact::SessionSucceeded { .. }
        )
    });
    boundary.map_or(&[], |index| &events[index..])
}

/// Typed failures from the public active-session read boundary.
#[derive(Debug, thiserror::Error)]
enum SessionQueryError {
    /// Debug-only immutable-history read instrumentation could not be recorded.
    #[error("tiber_session_debug_history_sentinel_failed: {0}")]
    DebugSentinel(std::io::Error),
    /// The signed authority snapshot could not be opened.
    #[error(transparent)]
    Store(#[from] GitStoreError),
    /// The selected immutable transaction history was invalid.
    #[error(transparent)]
    History(#[from] TransactionHistoryError),
    /// A verified page could not be decoded from the immutable snapshot.
    #[error(transparent)]
    Page(#[from] EventStoreError),
    /// The fixed application-owned stream pattern was invalid.
    #[error("tiber_session_stream_invalid")]
    InvalidStream,
    /// The first durable session fact was not a session start.
    #[error(transparent)]
    Projection(#[from] tiber_session_service::SessionProjectionError),
    /// A workflow execution stream could not be constructed.
    #[error(transparent)]
    Workflow(#[from] WorkflowServiceError),
}

impl SessionQueryError {
    /// Returns the stable owner-facing failure code without discarding its cause.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the harness boundary preserves its closed recovery and projection control flow"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::DebugSentinel(_) => "tiber_session_debug_history_sentinel_failed",
            Self::Store(error) => error.code(),
            Self::History(error) => error.code(),
            Self::Page(_) => "tiber_store_event_history_payload_invalid",
            Self::InvalidStream => "tiber_session_stream_invalid",
            Self::Projection(error) => error.code(),
            Self::Workflow(error) => error.code(),
        }
    }
}

#[expect(
    clippy::print_stdout,
    reason = "the public session query renders its durable binding for the repository owner"
)]
/// Renders the complete currently proven task/workflow binding.
#[expect(
    clippy::match_same_arms,
    clippy::shadow_unrelated,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the projection renderer matches borrowed durable session facts"
)]
fn print_session_binding(
    binding: &SessionBinding,
    events: &[SessionEvent],
    workflow_state: &'static str,
) {
    let effect = binding.workflow_state().initial_effect();
    println!("task: {}", binding.task_id().as_str());
    println!("session: {}", effect.session_id().as_str());
    println!("workflow: {}", effect.workflow_id().as_str());
    println!("workflow-state: {workflow_state}");
    println!("assignment: {}", effect.assignment_id().as_str());
    println!(
        "next-action: {}",
        match workflow_state {
            "ready" | "completed" => "prompt",
            "observed" => "advance",
            "requested" | "initialized" | "missing" | "stopped" | "unknown" => "reconcile",
            _ => "reconcile",
        }
    );
    for event in events {
        if let SessionFact::InferenceRequested { prompt, effect, .. } = event.fact() {
            println!("effect: {}", effect.effect_id().as_str());
            println!("user: {}", prompt.as_str());
        }
        if let SessionFact::InferenceObserved { assistant, .. } = event.fact() {
            println!("assistant: {}", assistant.as_str());
        }
    }
}

/// Renders applied repository receipts owned by the active inference effect.
#[expect(
    clippy::print_stdout,
    reason = "the public session query renders durable applied receipts for the owner"
)]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::semicolon_if_nothing_returned,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn print_repository_receipts(
    repository: &Path,
    session_events: &[SessionEvent],
) -> Result<(), PromptPublicationError> {
    let Some(effect) = session_events
        .iter()
        .rev()
        .find_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect),
            _ => None,
        })
    else {
        return Ok(());
    };
    let pattern = StreamPattern::try_new(format!(
        "tiber:repository-mutation:{}",
        effect.effect_id().as_str()
    ))
    .map_err(|_error| SessionQueryError::InvalidStream)?;
    let store = TiberEventStore::open(repository)?;
    let reader = store.verified_transaction_reader::<RepositoryMutationEvent>(&[pattern])?;
    let mut page = TransactionEventPage::first(BatchSize::new(128));
    loop {
        let events = reader.read_page(page)?;
        for event in &events {
            match event.fact() {
                tiber_repository_service::RepositoryMutationFact::Proposed(proposal) => println!(
                    "repository change proposed: {} precondition: {}",
                    proposal.path().as_str(),
                    repository_precondition_text(proposal.precondition())
                ),
                tiber_repository_service::RepositoryMutationFact::Applied(receipt) => println!(
                    "repository change applied: {}",
                    receipt.identity().path().as_str()
                ),
                tiber_repository_service::RepositoryMutationFact::Denied(proposal) => {
                    println!("repository change denied: {}", proposal.path().as_str())
                }
                tiber_repository_service::RepositoryMutationFact::Cancelled(proposal) => {
                    println!("repository change cancelled: {}", proposal.path().as_str())
                }
                tiber_repository_service::RepositoryMutationFact::Reproposed(proposal) => {
                    println!(
                        "repository change reproposed: {} precondition: {}",
                        proposal.path().as_str(),
                        repository_precondition_text(proposal.precondition())
                    );
                }
                tiber_repository_service::RepositoryMutationFact::Approved(approval) => println!(
                    "repository change approved: {}",
                    approval.proposal().path().as_str()
                ),
                tiber_repository_service::RepositoryMutationFact::Prepared(identity) => {
                    println!("repository change prepared: {}", identity.path().as_str())
                }
                tiber_repository_service::RepositoryMutationFact::Failed(failure) => println!(
                    "repository change failed: {} {} retry: {}",
                    failure.identity().path().as_str(),
                    failure.error().code(),
                    repository_retryability_text(failure.retryability())
                ),
                tiber_repository_service::RepositoryMutationFact::Unknown(reconciliation) => {
                    println!(
                        "repository change unknown: {} retry: {}",
                        reconciliation.identity().path().as_str(),
                        repository_retryability_text(RepositoryRetryability::ReadOnlyRetryable)
                    )
                }
                tiber_repository_service::RepositoryMutationFact::Reconciled(outcome) => {
                    let (receipt, state) = match outcome {
                        RepositoryReconciliationOutcome::Applied(receipt) => (receipt, "applied"),
                        RepositoryReconciliationOutcome::NotApplied(receipt) => {
                            (receipt, "not-applied")
                        }
                        RepositoryReconciliationOutcome::StillUnknown(receipt) => {
                            (receipt, "still-unknown")
                        }
                    };
                    println!(
                        "repository change reconciled: {} {state}",
                        receipt.identity().path().as_str()
                    );
                }
                _ => {}
            }
        }
        let Some(next) = page.next_from_results(&events) else {
            break;
        };
        page = next;
    }
    Ok(())
}

/// Renders one safe repository precondition without exposing raw content.
fn repository_precondition_text(precondition: RepositoryMutationPrecondition) -> String {
    match precondition {
        RepositoryMutationPrecondition::Write(WritePrecondition::Absent) => "absent".to_owned(),
        RepositoryMutationPrecondition::Write(WritePrecondition::ExactDigest(digest))
        | RepositoryMutationPrecondition::Delete(digest) => digest.as_hex(),
    }
}

/// Returns the stable owner-facing label for repository retry guidance.
fn repository_retryability_text(retryability: RepositoryRetryability) -> &'static str {
    match retryability {
        RepositoryRetryability::FreshAuthorizationRequired => "fresh-authorization-required",
        RepositoryRetryability::ReadOnlyRetryable => "read-only-reconciliation-required",
    }
}

/// Replays the workflow stream for the active session's current effect.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the projection folds borrowed durable workflow facts"
)]
fn load_latest_workflow_state(
    store: &TiberEventStore,
    events: &[SessionEvent],
) -> Result<&'static str, SessionQueryError> {
    let Some(effect) = events.iter().rev().find_map(|event| match event.fact() {
        SessionFact::InferenceRequested { effect, .. } => Some(effect),
        _ => None,
    }) else {
        return Ok("ready");
    };
    let stream = WorkflowStream::for_effect(effect)?;
    let history = read_workflow_events_query(store, &stream)?;
    Ok(history
        .last()
        .map_or("missing", |event| match event.fact() {
            tiber_workflow_service::WorkflowFact::WorkflowInitialized { .. } => "initialized",
            tiber_workflow_service::WorkflowFact::EffectRequested { .. } => "requested",
            tiber_workflow_service::WorkflowFact::EffectObserved { .. } => "observed",
            tiber_workflow_service::WorkflowFact::WorkflowCompleted { .. } => "completed",
            tiber_workflow_service::WorkflowFact::WorkflowStopped { .. } => "stopped",
            _ => "unknown",
        }))
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "the command shell writes read-model results and stable owner-facing diagnostics"
)]
/// Runs one native task query or narrow signed task mutation against the current repository.
fn run_tasks(arguments: impl Iterator<Item = std::ffi::OsString>) {
    let command = match tasks::parse(arguments) {
        Ok(command) => command,
        Err(error) => exit_for_task_error(&error),
    };
    if let Some(output) = tasks::context_free_output(&command) {
        print!("{output}");
        return;
    }
    let repository = env::current_dir().unwrap_or_else(|_error| {
        eprintln!("tiber_tasks_repository_unavailable: current directory could not be read");
        process::exit(1);
    });
    match tasks::run(&repository, command) {
        Ok(output) => print!("{output}"),
        Err(error) => exit_for_task_error(&error),
    }
}

/// Renders one task failure and terminates with its stable command-line status.
#[expect(
    clippy::print_stderr,
    reason = "the task CLI failure boundary emits its stable owner-facing diagnostic"
)]
fn exit_for_task_error(error: &tasks::TaskCliError) -> ! {
    eprintln!("{}: {error}", error.code());
    if error.is_usage_error() {
        tasks_usage();
        process::exit(2);
    }
    process::exit(1);
}

/// Runs the interactive projection-only terminal presentation.
#[expect(
    clippy::print_stderr,
    reason = "terminal startup and adapter failures use stable owner-facing diagnostics"
)]
fn run_tui() {
    let repository = env::current_dir().unwrap_or_else(|error| {
        eprintln!("tiber_session_repository_unavailable: {error}");
        process::exit(1);
    });
    let (_binding, session_events) = ensure_started_session(&repository).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    cancel_lost_repository_proposal(&repository, &session_events).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    reconcile_prepared_repository_mutation(&repository, &session_events).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    let history = active_session_events(&session_events).to_vec();
    let store = TiberEventStore::open(&repository).unwrap_or_else(|error| {
        eprintln!("tiber_session_store_unavailable: {error}");
        process::exit(1);
    });
    let _workflow_state = load_latest_workflow_state(&store, &history).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    let mut projection = restored_conversation_projection(&history);
    apply_repository_restart_receipts(&repository, &history, &mut projection).unwrap_or_else(
        |error| {
            eprintln!("{}: {error}", error.code());
            process::exit(1);
        },
    );
    let mut terminal = ratatui::try_init().unwrap_or_else(|error| {
        eprintln!("tiber_tui_initialize_failed: {error}");
        process::exit(1);
    });
    let client = start_default_client();
    let mut worker = InferenceWorker::start(client);
    let result = run_tui_loop(&repository, &mut terminal, &mut worker, &mut projection);
    worker.stop();
    ratatui::restore();
    result.unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
}

/// Ensures the sole active native task has one durable conversation binding.
#[expect(
    clippy::collapsible_if,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only terminal native task statuses transfer session ownership"
)]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the startup decision matches borrowed durable session facts"
)]
fn ensure_started_session(
    repository: &Path,
) -> Result<(SessionBinding, Vec<SessionEvent>), SessionStartupError> {
    let store = TiberEventStore::open(repository)?;
    let revision = store.revision().clone();
    let mut session_events = read_session_events(&store)?;
    let task_history = tasks::read_history(&store)?;
    let task_projection = TaskBoardProjection::replay(&task_history)?;
    let actionable = task_projection.next_actionable_task()?;
    if let Some(binding) = session_events
        .iter()
        .rev()
        .find_map(|event| project_started_session(event).ok())
    {
        let active_events = active_session_events(&session_events);
        let unresolved = active_events
            .iter()
            .rev()
            .find_map(|event| match event.fact() {
                SessionFact::InferenceRequested { effect, .. } => Some(effect.effect_id()),
                _ => None,
            });
        if unresolved.is_some_and(|effect_id| {
            !active_events.iter().any(|event| {
                matches!(event.fact(), SessionFact::InferenceObserved { effect_id: observed, .. } if observed == effect_id)
            })
        }) {
            return Ok((binding, session_events));
        }
        if let Some(effect) = active_session_events(&session_events)
            .iter()
            .rev()
            .find_map(|event| match event.fact() {
                SessionFact::InferenceRequested { effect, .. } => Some(effect),
                _ => None,
            })
        {
            let stream = WorkflowStream::for_effect(effect)?;
            let workflow_history = read_workflow_events_query(&store, &stream)?;
            if matches!(
                workflow_history.last().map(WorkflowEvent::fact),
                Some(tiber_workflow_service::WorkflowFact::EffectObserved { .. })
            ) {
                let advance = decide_advance_workflow(&workflow_history, stream)?;
                let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
                let runtime = RuntimeBuilder::new_current_thread().build()?;
                let _completed = runtime.block_on(publisher.publish_workflow_advance(advance))?;
                return ensure_started_session(repository);
            }
        }
        if let Some(task) = actionable {
            if task.status == tiber_tasks_core::TaskStatus::InProgress
                && task.stem != *binding.task_id()
                && task_projection
                    .task(binding.task_id())
                    .is_some_and(|prior| {
                        matches!(
                            prior.status,
                            tiber_tasks_core::TaskStatus::Done
                                | tiber_tasks_core::TaskStatus::Abandoned
                        )
                    })
            {
                if recoverable_repository_effect(&store, active_session_events(&session_events))?
                    .is_some()
                {
                    return Ok((binding, session_events));
                }
                let successor = binding_for_task(&task.stem)?;
                let publication =
                    decide_succeed_session(&session_events, binding.clone(), successor.clone())?;
                let successor_event = publication.event().clone();
                let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
                let runtime = RuntimeBuilder::new_current_thread().build()?;
                let _published =
                    runtime.block_on(publisher.publish_session_successor(publication))?;
                session_events.push(successor_event);
                return Ok((successor, session_events));
            }
        }
        return Ok((binding, session_events));
    }
    let Some(task) = actionable else {
        return Err(SessionStartupError::NoEligibleTask);
    };
    let task_id = task.stem.clone();
    if task.status == tiber_tasks_core::TaskStatus::Backlog {
        drop(task_projection);
        drop(task_history);
        drop(store);
        tasks::start_task_by_id(repository, &task_id)?;
        return ensure_started_session(repository);
    }
    let binding = binding_for_task(&task_id)?;
    let Some(publication) = decide_start_session(&session_events, binding.clone())? else {
        return Ok((binding, session_events));
    };
    let started_event = publication.event().clone();
    let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let _published = runtime.block_on(publisher.publish_session_start(publication))?;
    session_events.push(started_event);
    Ok((binding, session_events))
}

#[expect(
    clippy::semicolon_if_nothing_returned,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
/// Reconstructs the active session transcript without granting effect authority.
#[expect(
    clippy::match_same_arms,
    reason = "session lifecycle facts deliberately leave the transcript projection unchanged"
)]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn restored_conversation_projection(events: &[SessionEvent]) -> ConversationProjection {
    let mut projection = ConversationProjection::new();
    for event in events {
        match event.fact() {
            SessionFact::SessionStarted { .. } => {}
            SessionFact::InferenceRequested { prompt, .. } => {
                projection.apply(ProjectionEvent::PromptSubmitted {
                    text: prompt.as_str().to_owned(),
                })
            }
            SessionFact::InferenceObserved { assistant, .. } => {
                projection.apply(ProjectionEvent::AssistantDelta {
                    text: assistant.as_str().to_owned(),
                });
                projection.apply(ProjectionEvent::TurnCompleted);
            }
            _ => {}
        }
    }
    let unresolved = events.iter().rev().find_map(|event| match event.fact() {
        SessionFact::InferenceRequested { effect, .. } => Some(effect.effect_id()),
        _ => None,
    });
    if unresolved.is_some_and(|effect_id| {
        !events.iter().any(|event| {
            matches!(event.fact(), SessionFact::InferenceObserved { effect_id: observed, .. } if observed == effect_id)
        })
    }) {
        projection.apply(ProjectionEvent::ReconciliationRequired {
            code: "session_inference_reconciliation_required".to_owned(),
            message: "durable inference outcome requires reconciliation before another prompt"
                .to_owned(),
        });
    }
    projection
}

/// Projects content-free repository recovery receipts for the active effect.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn apply_repository_restart_receipts(
    repository: &Path,
    session_events: &[SessionEvent],
    projection: &mut ConversationProjection,
) -> Result<(), PromptPublicationError> {
    let Some(effect) = session_events
        .iter()
        .rev()
        .find_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect),
            _ => None,
        })
    else {
        return Ok(());
    };
    let stream = RepositoryMutationStream::for_effect(effect.effect_id())?;
    let store = TiberEventStore::open(repository)?;
    let events = read_repository_mutation_events_with_limit(&store, &stream, Some(128))?;
    for event in events {
        match event.fact() {
            tiber_repository_service::RepositoryMutationFact::Cancelled(proposal) => {
                projection.apply(ProjectionEvent::RepositoryChangeCancelled {
                    path: proposal.path().as_str().to_owned(),
                });
            }
            tiber_repository_service::RepositoryMutationFact::Failed(failure) => {
                projection.apply(ProjectionEvent::RepositoryChangeFailed {
                    code: failure.error().code().to_owned(),
                    path: failure.identity().path().as_str().to_owned(),
                    retryable: matches!(
                        failure.retryability(),
                        RepositoryRetryability::ReadOnlyRetryable
                    ),
                });
            }
            tiber_repository_service::RepositoryMutationFact::Unknown(reconciliation) => {
                projection.apply(ProjectionEvent::RepositoryChangeUnknown {
                    path: reconciliation.identity().path().as_str().to_owned(),
                });
            }
            tiber_repository_service::RepositoryMutationFact::Reconciled(outcome) => {
                let (receipt, outcome) = match outcome {
                    RepositoryReconciliationOutcome::Applied(receipt) => (receipt, "applied"),
                    RepositoryReconciliationOutcome::NotApplied(receipt) => {
                        (receipt, "not-applied")
                    }
                    RepositoryReconciliationOutcome::StillUnknown(receipt) => {
                        (receipt, "still-unknown")
                    }
                };
                projection.apply(ProjectionEvent::RepositoryChangeReconciled {
                    outcome: outcome.to_owned(),
                    path: receipt.identity().path().as_str().to_owned(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Builds stable first-turn identities so an unchanged start can be reconciled.
#[expect(
    clippy::format_collect,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
fn binding_for_task(task_id: &TaskId) -> Result<SessionBinding, HarnessError> {
    let digest = Sha256::digest(task_id.as_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let session_text = format!("session-{digest}");
    let session = SessionId::parse(&session_text)?;
    let effect = InferEffect::new(
        session,
        AgentId::parse(&format!("agent-{digest}"))?,
        WorkflowId::parse(&format!("workflow-{digest}"))?,
        AssignmentId::parse(&format!("assignment-{digest}"))?,
        task_assignment_scope(task_id)?,
        AssignmentEpoch::FIRST,
        AttemptNumber::FIRST,
        ContextReceiptId::parse(&format!("context-{digest}-turn-1"))?,
        PolicyDecisionId::parse(&format!("policy-{digest}-turn-1"))?,
        EffectId::parse(&format!("effect-{digest}-turn-1"))?,
        IdempotencyKey::parse(&format!("{session_text}:turn-1"))?,
        DeadlineMilliseconds::parse(600_000)?,
    );
    Ok(SessionBinding::new(
        task_id.clone(),
        HarnessState::new(effect),
    ))
}

/// Typed startup failures retaining the complete authority and publication cause.
#[derive(Debug, thiserror::Error)]
enum SessionStartupError {
    /// Signed authority contains no active or eligible task to bind.
    #[error("no active or eligible backlog task is available")]
    NoEligibleTask,
    /// Signed authority could not be opened for the start decision.
    #[error(transparent)]
    Authority(#[from] GitStoreError),
    /// Existing session history could not be verified or decoded.
    #[error(transparent)]
    Query(#[from] SessionQueryError),
    /// Semantic workflow provenance could not be constructed.
    #[error(transparent)]
    Harness(#[from] HarnessError),
    /// Native task history could not be read or projected.
    #[error(transparent)]
    Tasks(#[from] tasks::TaskCliError),
    /// Task history violated its public projection contract.
    #[error(transparent)]
    TaskProjection(#[from] tiber_tasks_service::TaskProjectionError),
    /// Existing durable session history did not begin with its start fact.
    #[error(transparent)]
    SessionProjection(#[from] tiber_session_service::SessionProjectionError),
    /// Workflow authority could not be reconstructed or advanced.
    #[error(transparent)]
    Workflow(#[from] WorkflowServiceError),
    /// The command-specific modeled start could not be produced.
    #[error(transparent)]
    Session(#[from] SessionServiceError),
    /// Repository mutation authority must be recovered before session transfer.
    #[error(transparent)]
    Repository(#[from] PromptPublicationError),
    /// The signed publication boundary rejected or could not confirm the start.
    #[error(transparent)]
    Publication(#[from] TiberPublicationError),
    /// The bounded local executor could not be constructed.
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
}

impl SessionStartupError {
    /// Returns a stable owner-facing code without erasing the typed cause.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the code mapping borrows typed failures without taking their owned causes"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::NoEligibleTask => "tiber_session_no_eligible_task",
            Self::Authority(error) => error.code(),
            Self::Query(error) => error.code(),
            Self::Harness(error) => error.code(),
            Self::Tasks(error) => error.code(),
            Self::TaskProjection(error) => error.code(),
            Self::SessionProjection(error) => error.code(),
            Self::Workflow(error) => error.code(),
            Self::Session(error) => error.code(),
            Self::Repository(error) => error.code(),
            Self::Publication(error) => error.code(),
            Self::Runtime(_) => "tiber_session_runtime_unavailable",
        }
    }
}

/// Drives terminal intents and app-server observations without granting UI authority.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative terminal shell propagates sanitized I/O failures to one owner-facing boundary"
)]
#[expect(
    clippy::too_many_lines,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn run_tui_loop(
    repository: &Path,
    terminal: &mut ratatui::DefaultTerminal,
    worker: &mut InferenceWorker,
    projection: &mut ConversationProjection,
) -> Result<(), TuiRunError> {
    let mut dirty = true;
    let mut pending_repository_change = None;
    let mut repository_proposal_admitted = false;
    loop {
        for _observation in 0..MAX_OBSERVATIONS_PER_FRAME {
            match worker.observations.try_recv() {
                Ok(WorkerObservation::Projection(observation)) => {
                    projection.apply(observation);
                    dirty = true;
                }
                Ok(WorkerObservation::Completed { assistant }) => {
                    publish_inference_observation(repository, &assistant)
                        .map_err(TuiRunError::PromptPublication)?;
                    projection.apply(ProjectionEvent::TurnCompleted);
                    dirty = true;
                }
                Ok(WorkerObservation::AssistantRejected(error)) => {
                    return Err(TuiRunError::Assistant(error));
                }
                Ok(WorkerObservation::RepositoryProposal {
                    expected,
                    path,
                    replacement,
                }) => {
                    if !repository_proposal_admitted {
                        let (pending, diff) = publish_repository_proposal(
                            repository,
                            &path,
                            &expected,
                            &replacement,
                        )?;
                        projection.apply(ProjectionEvent::RepositoryChangeProposed { diff, path });
                        pending_repository_change = Some(pending);
                        repository_proposal_admitted = true;
                        dirty = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(TuiRunError::WorkerStopped);
                }
            }
        }
        if dirty {
            terminal
                .draw(|frame| tiber_tui::render(frame, projection))
                .map_err(TuiRunError::Terminal)?;
            dirty = false;
        }
        if !event::poll(TUI_POLL_INTERVAL).map_err(TuiRunError::Terminal)? {
            continue;
        }
        let input = event::read().map_err(TuiRunError::Terminal)?;
        let Event::Key(key) = input else {
            continue;
        };
        match projection.handle_key(key) {
            ComposerIntent::None => {}
            ComposerIntent::Quit => {
                worker.cancellation.cancel();
                return Ok(());
            }
            ComposerIntent::Approve => {
                let Some(pending) = pending_repository_change.take() else {
                    continue;
                };
                let path = pending.path.as_str().to_owned();
                match publish_approved_repository_change(repository, pending)? {
                    RepositoryApprovalResult::Applied => {
                        projection.apply(ProjectionEvent::RepositoryChangeApplied { path });
                    }
                    RepositoryApprovalResult::Reproposed { diff, pending } => {
                        projection.apply(ProjectionEvent::RepositoryChangeProposed { diff, path });
                        pending_repository_change = Some(pending);
                    }
                }
            }
            ComposerIntent::Deny => {
                let Some(pending) = pending_repository_change.take() else {
                    continue;
                };
                let path = pending.path.as_str().to_owned();
                publish_denied_repository_change(repository, pending)?;
                projection.apply(ProjectionEvent::RepositoryChangeDenied { path });
            }
            ComposerIntent::Cancel => {
                let Some(pending) = pending_repository_change.take() else {
                    continue;
                };
                let path = pending.path.as_str().to_owned();
                publish_cancelled_repository_change(repository, pending)?;
                projection.apply(ProjectionEvent::RepositoryChangeCancelled { path });
            }
            ComposerIntent::Submit(prompt) => {
                publish_prompt_request(repository, &prompt)
                    .map_err(TuiRunError::PromptPublication)?;
                projection.apply(ProjectionEvent::PromptSubmitted {
                    text: prompt.clone(),
                });
                worker
                    .submit(prompt)
                    .map_err(|_error| TuiRunError::WorkerStopped)?;
                repository_proposal_admitted = false;
            }
        }
        dirty = true;
    }
}

/// Publishes the exact prompt request before granting the worker execution authority.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the prompt publisher inspects only request and successor facts from borrowed durable history"
)]
fn publish_prompt_request(repository: &Path, prompt: &str) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let revision = store.revision().clone();
    let all_events = read_session_events(&store).map_err(PromptPublicationError::Query)?;
    let events = active_session_events(&all_events);
    let prompt = PromptText::parse(prompt)?;
    let binding = events
        .iter()
        .rev()
        .find_map(|event| project_started_session(event).ok())
        .ok_or(PromptPublicationError::MissingSession)?;
    let latest_effect = events.iter().rev().find_map(|event| match event.fact() {
        SessionFact::InferenceRequested { effect, .. } => Some(effect),
        _ => None,
    });
    let (effect, predecessor) = if let Some(previous) = latest_effect {
        let previous_stream = WorkflowStream::for_effect(previous)?;
        let previous_history = read_workflow_events_query(&store, &previous_stream)
            .map_err(PromptPublicationError::Query)?;
        let effect = previous_history
            .iter()
            .rev()
            .find_map(|event| match event.fact() {
                tiber_workflow_service::WorkflowFact::WorkflowCompleted { successor, .. } => {
                    Some(successor.initial_effect().clone())
                }
                _ => None,
            })
            .ok_or(PromptPublicationError::MissingSession)?;
        (effect, Some((previous_stream, previous_history)))
    } else {
        (binding.workflow_state().initial_effect().clone(), None)
    };
    let publication = decide_request_inference(&all_events, prompt, effect.clone())?;
    let workflow_stream = WorkflowStream::for_effect(&effect)?;
    let initialization = if let Some((previous_stream, previous_history)) = predecessor {
        decide_initialize_successor_workflow(
            &previous_history,
            previous_stream,
            workflow_stream.clone(),
        )?
    } else {
        decide_initialize_workflow(workflow_stream.clone(), HarnessState::new(effect))?
    };
    let request = decide_request_next_effect(
        core::slice::from_ref(initialization.event()),
        workflow_stream,
    )?;
    let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    runtime.block_on(publisher.publish_inference_request_with_workflow(
        publication,
        initialization,
        request,
    ))?;
    Ok(())
}

/// Publishes one validated assistant observation and terminal workflow advance.
#[expect(
    clippy::format_collect,
    clippy::shadow_unrelated,
    reason = "the observation boundary derives a bounded successor identity and opens a new publication stage"
)]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the observation publisher inspects only the pending borrowed request fact"
)]
fn publish_inference_observation(
    repository: &Path,
    assistant: &str,
) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let revision = store.revision().clone();
    let all_events = read_session_events(&store).map_err(PromptPublicationError::Query)?;
    let events = active_session_events(&all_events);
    let effect = events
        .iter()
        .rev()
        .find_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect.clone()),
            _ => None,
        })
        .ok_or(PromptPublicationError::MissingSession)?;
    let publication = decide_observe_inference(&all_events, AssistantText::parse(assistant)?)?;
    let workflow_stream = WorkflowStream::for_effect(&effect)?;
    let workflow_history = read_workflow_events_query(&store, &workflow_stream)
        .map_err(PromptPublicationError::Query)?;
    let receipt_digest =
        Sha256::digest(format!("{}:{assistant}", effect.effect_id().as_str()).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
    let observation = EffectObservation::Succeeded {
        effect_id: effect.effect_id().clone(),
        receipt_id: EffectReceiptId::parse(&format!("receipt-{receipt_digest}"))?,
    };
    let workflow_observation =
        decide_record_observation(&workflow_history, workflow_stream.clone(), observation)?;
    let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let observed_revision = runtime.block_on(
        publisher.publish_inference_observation_with_workflow(publication, workflow_observation),
    )?;
    let observed_store = TiberEventStore::open(repository)?;
    let observed_history = read_workflow_events_query(&observed_store, &workflow_stream)
        .map_err(PromptPublicationError::Query)?;
    let advance = decide_advance_workflow(&observed_history, workflow_stream)?;
    let mut publisher = TiberEventPublisher::open_at(repository, observed_revision.revision())?;
    let _completed = runtime.block_on(publisher.publish_workflow_advance(advance))?;
    Ok(())
}

/// Durably admits one exact, current-content repository proposal before display.
fn publish_repository_proposal(
    repository: &Path,
    path: &str,
    expected: &[u8],
    replacement: &[u8],
) -> Result<(PendingRepositoryChange, String), PromptPublicationError> {
    let path = RepositoryPath::parse(path)?;
    let current = read_bounded_repository_preimage(repository, path.as_str())?;
    if current != expected {
        return Err(PromptPublicationError::StaleRepositoryProposal);
    }
    let store = TiberEventStore::open(repository)?;
    let revision = store.revision().clone();
    let effect = active_inference_effect(&store)?;
    let provenance = repository_provenance(&effect);
    let repository_id = repository_id(repository)?;
    let assignment = RepositoryAssignmentContext::new(
        provenance.clone(),
        repository_id.clone(),
        ComponentScope::repository_root(),
    );
    let policy =
        RepositoryMutationPolicy::new(assignment.clone(), [RepositoryCapability::MutateRepository]);
    let proposal = RepositoryMutationProposal::write(
        provenance.clone(),
        repository_id.clone(),
        path.clone(),
        RepositoryContent::from_bytes(replacement)?,
        WritePrecondition::ExactDigest(Sha256Digest::of(expected)),
    );
    let stream = RepositoryMutationStream::new(&proposal.identity())?;
    let history = read_repository_mutation_events(&store, &stream)?;
    let publication = decide_propose_mutation(
        &history,
        stream.clone(),
        RepositoryMutationProposal::write(
            provenance,
            repository_id,
            path.clone(),
            RepositoryContent::from_bytes(replacement)?,
            WritePrecondition::ExactDigest(Sha256Digest::of(expected)),
        ),
    )?;
    if let Some(publication) = publication {
        let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
        let runtime = RuntimeBuilder::new_current_thread().build()?;
        let _published = runtime.block_on(publisher.publish_repository_mutation(publication))?;
    }
    let approval_id = OwnerApprovalId::parse(&format!(
        "approval-{}",
        Sha256Digest::of(effect.effect_id().as_str().as_bytes()).as_hex()
    ))?;
    let diff = repository_diff(path.as_str(), expected, replacement);
    Ok((
        PendingRepositoryChange {
            approval_id,
            assignment,
            expected: expected.to_vec(),
            path,
            policy,
            proposal,
            replacement: replacement.to_vec(),
            stream,
        },
        diff,
    ))
}

/// Publishes approval and preparation, dispatches once, then signs one terminal outcome.
#[expect(
    clippy::collapsible_if,
    clippy::shadow_unrelated,
    clippy::too_many_lines,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn publish_approved_repository_change(
    repository: &Path,
    pending: PendingRepositoryChange,
) -> Result<RepositoryApprovalResult, PromptPublicationError> {
    let PendingRepositoryChange {
        approval_id,
        assignment,
        expected,
        path,
        policy,
        proposal,
        replacement,
        stream,
    } = pending;
    let current = read_bounded_repository_preimage(repository, path.as_str())?;
    if current != expected {
        let provenance = proposal.identity().provenance().clone();
        let replacement_proposal = RepositoryMutationProposal::write(
            provenance.clone(),
            assignment.repository_id().clone(),
            path.clone(),
            RepositoryContent::from_bytes(&replacement)?,
            WritePrecondition::ExactDigest(Sha256Digest::of(&current)),
        );
        let store = TiberEventStore::open(repository)?;
        let history = read_repository_mutation_events(&store, &stream)?;
        let reproposed = decide_repropose_mutation(
            &history,
            stream.clone(),
            RepositoryMutationProposal::write(
                provenance,
                assignment.repository_id().clone(),
                path.clone(),
                RepositoryContent::from_bytes(&replacement)?,
                WritePrecondition::ExactDigest(Sha256Digest::of(&current)),
            ),
        )?;
        let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
        let runtime = RuntimeBuilder::new_current_thread().build()?;
        let _reproposed_revision =
            runtime.block_on(publisher.publish_repository_mutation(reproposed))?;
        let diff = repository_diff(path.as_str(), &current, &replacement);
        return Ok(RepositoryApprovalResult::Reproposed {
            diff,
            pending: PendingRepositoryChange {
                approval_id,
                assignment,
                expected: current,
                path,
                policy,
                proposal: replacement_proposal,
                replacement,
                stream,
            },
        });
    }
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let store = TiberEventStore::open(repository)?;
    let active_provenance = repository_provenance(&active_inference_effect(&store)?);
    let history = read_repository_mutation_events(&store, &stream)?;
    let approval_and_preparation = decide_approve_and_prepare_mutation(
        &history,
        stream.clone(),
        &proposal,
        &assignment,
        &policy,
        active_provenance,
        approval_id,
    )?;
    let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
    let _prepared_revision = runtime.block_on(
        publisher.publish_approved_and_prepared_repository_mutation(approval_and_preparation),
    )?;
    if cfg!(debug_assertions) {
        if let Some(sentinel) = env::var_os("TIBER_TEST_CRASH_AFTER_APPROVED_SENTINEL") {
            fs::write(sentinel, b"approved\n")?;
            process::exit(85);
        }
    }
    if cfg!(debug_assertions) {
        if let Some(sentinel) = env::var_os("TIBER_TEST_CRASH_AFTER_PREPARED_SENTINEL") {
            fs::write(sentinel, b"prepared\n")?;
            process::exit(86);
        }
    }

    let store = TiberEventStore::open(repository)?;
    let history = read_repository_mutation_events(&store, &stream)?;
    let authority = tiber_repository_service::authorize_prepared_mutation(
        &history,
        proposal,
        &assignment,
        &policy,
    )?;
    let service = LinuxRepositoryService::new(repository_service_config(repository)?);
    record_test_repository_worker_invocation(b"dispatch\n")?;
    let outcome = runtime.block_on(service.dispatch(authority));

    let store = TiberEventStore::open(repository)?;
    let history = read_repository_mutation_events(&store, &stream)?;
    let (terminal, result) = match outcome {
        Ok(RepositoryDispatchOutcome::Applied(receipt)) => {
            (decide_record_applied(&history, stream, receipt)?, Ok(()))
        }
        Ok(RepositoryDispatchOutcome::OutcomeUnknown(reconciliation)) => (
            decide_record_unknown(&history, stream, reconciliation)?,
            Err(PromptPublicationError::RepositoryOutcomeUnknown),
        ),
        Err(failure) => {
            let code = failure.error();
            let retryability = failure.retryability();
            (
                decide_record_failed(&history, stream, failure)?,
                Err(PromptPublicationError::RepositoryDispatchFailed { code, retryability }),
            )
        }
    };
    let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
    let _terminal_revision = runtime.block_on(publisher.publish_repository_mutation(terminal))?;
    result.map(|()| RepositoryApprovalResult::Applied)
}

/// Durably records explicit denial of the exact active proposal without dispatch.
fn publish_denied_repository_change(
    repository: &Path,
    pending: PendingRepositoryChange,
) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let active_provenance = repository_provenance(&active_inference_effect(&store)?);
    let history = read_repository_mutation_events(&store, &pending.stream)?;
    let denial = decide_deny_mutation(
        &history,
        pending.stream,
        pending.proposal.identity(),
        active_provenance,
    )?;
    let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let _denied_revision = runtime.block_on(publisher.publish_repository_mutation(denial))?;
    Ok(())
}

/// Durably records explicit cancellation of the exact active proposal without dispatch.
fn publish_cancelled_repository_change(
    repository: &Path,
    pending: PendingRepositoryChange,
) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let active_provenance = repository_provenance(&active_inference_effect(&store)?);
    let history = read_repository_mutation_events(&store, &pending.stream)?;
    let cancellation = decide_cancel_mutation(
        &history,
        pending.stream,
        pending.proposal.identity(),
        active_provenance,
    )?;
    let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let _cancelled_revision =
        runtime.block_on(publisher.publish_repository_mutation(cancellation))?;
    Ok(())
}

/// Terminates a signed proposal whose raw decision bytes were lost on restart.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn cancel_lost_repository_proposal(
    repository: &Path,
    session_events: &[SessionEvent],
) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let active_events = active_session_events(session_events);
    let Some(effect) = active_events
        .iter()
        .rev()
        .find_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect),
            _ => None,
        })
    else {
        return Ok(());
    };
    let stream = RepositoryMutationStream::for_effect(effect.effect_id())?;
    let history = read_repository_mutation_events(&store, &stream)?;
    let Some(cancellation) = decide_cancel_open_proposal_on_restart(&history, stream)? else {
        return Ok(());
    };
    let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let _cancelled_revision =
        runtime.block_on(publisher.publish_repository_mutation(cancellation))?;
    Ok(())
}

/// Reconciles one signed Prepared/Unknown mutation exactly once on startup.
fn reconcile_prepared_repository_mutation(
    repository: &Path,
    session_events: &[SessionEvent],
) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let Some(effect) =
        recoverable_repository_effect(&store, active_session_events(session_events))?
    else {
        return Ok(());
    };
    let stream = RepositoryMutationStream::for_effect(effect.effect_id())?;
    let mut history = read_repository_mutation_events(&store, &stream)?;
    let Some(reconciliation) = recover_prepared_from_history(&history, &stream)? else {
        return Ok(());
    };
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    if !history.iter().any(|event| {
        matches!(
            event.fact(),
            tiber_repository_service::RepositoryMutationFact::Unknown(_)
        )
    }) {
        let unknown = decide_record_unknown(&history, stream.clone(), reconciliation.clone())?;
        let mut publisher = TiberEventPublisher::open_at(repository, store.revision())?;
        let _unknown_revision = runtime.block_on(publisher.publish_repository_mutation(unknown))?;
        let refreshed_store = TiberEventStore::open(repository)?;
        history = read_repository_mutation_events(&refreshed_store, &stream)?;
    }
    let service = LinuxRepositoryService::new(repository_service_config(repository)?);
    record_test_repository_worker_invocation(b"reconcile\n")?;
    let outcome = runtime
        .block_on(service.reconcile(reconciliation))
        .map_err(|_failure| PromptPublicationError::RepositoryReconciliationFailed)?;
    let publication = decide_record_reconciled(&history, stream, outcome)?;
    let latest_store = TiberEventStore::open(repository)?;
    let mut publisher = TiberEventPublisher::open_at(repository, latest_store.revision())?;
    let _reconciled_revision =
        runtime.block_on(publisher.publish_repository_mutation(publication))?;
    Ok(())
}

/// Finds unresolved mutation authority by folding each exact effect-owned stream.
#[expect(
    clippy::unseparated_literal_suffix,
    reason = "workspace policy denies both conflicting literal-suffix styles; this function uses Rust's conventional attached suffix"
)]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn recoverable_repository_effect(
    store: &TiberEventStore,
    session_events: &[SessionEvent],
) -> Result<Option<InferEffect>, PromptPublicationError> {
    const MAX_RECOVERY_CANDIDATES: usize = 64;
    const MAX_RECOVERY_EVENTS_PER_STREAM: usize = 128;
    let observed_effects = session_events
        .iter()
        .filter_map(|event| match event.fact() {
            SessionFact::InferenceObserved { effect_id, .. } => Some(effect_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut recovery_candidates = 0usize;
    let mut newest_request = true;
    for effect in session_events
        .iter()
        .rev()
        .filter_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect),
            _ => None,
        })
    {
        let observed = observed_effects.contains(effect.effect_id().as_str());
        let inspect_observed = newest_request;
        newest_request = false;
        if observed && !inspect_observed {
            continue;
        }
        if !observed && recovery_candidates >= MAX_RECOVERY_CANDIDATES {
            return Err(PromptPublicationError::RepositoryRecoveryBudgetExceeded);
        }
        let stream = RepositoryMutationStream::for_effect(effect.effect_id())?;
        let history = read_repository_mutation_events_with_limit(
            store,
            &stream,
            Some(MAX_RECOVERY_EVENTS_PER_STREAM),
        )?;
        let recovery = recover_prepared_from_history(&history, &stream)?;
        if observed && recovery.is_none() {
            continue;
        }
        if recovery_candidates >= MAX_RECOVERY_CANDIDATES {
            return Err(PromptPublicationError::RepositoryRecoveryBudgetExceeded);
        }
        recovery_candidates = recovery_candidates.saturating_add(1);
        if recovery.is_some() {
            return Ok(Some(effect.clone()));
        }
    }
    Ok(None)
}

/// Records one deterministic debug-fixture adapter invocation at the exact call boundary.
#[expect(
    clippy::collapsible_if,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn record_test_repository_worker_invocation(
    operation: &[u8],
) -> Result<(), PromptPublicationError> {
    if cfg!(debug_assertions) {
        if let Some(path) = env::var_os("TIBER_TEST_REPOSITORY_WORKER_INVOCATIONS") {
            use std::io::Write as _;

            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            file.write_all(operation)?;
        }
    }
    Ok(())
}

/// Reads verified repository-mutation history for one effect-owned stream.
fn read_repository_mutation_events(
    store: &TiberEventStore,
    stream: &RepositoryMutationStream,
) -> Result<Vec<RepositoryMutationEvent>, PromptPublicationError> {
    read_repository_mutation_events_with_limit(store, stream, None)
}

/// Reads one exact mutation stream while enforcing an optional startup bound.
fn read_repository_mutation_events_with_limit(
    store: &TiberEventStore,
    stream: &RepositoryMutationStream,
    limit: Option<usize>,
) -> Result<Vec<RepositoryMutationEvent>, PromptPublicationError> {
    use eventcore::model::StreamIdentity as _;

    let pattern = StreamPattern::try_new(stream.as_stream_id().as_ref().to_owned())
        .map_err(|_error| SessionQueryError::InvalidStream)?;
    let reader = store.verified_transaction_reader::<RepositoryMutationEvent>(&[pattern])?;
    let mut page = TransactionEventPage::first(BatchSize::new(128));
    let mut all = Vec::new();
    loop {
        let events = reader.read_page(page)?;
        if limit.is_some_and(|limit| all.len().saturating_add(events.len()) > limit) {
            return Err(PromptPublicationError::RepositoryRecoveryBudgetExceeded);
        }
        let next = page.next_from_results(&events);
        all.extend(events);
        let Some(next) = next else {
            break;
        };
        page = next;
    }
    Ok(all)
}

/// Returns the active durable inference effect that owns repository provenance.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn active_inference_effect(store: &TiberEventStore) -> Result<InferEffect, PromptPublicationError> {
    read_session_events(store)
        .map_err(PromptPublicationError::Query)?
        .into_iter()
        .rev()
        .find_map(|event| match event.fact() {
            SessionFact::InferenceRequested { effect, .. } => Some(effect.clone()),
            _ => None,
        })
        .ok_or(PromptPublicationError::MissingSession)
}

/// Copies the complete active inference provenance into repository authority.
fn repository_provenance(effect: &InferEffect) -> RepositoryMutationProvenance {
    RepositoryMutationProvenance::new(
        effect.session_id().clone(),
        effect.agent_id().clone(),
        effect.workflow_id().clone(),
        effect.assignment_id().clone(),
        effect.assignment_scope().clone(),
        effect.assignment_epoch(),
        effect.attempt_number(),
        effect.context_receipt_id().clone(),
        effect.policy_decision_id().clone(),
        effect.effect_id().clone(),
        effect.idempotency_key().clone(),
        effect.deadline_milliseconds(),
    )
}

/// Derives one stable repository identity from the trusted canonical root.
fn repository_id(repository: &Path) -> Result<RepositoryId, PromptPublicationError> {
    let canonical = repository.canonicalize()?;
    RepositoryId::parse(&format!(
        "repository-{}",
        Sha256Digest::of(canonical.as_os_str().as_encoded_bytes()).as_hex()
    ))
    .map_err(PromptPublicationError::Repository)
}

/// Resolves packaged sibling repository helpers beside the running executable.
fn resolve_repository_helper_paths(
    executable: &Path,
    path_bubblewrap: Option<PathBuf>,
) -> Option<(PathBuf, PathBuf)> {
    let helper_directory = executable.parent()?;
    let sibling_bubblewrap = helper_directory.join("bwrap");
    let bubblewrap = if sibling_bubblewrap.is_file() {
        sibling_bubblewrap
    } else {
        path_bubblewrap?
    };
    Some((bubblewrap, helper_directory.join("tiber-repository-worker")))
}

/// Resolves fixed package-owned Linux adapter paths and private state.
fn repository_service_config(
    repository: &Path,
) -> Result<LinuxRepositoryServiceConfig, PromptPublicationError> {
    let repository_root = repository.canonicalize()?;
    let repository_id = repository_id(&repository_root)?;
    let path_bubblewrap = resolve_executable("bwrap");
    let executable = env::current_exe()?;
    let (bubblewrap, installed_worker) =
        resolve_repository_helper_paths(&executable, path_bubblewrap).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "repository helper directory unavailable",
            )
        })?;
    let worker = if cfg!(debug_assertions) {
        env::var_os("TIBER_TEST_REPOSITORY_WORKER")
            .map(PathBuf::from)
            .or(Some(installed_worker.clone()))
    } else {
        Some(installed_worker)
    }
    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "worker path unavailable"))?;
    let state_base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "state home unavailable")
        })?;
    LinuxRepositoryServiceConfig::new(repository_id, repository_root, bubblewrap, worker)?
        .with_state_root(state_base.join("tiber/repository-mutations"))
        .map_err(PromptPublicationError::RepositoryConfiguration)
}

/// Reads at most one byte beyond the supported proposal preimage bound.
fn read_bounded_repository_preimage(
    repository: &Path,
    path: &str,
) -> Result<Vec<u8>, PromptPublicationError> {
    const MAX_PREIMAGE_BYTES: usize = 64 * 1024;

    let root = open(
        repository,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_error| PromptPublicationError::RepositoryPreimageUnsafe)?;
    let target = openat2(
        &root,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|_error| PromptPublicationError::RepositoryPreimageUnsafe)?;
    let stat = fstat(&target).map_err(|_error| PromptPublicationError::RepositoryPreimageUnsafe)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(PromptPublicationError::RepositoryPreimageUnsafe);
    }
    let file = fs::File::from(target);
    let mut bytes = Vec::with_capacity(MAX_PREIMAGE_BYTES + 1);
    file.take(u64::try_from(MAX_PREIMAGE_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PREIMAGE_BYTES {
        return Err(PromptPublicationError::RepositoryPreimageTooLarge);
    }
    if std::str::from_utf8(&bytes).is_err() {
        return Err(PromptPublicationError::RepositoryPreimageUnsupportedEncoding);
    }
    Ok(bytes)
}

/// Renders one exact byte image as independently marked, escaped records.
#[expect(
    clippy::default_numeric_fallback,
    clippy::indexing_slicing,
    clippy::pattern_type_mismatch,
    reason = "this imperative CLI boundary keeps the closed typed lifecycle projection and owner-facing control flow explicit"
)]
fn repository_diff_records(marker: char, bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut rendered = String::new();
    for record in bytes
        .split_inclusive(|byte| *byte == b'\n')
        .chain(bytes.is_empty().then_some(bytes))
    {
        rendered.push(marker);
        rendered.push_str("b\"");
        for byte in record {
            match byte {
                b'\\' => rendered.push_str("\\\\"),
                b'\"' => rendered.push_str("\\\""),
                0x20..=0x7e => rendered.push(char::from(*byte)),
                _ => {
                    rendered.push_str("\\x");
                    rendered.push(char::from(HEX[usize::from(*byte >> 4)]));
                    rendered.push(char::from(HEX[usize::from(*byte & 0x0f)]));
                }
            }
        }
        rendered.push_str("\"\n");
    }
    rendered
}

/// Renders the exact reread proposal bytes as a bounded, byte-faithful diff.
fn repository_diff(path: &str, expected: &[u8], replacement: &[u8]) -> String {
    format!(
        "--- a/{path}\n+++ b/{path}\n{}{}",
        repository_diff_records('-', expected),
        repository_diff_records('+', replacement)
    )
}

/// Reads the complete workflow history for one effect-bound stream.
fn read_workflow_events_query(
    store: &TiberEventStore,
    stream: &WorkflowStream,
) -> Result<Vec<WorkflowEvent>, SessionQueryError> {
    let pattern = StreamPattern::try_new(stream.stream_id().as_ref().to_owned())
        .map_err(|_error| SessionQueryError::InvalidStream)?;
    let reader = store
        .verified_transaction_reader::<WorkflowEvent>(&[pattern])
        .map_err(SessionQueryError::History)?;
    let mut page = TransactionEventPage::first(BatchSize::new(128));
    let mut all = Vec::new();
    loop {
        let events = reader.read_page(page).map_err(SessionQueryError::Page)?;
        let next = page.next_from_results(&events);
        all.extend(events);
        let Some(next) = next else {
            break;
        };
        page = next;
    }
    Ok(all)
}

#[derive(Debug, thiserror::Error)]
/// Typed failures from durable prompt and observation publication.
enum PromptPublicationError {
    /// Git-backed store access failed.
    #[error(transparent)]
    Store(#[from] GitStoreError),
    /// Verified history replay failed.
    #[error(transparent)]
    History(#[from] TransactionHistoryError),
    /// Event-store paging failed.
    #[error(transparent)]
    Page(#[from] EventStoreError),
    /// Owner prompt validation failed.
    #[error(transparent)]
    Prompt(#[from] PromptTextError),
    /// Assistant response validation failed.
    #[error(transparent)]
    Assistant(#[from] AssistantTextError),
    /// Session command modeling failed.
    #[error(transparent)]
    Session(#[from] SessionServiceError),
    /// Signed authority publication failed.
    #[error(transparent)]
    Publication(#[from] TiberPublicationError),
    /// Runtime construction failed.
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
    /// No durable session owns the prompt.
    #[error("durable session start is missing")]
    MissingSession,
    /// Pure workflow planning failed.
    #[error(transparent)]
    Harness(#[from] HarnessError),
    /// Workflow command modeling failed.
    #[error(transparent)]
    Workflow(#[from] WorkflowServiceError),
    /// Durable session query failed.
    #[error(transparent)]
    Query(#[from] SessionQueryError),
    /// Repository proposal parsing or policy validation failed.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    /// Repository lifecycle modeling rejected the transition.
    #[error(transparent)]
    RepositoryMutation(#[from] RepositoryMutationServiceError),
    /// The fixed Linux adapter configuration was invalid.
    #[error(transparent)]
    RepositoryConfiguration(#[from] LinuxRepositoryConfigurationError),
    /// The proposal's exact expected bytes no longer match the requested file.
    #[error("repository proposal is stale")]
    StaleRepositoryProposal,
    /// The selected repository preimage exceeds the bounded proposal limit.
    #[error("repository proposal preimage exceeds 64 KiB")]
    RepositoryPreimageTooLarge,
    /// Repository proposals intentionally support UTF-8 text preimages only.
    #[error("repository proposal preimage is not valid UTF-8 text")]
    RepositoryPreimageUnsupportedEncoding,
    /// Startup recovery exceeded its explicit candidate or event budget.
    #[error("repository mutation recovery history exceeds the bounded startup budget")]
    RepositoryRecoveryBudgetExceeded,
    /// The selected preimage is not a confined, non-symlink regular file.
    #[error("repository proposal preimage is not a confined regular file")]
    RepositoryPreimageUnsafe,
    /// The adapter definitively rejected the prepared mutation after recording failure.
    #[error("{code}: repository mutation was not applied; retry: {retryability}")]
    RepositoryDispatchFailed {
        /// Stable closed adapter failure code.
        code: RepositoryMutationFailureCode,
        /// Safe retry directive retained from the consumed authority.
        retryability: RepositoryRetryability,
    },
    /// The adapter outcome is durably unknown and requires later reconciliation.
    #[error("repository mutation outcome is unknown")]
    RepositoryOutcomeUnknown,
    /// Read-only adapter reconciliation could not establish a durable outcome.
    #[error("repository reconciliation query failed")]
    RepositoryReconciliationFailed,
}

impl PromptPublicationError {
    /// Returns the stable owner-facing failure code.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the code mapping borrows typed failures without taking their owned causes"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::Store(error) => error.code(),
            Self::History(error) => error.code(),
            Self::Page(_) => "tiber_store_event_history_payload_invalid",
            Self::Prompt(error) => error.code(),
            Self::Assistant(error) => error.code(),
            Self::Session(error) => error.code(),
            Self::Publication(error) => error.code(),
            Self::Runtime(_) => "tiber_session_runtime_unavailable",
            Self::MissingSession => "tiber_session_not_found",
            Self::Harness(error) => error.code(),
            Self::Workflow(error) => error.code(),
            Self::Query(error) => error.code(),
            Self::Repository(error) => error.code(),
            Self::RepositoryMutation(error) => error.code(),
            Self::RepositoryConfiguration(error) => error.code(),
            Self::StaleRepositoryProposal => "repository_mutation_stale_proposal",
            Self::RepositoryPreimageTooLarge => "repository_mutation_preimage_too_large",
            Self::RepositoryPreimageUnsupportedEncoding => {
                "repository_mutation_preimage_unsupported_encoding"
            }
            Self::RepositoryRecoveryBudgetExceeded => {
                "repository_mutation_recovery_budget_exceeded"
            }
            Self::RepositoryPreimageUnsafe => "repository_mutation_preimage_unsafe",
            Self::RepositoryDispatchFailed { code, .. } => code.code(),
            Self::RepositoryOutcomeUnknown => "repository_mutation_outcome_unknown",
            Self::RepositoryReconciliationFailed => "repository_reconciliation_failed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Typed failures returned by the terminal application loop.
enum TuiRunError {
    /// The inference worker ended before the terminal loop requested shutdown.
    #[error("inference worker stopped unexpectedly")]
    WorkerStopped,
    /// Terminal input or rendering failed.
    #[error("terminal I/O failed: {0}")]
    Terminal(std::io::Error),
    /// Durable prompt or observation publication failed.
    #[error(transparent)]
    PromptPublication(#[from] PromptPublicationError),
    /// Assistant output failed semantic validation.
    #[error(transparent)]
    Assistant(#[from] AssistantTextError),
}

impl TuiRunError {
    /// Returns the stable owner-facing failure code.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the code mapping borrows typed failures without taking their owned causes"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::WorkerStopped => "tiber_inference_worker_stopped",
            Self::Terminal(_) => "tiber_tui_io_failed",
            Self::PromptPublication(error) => error.code(),
            Self::Assistant(error) => error.code(),
        }
    }
}

/// One exact durable proposal awaiting explicit owner approval.
struct PendingRepositoryChange {
    /// Exact owner-approval identifier minted for this proposal.
    approval_id: OwnerApprovalId,
    /// Repository assignment that scopes mutation authority.
    assignment: RepositoryAssignmentContext,
    /// Exact preimage bytes shown to the owner.
    expected: Vec<u8>,
    /// Validated repository-relative target path.
    path: RepositoryPath,
    /// Policy snapshot governing the proposed mutation.
    policy: RepositoryMutationPolicy,
    /// Exact safe proposal awaiting a decision.
    proposal: RepositoryMutationProposal,
    /// Replacement bytes shown in the approval diff.
    replacement: Vec<u8>,
    /// Owning mutation stream used for durable consistency.
    stream: RepositoryMutationStream,
}

/// Outcome of one explicit repository approval attempt.
#[expect(
    clippy::large_enum_variant,
    reason = "the two closed owner outcomes stay directly typed and avoid heap allocation in the interactive shell"
)]
enum RepositoryApprovalResult {
    /// The exact current proposal was prepared and applied.
    Applied,
    /// Current bytes changed, so a replacement proposal awaits fresh approval.
    Reproposed {
        /// Exact byte-faithful replacement diff shown to the owner.
        diff: String,
        /// Fresh proposal that now requires another explicit decision.
        pending: PendingRepositoryChange,
    },
}

/// Commands accepted by the inference-owning imperative worker.
enum InferenceCommand {
    /// Start one owner-submitted turn.
    Submit(String),
    /// Stop after cancelling any active operation.
    Stop,
}

/// Keeps blocking app-server protocol work outside the terminal input loop.
struct InferenceWorker {
    /// Command channel owned by the terminal shell.
    commands: SyncSender<InferenceCommand>,
    /// Presentation-only observations returned by the adapter owner.
    observations: Receiver<WorkerObservation>,
    /// Cooperative cancellation observed during bounded protocol waits.
    cancellation: OperationCancellation,
    /// Worker lifecycle handle.
    thread: Option<JoinHandle<()>>,
}

/// Presentation-safe observations returned by the inference worker.
enum WorkerObservation {
    /// A projection event safe to apply immediately.
    Projection(ProjectionEvent),
    /// A complete assistant response ready for durable publication.
    Completed {
        /// Complete validated assistant text.
        assistant: String,
    },
    /// Assistant output rejected at its semantic boundary.
    AssistantRejected(AssistantTextError),
    /// One closed repository write proposal parsed from the app-server tool boundary.
    RepositoryProposal {
        /// Exact bytes the model reports observing before proposing the change.
        expected: Vec<u8>,
        /// Root-relative repository path selected by the proposal.
        path: String,
        /// Exact replacement bytes proposed for the target.
        replacement: Vec<u8>,
    },
}

impl InferenceWorker {
    /// Starts one worker that exclusively owns the app-server client.
    #[expect(
        clippy::too_many_lines,
        reason = "the harness boundary preserves its closed recovery and projection control flow"
    )]
    fn start(mut client: AppServerClient) -> Self {
        let cancellation = client.cancellation_handle();
        let worker_cancellation = cancellation.clone();
        let (command_sender, command_receiver) = mpsc::sync_channel(1);
        let (observation_sender, observations) = mpsc::sync_channel(32);
        let thread = thread::spawn(move || {
            while let Ok(command) = command_receiver.recv() {
                let InferenceCommand::Submit(prompt) = command else {
                    break;
                };
                let turn = match client.start_turn(&prompt) {
                    Ok(turn) => turn,
                    Err(error) => {
                        if !send_observation(
                            &observation_sender,
                            &worker_cancellation,
                            WorkerObservation::Projection(
                                ProjectionEvent::ReconciliationRequired {
                                    code: error.code().to_owned(),
                                    message: error.to_string(),
                                },
                            ),
                        ) {
                            break;
                        }
                        continue;
                    }
                };
                let mut assistant = String::new();
                loop {
                    let (observation, terminal) = match client
                        .poll_turn_event(&turn, TUI_POLL_INTERVAL)
                    {
                        Ok(None) => continue,
                        Ok(Some(TurnEvent::AssistantDelta(text))) => {
                            if let Err(error) = AssistantText::parse(&text) {
                                if !send_observation(
                                    &observation_sender,
                                    &worker_cancellation,
                                    WorkerObservation::AssistantRejected(error),
                                ) {
                                    break;
                                }
                                break;
                            }
                            if assistant.len().saturating_add(text.len()) > AssistantText::MAX_BYTES
                            {
                                let observation = WorkerObservation::AssistantRejected(
                                    AssistantTextError::TooLarge,
                                );
                                if !send_observation(
                                    &observation_sender,
                                    &worker_cancellation,
                                    observation,
                                ) {
                                    break;
                                }
                                break;
                            }
                            assistant.push_str(&text);
                            (
                                WorkerObservation::Projection(ProjectionEvent::AssistantDelta {
                                    text,
                                }),
                                false,
                            )
                        }
                        Ok(Some(TurnEvent::InertToolRequested(request))) => {
                            let repository_proposal = (request.tool
                                == TIBER_REPOSITORY_PROPOSAL_TOOL_NAME
                                && request
                                    .arguments
                                    .get("action")
                                    .and_then(|value| value.as_str())
                                    == Some("write"))
                            .then(|| {
                                Some(WorkerObservation::RepositoryProposal {
                                    expected: request
                                        .arguments
                                        .get("expected")?
                                        .as_str()?
                                        .as_bytes()
                                        .to_vec(),
                                    path: request.arguments.get("path")?.as_str()?.to_owned(),
                                    replacement: request
                                        .arguments
                                        .get("replacement")?
                                        .as_str()?
                                        .as_bytes()
                                        .to_vec(),
                                })
                            })
                            .flatten();
                            (
                                repository_proposal.unwrap_or({
                                    WorkerObservation::Projection(
                                        ProjectionEvent::InertToolRequested {
                                            arguments: request.arguments,
                                            call_id: request.call_id,
                                            tool: request.tool,
                                        },
                                    )
                                }),
                                false,
                            )
                        }
                        Ok(Some(TurnEvent::Completed)) => (
                            WorkerObservation::Completed {
                                assistant: core::mem::take(&mut assistant),
                            },
                            true,
                        ),
                        Err(error) => (
                            WorkerObservation::Projection(
                                ProjectionEvent::ReconciliationRequired {
                                    code: error.code().to_owned(),
                                    message: error.to_string(),
                                },
                            ),
                            true,
                        ),
                    };
                    if !send_observation(&observation_sender, &worker_cancellation, observation)
                        || terminal
                    {
                        break;
                    }
                }
            }
        });
        Self {
            commands: command_sender,
            observations,
            cancellation,
            thread: Some(thread),
        }
    }

    /// Submits one prompt without blocking terminal input.
    fn submit(&self, prompt: String) -> Result<(), String> {
        self.commands
            .send(InferenceCommand::Submit(prompt))
            .map_err(|_error| "inference worker stopped unexpectedly".to_owned())
    }

    /// Cancels the current operation and joins the worker.
    fn stop(&mut self) {
        self.cancellation.cancel();
        let _ignored = self.commands.send(InferenceCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _join_result = thread.join();
        }
    }
}

/// Delivers one bounded observation while remaining responsive to owner cancellation.
fn send_observation(
    sender: &SyncSender<WorkerObservation>,
    cancellation: &OperationCancellation,
    mut observation: WorkerObservation,
) -> bool {
    loop {
        match sender.try_send(observation) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_observation)) => return false,
            Err(TrySendError::Full(returned)) => {
                if cancellation.is_cancelled() {
                    return false;
                }
                observation = returned;
                thread::sleep(TUI_POLL_INTERVAL);
            }
        }
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "a command-line adapter intentionally writes its result and diagnostics"
)]
/// Runs the pinned protocol-surface checker.
fn run_schema_probe(mut arguments: impl Iterator<Item = std::ffi::OsString>) {
    let Some(schema_path) = arguments.next() else {
        usage();
        process::exit(2);
    };
    if arguments.next().is_some() {
        usage();
        process::exit(2);
    }
    let schema = fs::read_to_string(&schema_path).unwrap_or_else(|error| {
        eprintln!("app_server_schema_read_failed: {error}");
        process::exit(1);
    });
    let report = inspect_protocol_schema(&schema).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    println!(
        "app-server protocol exposes the reviewed Tiber control surface; runtime policy must cover: {}",
        report.controlled_operations().join(", ")
    );
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "authentication commands intentionally print owner-facing status and Codex-owned login handoff information"
)]
/// Runs one authentication operation through its explicit Codex authority boundary.
fn run_auth(mut arguments: impl Iterator<Item = std::ffi::OsString>) {
    let Some(operation) = arguments.next() else {
        usage();
        process::exit(2);
    };
    let operation = operation.to_string_lossy().into_owned();
    if !matches!(
        operation.as_str(),
        "status" | "login" | "login-api-key" | "logout"
    ) || arguments.next().is_some()
    {
        usage();
        process::exit(2);
    }
    let config = default_app_server_config();
    if operation == "login-api-key" {
        config
            .login_with_api_key_from_stdin(ISOLATED_CONFIG)
            .unwrap_or_else(|error| {
                eprintln!("{}: {error}", error.code());
                process::exit(1);
            });
    }
    let mut client = AppServerClient::start(config, ISOLATED_CONFIG).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    let result = match operation.as_str() {
        "status" => client.account_status().map(|status| match status {
            AccountStatus::ApiKey => println!("authenticated: api-key"),
            AccountStatus::ChatGpt { email } => println!(
                "authenticated: chatgpt{}",
                email.map_or_else(String::new, |email| format!(" ({email})"))
            ),
            AccountStatus::SignedOut => println!("signed out"),
        }),
        "login" => client.start_chatgpt_login().and_then(|handoff| {
            println!("open {}", handoff.auth_url);
            println!("waiting for login id: {}", handoff.login_id);
            client.await_chatgpt_login(&handoff.login_id)
        }),
        "login-api-key" => client
            .require_api_key_account()
            .map(|()| println!("authenticated: api-key")),
        _ => client.logout().map(|()| println!("signed out")),
    };
    result.unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "the conversation CLI streams its final observation and inert tool requests"
)]
/// Runs one minimal streamed conversation.
fn run_conversation(arguments: impl Iterator<Item = std::ffi::OsString>) {
    let prompt = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.is_empty() {
        usage();
        process::exit(2);
    }
    let mut client = start_default_client();
    let result = client.converse(&prompt).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    print!("{}", result.text);
    for request in result.inert_tool_requests {
        eprintln!(
            "inert tool request: {} {} {}",
            request.tool, request.call_id, request.arguments
        );
    }
}

#[expect(
    clippy::print_stderr,
    reason = "startup failures are emitted as stable CLI diagnostics"
)]
/// Starts the default isolated app-server client.
fn start_default_client() -> AppServerClient {
    let config = default_app_server_config();
    AppServerClient::start(config, ISOLATED_CONFIG).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    })
}

#[expect(
    clippy::print_stderr,
    reason = "startup failures are emitted as stable CLI diagnostics"
)]
/// Builds the default isolated app-server process configuration.
fn default_app_server_config() -> AppServerConfig {
    let executable = resolve_executable("codex").unwrap_or_else(|| {
        eprintln!("app_server_executable_not_found: codex is not on PATH");
        process::exit(1);
    });
    let codex_home = tiber_codex_home().unwrap_or_else(|| {
        eprintln!("app_server_state_home_unavailable: HOME and XDG_STATE_HOME are unset");
        process::exit(1);
    });
    let workspace = env::current_dir().unwrap_or_else(|error| {
        eprintln!("app_server_workspace_unavailable: {error}");
        process::exit(1);
    });
    AppServerConfig::new(
        executable,
        vec![
            "app-server".to_owned(),
            "--stdio".to_owned(),
            "--strict-config".to_owned(),
        ],
        codex_home,
        workspace,
        Duration::from_mins(10),
    )
    .unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    })
}

/// Resolves one executable from `PATH` without invoking a shell.
fn resolve_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Resolves Tiber's persistent isolated Codex home.
fn tiber_codex_home() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .map(|state| state.join("tiber/codex"))
}

#[expect(
    clippy::print_stderr,
    reason = "usage belongs on stderr for invalid command invocations"
)]
/// Prints the supported command grammar.
fn usage() {
    eprintln!(
        "usage: tiber [app-server-probe <authority-surface.json> | auth <status|login|login-api-key|logout> | converse <prompt> | session active | tasks <{}>]",
        tasks::TASKS_COMMAND_GRAMMAR
    );
}

#[expect(
    clippy::print_stdout,
    reason = "an explicit help request intentionally renders the supported command grammar to standard output"
)]
/// Prints the supported command grammar for an explicit help request.
fn print_help() {
    println!(
        "usage: tiber [app-server-probe <authority-surface.json> | auth <status|login|login-api-key|logout> | converse <prompt> | session active | tasks <{}>]",
        tasks::TASKS_COMMAND_GRAMMAR
    );
}

#[expect(
    clippy::print_stderr,
    reason = "nested command usage belongs on stderr for invalid task invocations"
)]
/// Prints the supported native task grammar.
fn tasks_usage() {
    eprintln!("usage: tiber tasks <{}>", tasks::TASKS_COMMAND_GRAMMAR);
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "binary-shell integration fixtures use fail-fast assertions and fixed event sequences"
)]
mod tests {
    use std::{
        path::PathBuf,
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn repository_diff_marks_every_record_and_escapes_exact_bytes() {
        let rendered = repository_diff(
            "README.md",
            b"same\nold\xff\nno-newline",
            b"same\nnew\x1b[31m\nno-newline",
        );

        assert_eq!(
            rendered,
            "--- a/README.md\n+++ b/README.md\n-b\"same\\x0a\"\n-b\"old\\xff\\x0a\"\n-b\"no-newline\"\n+b\"same\\x0a\"\n+b\"new\\x1b[31m\\x0a\"\n+b\"no-newline\"\n"
        );
    }

    /// Builds the deterministic fake app-server configuration used by the CLI shell.
    fn fixture_client(mode: &str) -> AppServerClient {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("Tiber workspace should canonicalize");
        let node = PathBuf::from("/usr/bin/env");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow Unix epoch")
            .as_nanos();
        let config = AppServerConfig::new(
            node,
            vec![
                "node".to_owned(),
                repository
                    .join("scripts/tests/fake-app-server.mjs")
                    .to_string_lossy()
                    .into_owned(),
                format!("--mode={mode}"),
            ],
            std::env::temp_dir().join(format!("tiber-cli-worker-{}-{nonce}", process::id())),
            repository,
            Duration::from_secs(2),
        )
        .expect("fixture configuration should be valid");
        AppServerClient::start(config, ISOLATED_CONFIG).expect("fixture should initialize")
    }

    #[test]
    fn installed_private_tiber_resolves_sibling_repository_helpers() {
        let directory = tempfile::TempDir::new().expect("resolver fixture should initialize");
        let helper_directory = directory.path().join("out/libexec/tiber");
        std::fs::create_dir_all(&helper_directory)
            .expect("package helper directory should be created");
        let executable = helper_directory.join("tiber");
        let sibling_bubblewrap = helper_directory.join("bwrap");
        let sibling_worker = helper_directory.join("tiber-repository-worker");
        for path in [&executable, &sibling_bubblewrap, &sibling_worker] {
            std::fs::write(path, b"fixture").expect("package helper fixture should be written");
        }
        let path_bubblewrap = directory.path().join("path/bwrap");

        let (bubblewrap, worker) =
            resolve_repository_helper_paths(&executable, Some(path_bubblewrap))
                .expect("installed private executable should have a helper directory");

        assert_eq!(bubblewrap, sibling_bubblewrap);
        assert_eq!(worker, sibling_worker);
    }

    #[test]
    fn installed_private_tiber_does_not_require_bwrap_on_path() {
        let directory = tempfile::TempDir::new().expect("resolver fixture should initialize");
        let helper_directory = directory.path().join("out/libexec/tiber");
        std::fs::create_dir_all(&helper_directory)
            .expect("package helper directory should be created");
        let executable = helper_directory.join("tiber");
        let sibling_bubblewrap = helper_directory.join("bwrap");
        let sibling_worker = helper_directory.join("tiber-repository-worker");
        for path in [&executable, &sibling_bubblewrap, &sibling_worker] {
            std::fs::write(path, b"fixture").expect("package helper fixture should be written");
        }

        let (bubblewrap, worker) = resolve_repository_helper_paths(&executable, None)
            .expect("installed sibling helpers must not depend on PATH");

        assert_eq!(bubblewrap, sibling_bubblewrap);
        assert_eq!(worker, sibling_worker);
    }

    #[test]
    fn cli_preserves_distinct_repository_mutation_service_codes() {
        let invalid_history = PromptPublicationError::RepositoryMutation(
            RepositoryMutationServiceError::InvalidHistory,
        );
        let stream_mismatch = PromptPublicationError::RepositoryMutation(
            RepositoryMutationServiceError::StreamProposalMismatch,
        );

        assert_eq!(
            invalid_history.code(),
            "repository_mutation_history_invalid"
        );
        assert_eq!(
            stream_mismatch.code(),
            "repository_mutation_stream_proposal_mismatch"
        );
        assert_ne!(invalid_history.code(), stream_mismatch.code());
    }

    #[test]
    fn inference_worker_streams_typed_observations_and_stops() {
        let mut worker = InferenceWorker::start(fixture_client("split-stream"));
        worker
            .submit("exercise the default TUI shell".to_owned())
            .expect("worker should accept one prompt");
        let observations = std::iter::repeat_with(|| {
            worker
                .observations
                .recv_timeout(Duration::from_secs(1))
                .expect("worker observation should arrive")
        })
        .take(4)
        .collect::<Vec<_>>();
        assert!(matches!(
            observations.first(),
            Some(WorkerObservation::Projection(ProjectionEvent::AssistantDelta { text }))
                if text == "hello "
        ));
        assert!(matches!(
            observations.get(1),
            Some(WorkerObservation::Projection(ProjectionEvent::AssistantDelta { text }))
                if text == "from Tiber"
        ));
        assert!(matches!(
            observations.get(2),
            Some(WorkerObservation::Projection(ProjectionEvent::InertToolRequested {
                call_id,
                tool,
                arguments,
            }))
                if call_id == "call-fixture"
                    && tool == "tiber_authority_probe"
                    && arguments.pointer("/action").and_then(|value| value.as_str())
                        == Some("sentinel")
        ));
        assert!(matches!(
            observations.last(),
            Some(WorkerObservation::Completed { assistant }) if assistant == "hello from Tiber"
        ));
        worker.stop();
    }

    #[test]
    fn inference_worker_parses_the_closed_repository_proposal_tool() {
        let mut worker = InferenceWorker::start(fixture_client("repository-edit"));
        worker
            .submit("improve the fixture file".to_owned())
            .expect("worker should accept one prompt");
        let observations = std::iter::repeat_with(|| {
            worker
                .observations
                .recv_timeout(Duration::from_secs(1))
                .expect("worker observation should arrive")
        })
        .take(3)
        .collect::<Vec<_>>();

        assert!(matches!(
            observations.get(1),
            Some(WorkerObservation::RepositoryProposal {
                expected,
                path,
                replacement,
            }) if expected == b"before\n"
                && path == "README.md"
                && replacement == b"after\n"
        ));
        worker.stop();
    }

    #[test]
    fn inference_worker_cancels_delayed_start_and_joins_promptly() {
        let mut worker = InferenceWorker::start(fixture_client("delayed-start"));
        worker
            .submit("cancel the delayed start".to_owned())
            .expect("worker should accept one prompt");
        thread::sleep(Duration::from_millis(75));
        let started = Instant::now();
        worker.stop();
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
