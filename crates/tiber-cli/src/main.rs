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
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::Duration,
};

use eventcore_types::{BatchSize, EventStoreError, StreamPattern};
use ratatui::crossterm::event::{self, Event};
use sha2::{Digest as _, Sha256};
use tiber_app_server::{
    AccountStatus, AppServerClient, AppServerConfig, OperationCancellation, TurnEvent,
    inspect_protocol_schema,
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

#[expect(
    clippy::print_stderr,
    reason = "the command shell renders stable task diagnostics before its terminal exit status"
)]
/// Renders one task failure and terminates with its stable command-line status.
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
#[expect(
    clippy::used_underscore_binding,
    reason = "the harness boundary preserves its closed recovery and projection control flow"
)]
fn run_tui() {
    let repository = env::current_dir().unwrap_or_else(|error| {
        eprintln!("tiber_session_repository_unavailable: {error}");
        process::exit(1);
    });
    let _binding = ensure_started_session(&repository).unwrap_or_else(|error| {
        eprintln!("{}: {error}", error.code());
        process::exit(1);
    });
    let history = load_session_history(&repository)
        .unwrap_or_else(|error| {
            eprintln!("{}: {error}", error.code());
            process::exit(1);
        })
        .map_or_else(Vec::new, |_binding_events_and_workflow| {
            _binding_events_and_workflow.1
        });
    let mut projection = restored_conversation_projection(&history);
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
fn ensure_started_session(repository: &Path) -> Result<SessionBinding, SessionStartupError> {
    let store = TiberEventStore::open(repository)?;
    let revision = store.revision().clone();
    let session_events = read_session_events(&store)?;
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
            return Ok(binding);
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
                let successor = binding_for_task(&task.stem)?;
                let publication =
                    decide_succeed_session(&session_events, binding.clone(), successor.clone())?;
                let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
                let runtime = RuntimeBuilder::new_current_thread().build()?;
                let _published =
                    runtime.block_on(publisher.publish_session_successor(publication))?;
                return Ok(successor);
            }
        }
        return Ok(binding);
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
        return Ok(binding);
    };
    let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let _published = runtime.block_on(publisher.publish_session_start(publication))?;
    Ok(binding)
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
    clippy::wildcard_enum_match_arm,
    reason = "only transcript-bearing facts affect the restored conversation projection"
)]
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the transcript projection matches borrowed durable session facts"
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
fn run_tui_loop(
    repository: &Path,
    terminal: &mut ratatui::DefaultTerminal,
    worker: &mut InferenceWorker,
    projection: &mut ConversationProjection,
) -> Result<(), TuiRunError> {
    let mut dirty = true;
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
            ComposerIntent::Submit(prompt) => {
                publish_prompt_request(repository, &prompt)
                    .map_err(TuiRunError::PromptPublication)?;
                projection.apply(ProjectionEvent::PromptSubmitted {
                    text: prompt.clone(),
                });
                worker
                    .submit(prompt)
                    .map_err(|_error| TuiRunError::WorkerStopped)?;
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
                        Ok(Some(TurnEvent::InertToolRequested(request))) => (
                            WorkerObservation::Projection(ProjectionEvent::InertToolRequested {
                                arguments: request.arguments,
                                call_id: request.call_id,
                                tool: request.tool,
                            }),
                            false,
                        ),
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
