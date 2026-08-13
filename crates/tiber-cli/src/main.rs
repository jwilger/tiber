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

use ratatui::crossterm::event::{self, Event};
use tiber_app_server::{
    AccountStatus, AppServerClient, AppServerConfig, OperationCancellation, TurnEvent,
    inspect_protocol_schema,
};
use tiber_tui::{ComposerIntent, ConversationProjection, ProjectionEvent};

/// Reviewed isolated app-server configuration template.
const ISOLATED_CONFIG: &str = include_str!("../../../config/app-server.toml");
/// Maximum time the shell waits before checking terminal input again.
const TUI_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Maximum observations applied before terminal input is polled again.
const MAX_OBSERVATIONS_PER_FRAME: usize = 16;
/// Complete grammar accepted after the `tiber tasks` command prefix.
const TASKS_COMMAND_GRAMMAR: &str = "list [--status <backlog|in-progress|done|abandoned>] | show <ref> | search <query> | next | start <ref> | acceptance check <ref> <one-based-index> | subtask check <ref> <one-based-occurrence> | subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id> | transition <ref> done";
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
        "tasks" => run_tasks(arguments),
        _ => {
            eprintln!("unknown command: {}", command.to_string_lossy());
            usage();
            process::exit(2);
        }
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "the command shell writes read-model results and stable owner-facing diagnostics"
)]
/// Runs one native task query or narrow signed task mutation against the current repository.
fn run_tasks(arguments: impl Iterator<Item = std::ffi::OsString>) {
    let repository = env::current_dir().unwrap_or_else(|_error| {
        eprintln!("tiber_tasks_repository_unavailable: current directory could not be read");
        process::exit(1);
    });
    match tasks::run(&repository, arguments) {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            if error.is_usage_error() {
                tasks_usage();
                process::exit(2);
            }
            process::exit(1);
        }
    }
}

/// Runs the interactive projection-only terminal presentation.
#[expect(
    clippy::print_stderr,
    reason = "terminal startup and adapter failures use stable owner-facing diagnostics"
)]
fn run_tui() {
    let client = start_default_client();
    let mut worker = InferenceWorker::start(client);
    let mut projection = ConversationProjection::new();
    let mut terminal = ratatui::try_init().unwrap_or_else(|error| {
        eprintln!("tiber_tui_initialize_failed: {error}");
        process::exit(1);
    });
    let result = run_tui_loop(&mut terminal, &mut worker, &mut projection);
    worker.stop();
    ratatui::restore();
    result.unwrap_or_else(|error| {
        eprintln!("tiber_tui_failed: {error}");
        process::exit(1);
    });
}

/// Drives terminal intents and app-server observations without granting UI authority.
#[expect(
    clippy::question_mark_used,
    reason = "the imperative terminal shell propagates sanitized I/O failures to one owner-facing boundary"
)]
fn run_tui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    worker: &mut InferenceWorker,
    projection: &mut ConversationProjection,
) -> Result<(), String> {
    let mut dirty = true;
    loop {
        for _observation in 0..MAX_OBSERVATIONS_PER_FRAME {
            match worker.observations.try_recv() {
                Ok(observation) => {
                    projection.apply(observation);
                    dirty = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("inference worker stopped unexpectedly".to_owned());
                }
            }
        }
        if dirty {
            terminal
                .draw(|frame| tiber_tui::render(frame, projection))
                .map_err(|error| error.to_string())?;
            dirty = false;
        }
        if !event::poll(TUI_POLL_INTERVAL).map_err(|error| error.to_string())? {
            continue;
        }
        let input = event::read().map_err(|error| error.to_string())?;
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
                projection.apply(ProjectionEvent::PromptSubmitted {
                    text: prompt.clone(),
                });
                worker.submit(prompt)?;
            }
        }
        dirty = true;
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
    observations: Receiver<ProjectionEvent>,
    /// Cooperative cancellation observed during bounded protocol waits.
    cancellation: OperationCancellation,
    /// Worker lifecycle handle.
    thread: Option<JoinHandle<()>>,
}

impl InferenceWorker {
    /// Starts one worker that exclusively owns the app-server client.
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
                            ProjectionEvent::TurnFailed {
                                code: error.code().to_owned(),
                                message: error.to_string(),
                                retryable: error.is_retryable(),
                            },
                        ) {
                            break;
                        }
                        continue;
                    }
                };
                loop {
                    let observation = match client.poll_turn_event(&turn, TUI_POLL_INTERVAL) {
                        Ok(None) => continue,
                        Ok(Some(TurnEvent::AssistantDelta(text))) => {
                            ProjectionEvent::AssistantDelta { text }
                        }
                        Ok(Some(TurnEvent::InertToolRequested(request))) => {
                            ProjectionEvent::InertToolRequested {
                                arguments: request.arguments,
                                call_id: request.call_id,
                                tool: request.tool,
                            }
                        }
                        Ok(Some(TurnEvent::Completed)) => ProjectionEvent::TurnCompleted,
                        Err(error) => ProjectionEvent::TurnFailed {
                            code: error.code().to_owned(),
                            message: error.to_string(),
                            retryable: error.is_retryable(),
                        },
                    };
                    let terminal = matches!(
                        observation,
                        ProjectionEvent::TurnCompleted | ProjectionEvent::TurnFailed { .. }
                    );
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
    sender: &SyncSender<ProjectionEvent>,
    cancellation: &OperationCancellation,
    mut observation: ProjectionEvent,
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
        "usage: tiber [app-server-probe <authority-surface.json> | auth <status|login|login-api-key|logout> | converse <prompt> | tasks <{TASKS_COMMAND_GRAMMAR}>]"
    );
}

#[expect(
    clippy::print_stderr,
    reason = "nested command usage belongs on stderr for invalid task invocations"
)]
/// Prints the supported native task grammar.
fn tasks_usage() {
    eprintln!("usage: tiber tasks <{TASKS_COMMAND_GRAMMAR}>");
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
            Some(ProjectionEvent::AssistantDelta { text }) if text == "hello "
        ));
        assert!(matches!(
            observations.get(1),
            Some(ProjectionEvent::AssistantDelta { text }) if text == "from Tiber"
        ));
        assert!(matches!(
            observations.get(2),
            Some(ProjectionEvent::InertToolRequested { call_id, tool, arguments })
                if call_id == "call-fixture"
                    && tool == "tiber_authority_probe"
                    && arguments.pointer("/action").and_then(|value| value.as_str())
                        == Some("sentinel")
        ));
        assert!(matches!(
            observations.last(),
            Some(ProjectionEvent::TurnCompleted)
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
