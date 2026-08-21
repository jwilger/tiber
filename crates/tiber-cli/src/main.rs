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

mod codex_host_policy;
mod tasks;

extern crate alloc;

use alloc::{collections::BTreeMap, string::FromUtf8Error, sync::Arc};
use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs,
    io::Read as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process,
    sync::Mutex,
    time::Duration,
};

use clap::Parser as _;
use eventcore_types::{BatchSize, EventStoreError, StreamPattern};
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tiber_process_core::{
    AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog, ConfiguredCommandId,
    FixedEnvironment, LiteralArgument, MAX_TIMEOUT, OutputBounds, ProcessInvocationId,
    ProcessPolicyError, ProcessRequest, RelativeWorkingDirectory,
};
use tiber_process_linux::{
    LinuxProcessAdapter, LinuxProcessAdapterConfig, ProcessCancellation, ProcessDispatchOutcome,
    run_private_launcher,
};
use tiber_process_service::{
    MAX_PROCESS_INVOCATION_STREAMS, PreparedProcessIdentity, ProcessExitStatus, ProcessFact,
    ProcessReconciliationOutcome, ProcessRestartState, ProcessServiceError, ProcessStream,
    ProcessUnknown, admit_process_invocation, authorize_prepared_process,
    authorize_process_retirement, classify_process_restart, decide_process_request,
    decide_record_cancelled, decide_record_completed,
    decide_record_reconciled as decide_record_process_reconciled, decide_record_spawn_failed,
    decide_record_timed_out, decide_record_unknown as decide_record_process_unknown,
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
    decide_approve_and_prepare_mutation, decide_cancel_open_proposal_on_restart,
    decide_deny_mutation, decide_propose_mutation, decide_record_applied, decide_record_failed,
    decide_record_unknown, decide_repropose_mutation, recover_prepared_from_history,
};
use tiber_session_service::{
    AssistantText, AssistantTextError, PromptText, PromptTextError, SessionBinding, SessionEvent,
    SessionFact, SessionServiceError, decide_interrupt_inference, decide_observe_inference,
    decide_request_inference, decide_start_session, decide_succeed_session,
    project_started_session, task_assignment_scope,
};
use tiber_store_git::{
    GitStoreError, TiberEventStore, TransactionEventPage, TransactionHistoryError,
    publication::{TiberEventPublisher, TiberPublicationError},
};
use tiber_tasks_core::TaskId;
use tiber_tasks_service::TaskBoardProjection;
use tiber_tui::{ConversationProjection, ProjectionEvent};
use tiber_workflow_core::{
    AgentId, AssignmentEpoch, AssignmentId, AttemptNumber, ContextReceiptId, DeadlineMilliseconds,
    EffectFailureCode, EffectId, EffectObservation, EffectReceiptId, HarnessError, HarnessState,
    IdempotencyKey, InferEffect, PolicyDecisionId, Retryability, SessionId, WorkflowId,
    continue_after_interruption,
};
use tiber_workflow_service::{
    WorkflowEvent, WorkflowServiceError, WorkflowStream, decide_advance_workflow,
    decide_initialize_successor_workflow, decide_initialize_workflow, decide_record_observation,
    decide_request_next_effect,
};
use tokio::runtime::Builder as RuntimeBuilder;

/// Maximum bounded result returned from a Tiber-owned dynamic tool.
const MAX_TIBER_EFFECT_RESULT_BYTES: usize = 16 * 1024;
/// Maximum trusted configured-command document accepted at the CLI boundary.
const MAX_COMMAND_CONFIGURATION_BYTES: u64 = 64 * 1024;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
/// Parsed top-level trusted configured-command document.
struct CommandConfigurationDocument {
    /// Semantic identifiers mapped to fixed trusted execution entries.
    commands: BTreeMap<String, CommandConfigurationEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
/// One external configured-command entry before semantic validation.
struct CommandConfigurationEntry {
    /// Fixed absolute sandbox executable path.
    program: PathBuf,
    /// Fixed literal direct arguments.
    #[serde(default)]
    arguments: Vec<String>,
    /// Fixed repository-relative working directory.
    working_directory: String,
    /// Fixed cleared-process environment entries.
    #[serde(default)]
    environment: BTreeMap<String, String>,
    /// Explicit network request, which Tiber requires to be false.
    network: bool,
    /// Fixed execution deadline.
    timeout_milliseconds: u64,
    /// Fixed stdout capture bound.
    stdout_bytes: usize,
    /// Fixed stderr capture bound.
    stderr_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
/// Stable trusted configured-command boundary failures.
enum ConfiguredProcessError {
    /// The trusted document could not be read.
    #[error("configured command document is unavailable")]
    Io(#[from] std::io::Error),
    /// The trusted document exceeded its byte bound before parsing.
    #[error("configured command document exceeds its semantic bound")]
    TooLarge,
    /// TOML shape or fields were malformed.
    #[error("configured command document is malformed")]
    Malformed(#[from] toml::de::Error),
    /// The document was not valid UTF-8 TOML text.
    #[error("configured command document is not valid UTF-8")]
    Encoding(#[from] FromUtf8Error),
    /// Configuration attempted to enable network access.
    #[error("configured command requests network authority")]
    NetworkRequested,
    /// A parsed value violated process-core semantic policy.
    #[error("configured command violates process policy")]
    Policy(#[from] ProcessPolicyError),
}

impl ConfiguredProcessError {
    /// Returns the stable owner-facing machine code.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the code mapper borrows typed configuration failures without consuming their causes"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "process_configuration_unavailable",
            Self::TooLarge => "process_configuration_too_large",
            Self::Malformed(_) | Self::Encoding(_) => "process_configuration_malformed",
            Self::NetworkRequested => "process_configuration_network_forbidden",
            Self::Policy(_) => "process_configuration_rejected",
        }
    }
}

/// Parses the optional trusted command document exactly once at startup.
fn load_configured_process_catalog(
    repository: &Path,
) -> Result<Option<ConfiguredCommandCatalog>, ConfiguredProcessError> {
    let path = repository.join(".tiber/commands.toml");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ConfiguredProcessError::Io(error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_COMMAND_CONFIGURATION_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_COMMAND_CONFIGURATION_BYTES {
        return Err(ConfiguredProcessError::TooLarge);
    }
    let document: CommandConfigurationDocument = toml::from_str(&String::from_utf8(bytes)?)?;
    let commands = document
        .commands
        .into_iter()
        .map(|(raw_id, entry)| {
            if entry.network {
                return Err(ConfiguredProcessError::NetworkRequested);
            }
            let id = ConfiguredCommandId::parse(&raw_id)?;
            let arguments = entry
                .arguments
                .iter()
                .map(|argument| LiteralArgument::parse(argument))
                .collect::<Result<Vec<_>, _>>()?;
            let command = ConfiguredCommand::new(
                entry.program,
                arguments,
                RelativeWorkingDirectory::parse(&entry.working_directory)?,
                FixedEnvironment::new(entry.environment)?,
                Duration::from_millis(entry.timeout_milliseconds),
                OutputBounds::new(entry.stdout_bytes, entry.stderr_bytes)?,
            )?;
            Ok((id, command))
        })
        .collect::<Result<Vec<_>, ConfiguredProcessError>>()?;
    ConfiguredCommandCatalog::new(commands)
        .map(Some)
        .map_err(ConfiguredProcessError::from)
}

/// Launches the reviewed native Codex TUI and app-server in this process.
#[expect(
    clippy::print_stderr,
    reason = "the executable boundary reports embedded startup failures before exiting"
)]
fn run_native_codex_tui(arg0_paths: codex_arg0::Arg0DispatchPaths) {
    let runtime = RuntimeBuilder::new_multi_thread()
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("codex_tui_runtime_failed: {error}");
            process::exit(1);
        });
    let repository = env::current_dir().unwrap_or_else(|error| {
        eprintln!("codex_host_repository_unavailable: {error}");
        process::exit(1);
    });
    let host_policy = Arc::new(codex_host_policy::TiberHostPolicy::new(repository));
    let cli = codex_tui::Cli::parse_from(["tiber"]).with_host_policy(host_policy);
    runtime
        .block_on(codex_tui::run_main(
            cli,
            arg0_paths,
            codex_config::LoaderOverrides::default(),
            None,
        ))
        .unwrap_or_else(|error| {
            eprintln!("codex_tui_start_failed: {error}");
            process::exit(1);
        });
}

/// Declares the bounded signed task-board surface visible to native Codex.
fn native_dynamic_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "description": "Runs one bounded Tiber task-board operation through signed task authority.",
            "inputSchema": {
                "additionalProperties": false,
                "properties": {
                    "arguments": {
                        "items": { "maxLength": 4096, "type": "string" },
                        "maxItems": 32,
                        "minItems": 1,
                        "type": "array"
                    }
                },
                "required": ["arguments"],
                "type": "object"
            },
            "name": "tiber_tasks",
            "type": "function"
        }),
        serde_json::json!({
            "description": "Reads one bounded UTF-8 regular file beneath the repository without granting shell or mutation authority.",
            "inputSchema": {
                "additionalProperties": false,
                "properties": {
                    "operation": { "const": "read_file", "type": "string" },
                    "path": { "maxLength": 4096, "minLength": 1, "type": "string" }
                },
                "required": ["operation", "path"],
                "type": "object"
            },
            "name": "tiber_repository_read",
            "type": "function"
        }),
        serde_json::json!({
            "description": "Proposes one exact repository write for a later owner decision. This never applies the change by itself.",
            "inputSchema": {
                "additionalProperties": false,
                "properties": {
                    "action": { "const": "write", "type": "string" },
                    "expected": { "type": "string" },
                    "path": { "type": "string" },
                    "replacement": { "type": "string" }
                },
                "required": ["action", "expected", "path", "replacement"],
                "type": "object"
            },
            "name": "tiber_repository_proposal",
            "type": "function"
        }),
        serde_json::json!({
            "description": "Runs one trusted configured command by semantic identifier through Tiber process authority.",
            "inputSchema": {
                "additionalProperties": false,
                "properties": {
                    "command": { "maxLength": 128, "minLength": 1, "type": "string" },
                    "operation": { "const": "run_configured_command", "type": "string" }
                },
                "required": ["operation", "command"],
                "type": "object"
            },
            "name": "tiber_effect",
            "type": "function"
        }),
    ]
}

/// Shared cancellation for the single configured process active behind one native gateway.
#[derive(Default)]
struct NativeProcessCancellationState {
    /// Process cancellation handle installed after durable preparation.
    active: Option<ProcessCancellation>,
    /// Cancellation requested before or during handle installation.
    cancel_latched: bool,
    #[cfg(test)]
    latched_cancel_applied: bool,
}

#[derive(Clone, Default)]
/// Latched cancellation handshake shared with the embedded Codex host policy.
struct NativeProcessCancellation(Arc<Mutex<NativeProcessCancellationState>>);

impl NativeProcessCancellation {
    /// Cancels the active process tree, if one has crossed the dispatch boundary.
    fn cancel(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.cancel_latched = true;
        if let Some(cancellation) = state.active.as_ref() {
            cancellation.cancel();
        }
    }

    /// Clears the completed process cancellation without affecting later invocations.
    fn clear(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = None;
        state.cancel_latched = false;
        #[cfg(test)]
        {
            state.latched_cancel_applied = false;
        }
    }

    /// Installs the exact cancellation paired with the next process dispatch.
    fn install(&self, cancellation: ProcessCancellation) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.cancel_latched {
            cancellation.cancel();
            #[cfg(test)]
            {
                state.latched_cancel_applied = true;
            }
        }
        state.active = Some(cancellation);
    }

    #[cfg(test)]
    fn cancel_is_latched(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel_latched
    }

    #[cfg(test)]
    fn latched_cancel_was_applied(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latched_cancel_applied
    }
}

/// Reads one exact bounded repository preimage without minting effect authority.
fn native_repository_read_result(
    repository: &Path,
    params: &serde_json::Value,
) -> serde_json::Value {
    let Some(arguments) = params
        .get("arguments")
        .and_then(serde_json::Value::as_object)
    else {
        return native_effect_failure("repository_read_invalid", false);
    };
    if arguments.len() != 2
        || arguments
            .get("operation")
            .and_then(serde_json::Value::as_str)
            != Some("read_file")
    {
        return native_effect_failure("repository_read_invalid", false);
    }
    let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
        return native_effect_failure("repository_read_invalid", false);
    };
    if path.is_empty() || path.len() > 4096 {
        return native_effect_failure("repository_read_invalid", false);
    }
    match read_bounded_repository_preimage(repository, path) {
        Ok(bytes) => {
            let Ok(content) = String::from_utf8(bytes) else {
                return native_effect_failure(
                    "repository_mutation_preimage_unsupported_encoding",
                    false,
                );
            };
            serde_json::json!({
                "contentItems": [{
                    "text": serde_json::json!({ "content": content, "path": path }).to_string(),
                    "type": "inputText"
                }],
                "success": true
            })
        }
        Err(error) => native_effect_failure(error.code(), false),
    }
}

/// Persists one exact repository proposal without minting owner approval.
fn native_repository_result(
    repository: &Path,
    params: &serde_json::Value,
) -> (serde_json::Value, Option<PendingRepositoryChange>) {
    let Some(arguments) = params
        .get("arguments")
        .and_then(serde_json::Value::as_object)
    else {
        return (
            native_effect_failure("repository_proposal_invalid", false),
            None,
        );
    };
    if arguments.len() != 4
        || arguments.get("action").and_then(serde_json::Value::as_str) != Some("write")
    {
        return (
            native_effect_failure("repository_proposal_invalid", false),
            None,
        );
    }
    let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
        return (
            native_effect_failure("repository_proposal_invalid", false),
            None,
        );
    };
    let Some(expected) = arguments
        .get("expected")
        .and_then(serde_json::Value::as_str)
    else {
        return (
            native_effect_failure("repository_proposal_invalid", false),
            None,
        );
    };
    let Some(replacement) = arguments
        .get("replacement")
        .and_then(serde_json::Value::as_str)
    else {
        return (
            native_effect_failure("repository_proposal_invalid", false),
            None,
        );
    };
    match publish_repository_proposal(
        repository,
        path,
        expected.as_bytes(),
        replacement.as_bytes(),
    ) {
        Ok((pending, _diff)) => (
            serde_json::json!({
                "contentItems": [{
                    "text": serde_json::json!({
                        "instruction": "Ask the owner to type approve or deny in the next Codex turn.",
                        "path": path,
                        "status": "awaiting_owner"
                    }).to_string(),
                    "type": "inputText"
                }],
                "success": true
            }),
            Some(pending),
        ),
        Err(error) => (native_effect_failure(error.code(), false), None),
    }
}

/// Runs one typed in-process dynamic command request through signed process authority.
fn native_process_result_for_call(
    repository: &Path,
    params: &serde_json::Value,
    invocation: &str,
    cancellation: &ProcessCancellation,
) -> serde_json::Value {
    let Some(arguments) = params
        .get("arguments")
        .and_then(serde_json::Value::as_object)
    else {
        return native_effect_failure("process_request_invalid", false);
    };
    let catalog = match load_configured_process_catalog(repository) {
        Ok(catalog) => catalog,
        Err(error) => return native_effect_failure(error.code(), false),
    };
    match try_execute_configured_process_request(
        repository,
        catalog.as_ref(),
        arguments,
        invocation,
        cancellation,
    ) {
        Ok(result) => native_tiber_effect_result(result),
        Err(error) => native_effect_failure(error.code(), false),
    }
}

/// Closed configured-process result before it is rendered as a dynamic-tool response.
enum TiberEffectResult {
    /// Successfully completed bounded configured process output.
    Success {
        /// Sanitized bounded stdout rendered for Codex.
        output: String,
    },
    /// Stable typed configured-process failure.
    Failure {
        /// Stable machine-readable failure code.
        code: String,
        /// Sanitized owner-facing failure detail.
        message: String,
        /// Whether repeating the same request may safely succeed.
        retryable: bool,
    },
}

/// Projects one typed Tiber effect result into Codex's bounded dynamic-tool result envelope.
fn native_tiber_effect_result(result: TiberEffectResult) -> serde_json::Value {
    match result {
        TiberEffectResult::Success { output } => serde_json::json!({
            "contentItems": [{ "text": output, "type": "inputText" }],
            "success": true
        }),
        TiberEffectResult::Failure {
            code,
            message,
            retryable,
        } => serde_json::json!({
            "contentItems": [{
                "text": serde_json::json!({
                    "code": code,
                    "message": message,
                    "retryable": retryable
                }).to_string(),
                "type": "inputText"
            }],
            "success": false
        }),
    }
}

/// Returns one content-free native effect failure.
fn native_effect_failure(code: &str, retryable: bool) -> serde_json::Value {
    serde_json::json!({
        "contentItems": [{
            "text": serde_json::json!({ "code": code, "retryable": retryable }).to_string(),
            "type": "inputText"
        }],
        "success": false
    })
}

/// Parses and executes one bounded native task call through existing signed authority.
fn native_task_result(repository: &Path, params: &serde_json::Value) -> serde_json::Value {
    let Some(arguments) = params
        .get("arguments")
        .and_then(|arguments| arguments.get("arguments"))
        .and_then(serde_json::Value::as_array)
    else {
        return native_task_failure("tiber_tasks_invalid_arguments");
    };
    if arguments.is_empty() || arguments.len() > 32 {
        return native_task_failure("tiber_tasks_invalid_arguments");
    }
    let parsed_arguments = arguments
        .iter()
        .map(|argument| {
            argument
                .as_str()
                .filter(|argument| argument.len() <= 4096)
                .map(std::ffi::OsString::from)
        })
        .collect::<Option<Vec<_>>>();
    let Some(parsed_arguments) = parsed_arguments else {
        return native_task_failure("tiber_tasks_invalid_arguments");
    };
    let command = match tasks::parse(parsed_arguments.into_iter()) {
        Ok(command) => command,
        Err(error) => return native_task_failure(error.code()),
    };
    let result =
        tasks::context_free_output(&command).map_or_else(|| tasks::run(repository, command), Ok);
    match result {
        Ok(output) if output.len() <= MAX_TIBER_EFFECT_RESULT_BYTES => serde_json::json!({
            "contentItems": [{ "text": output, "type": "inputText" }],
            "success": true
        }),
        Ok(_) => native_task_failure("tiber_tasks_result_too_large"),
        Err(error) => native_task_failure(error.code()),
    }
}

/// Renders one content-free stable task failure for the model-facing tool result.
fn native_task_failure(code: &str) -> serde_json::Value {
    serde_json::json!({
        "contentItems": [{ "text": code, "type": "inputText" }],
        "success": false
    })
}

#[expect(
    clippy::print_stderr,
    reason = "a command-line adapter intentionally writes its result and diagnostics"
)]
fn main() {
    let arg0_guard = codex_arg0::arg0_dispatch();
    let current_exe = env::current_exe().ok();
    let arg0_paths = codex_arg0::Arg0DispatchPaths {
        codex_self_exe: current_exe.clone(),
        codex_linux_sandbox_exe: arg0_guard
            .as_ref()
            .and_then(|guard| guard.paths().codex_linux_sandbox_exe.clone())
            .or(current_exe),
        main_execve_wrapper_exe: arg0_guard
            .as_ref()
            .and_then(|guard| guard.paths().main_execve_wrapper_exe.clone()),
    };
    let mut arguments = env::args_os();
    let _executable = arguments.next();
    let Some(command) = arguments.next() else {
        run_native_codex_tui(arg0_paths);
        return;
    };
    match command.to_string_lossy().as_ref() {
        "session" => run_session(arguments),
        "tasks" => run_tasks(arguments),
        "validate" => run_tasks(core::iter::once(OsString::from("validate")).chain(arguments)),
        "__tiber-process-launcher" => process::exit(run_private_launcher(arguments)),
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
            if let Err(error) = print_process_receipts(&repository, &events) {
                eprintln!("{}: signed process receipt could not be read", error.code());
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
        if let SessionFact::InferenceInterrupted { observation } = event.fact() {
            let code = match observation {
                EffectObservation::Failed { code, .. } => code.as_str(),
                EffectObservation::OutcomeUnknown { .. } => "inference_outcome_unknown",
                EffectObservation::Succeeded { .. } => "inference_resolution_invalid",
            };
            println!("inference interrupted: {code}");
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

/// Renders typed process reconciliation receipts for the active effect.
#[expect(
    clippy::print_stdout,
    clippy::pattern_type_mismatch,
    clippy::semicolon_if_nothing_returned,
    clippy::wildcard_enum_match_arm,
    reason = "the public session query renders durable process receipts"
)]
fn print_process_receipts(
    repository: &Path,
    session_events: &[SessionEvent],
) -> Result<(), ProcessEffectError> {
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
    let store = TiberEventStore::open(repository)?;
    for stream in store
        .stream_ids()
        .iter()
        .filter_map(|stream| ProcessStream::from_verified_effect_stream(effect.effect_id(), stream))
    {
        let history = store.read_process_history(&stream)?;
        if let Some(ProcessFact::Reconciled(reconciled)) = history
            .events()
            .last()
            .map(tiber_process_service::ProcessEvent::fact)
        {
            match reconciled.outcome() {
                ProcessReconciliationOutcome::Completed(_) => {
                    println!("process reconciled: completed")
                }
                ProcessReconciliationOutcome::DefinitelyNotCompleted => {
                    println!("process reconciled: not-completed")
                }
                ProcessReconciliationOutcome::StillUnknown => println!(
                    "process outcome unknown; next action: inspect the configured operation before retrying"
                ),
                _ => println!(
                    "process outcome unknown; next action: upgrade Tiber before continuing"
                ),
            }
        }
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

/// Ensures the sole active native task has one durable conversation binding.
#[expect(
    clippy::collapsible_if,
    clippy::too_many_lines,
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
            !active_events.iter().any(|event| match event.fact() {
                SessionFact::InferenceObserved {
                    effect_id: observed,
                    ..
                } => observed == effect_id,
                SessionFact::InferenceInterrupted { observation } => {
                    observation.effect_id() == effect_id
                }
                _ => false,
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

/// Reconciles every signed per-invocation process stream for the active effect once.
#[expect(
    clippy::collapsible_if,
    clippy::pattern_type_mismatch,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    reason = "the startup shell keeps signed snapshot refreshes and the closed reconciliation lifecycle visibly ordered"
)]
fn apply_process_restart_receipts(
    repository: &Path,
    session_events: &[SessionEvent],
    projection: &mut ConversationProjection,
) -> Result<(), ProcessEffectError> {
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
    let store = TiberEventStore::open(repository)?;
    let streams = store
        .stream_ids()
        .iter()
        .filter_map(|stream| ProcessStream::from_verified_effect_stream(effect.effect_id(), stream))
        .take(MAX_PROCESS_INVOCATION_STREAMS.saturating_add(1))
        .collect::<Vec<_>>();
    if streams.len() > MAX_PROCESS_INVOCATION_STREAMS {
        return Err(ProcessEffectError::Service(
            ProcessServiceError::InvalidHistory,
        ));
    }
    drop(store);
    for stream in streams {
        let store = TiberEventStore::open(repository)?;
        let history = store.read_process_history(&stream)?;
        if let ProcessRestartState::Prepared(identity) =
            classify_process_restart(history.events(), &stream)?
        {
            let unknown = decide_record_process_unknown(
                history.events(),
                stream.clone(),
                ProcessUnknown::new(identity),
            )?;
            let runtime = RuntimeBuilder::new_current_thread().build()?;
            let mut publisher = TiberEventPublisher::open_at(repository, history.revision())?;
            let _revision = runtime.block_on(publisher.publish_process(&stream, unknown))?;
        }

        let store = TiberEventStore::open(repository)?;
        let history = store.read_process_history(&stream)?;
        match classify_process_restart(history.events(), &stream)? {
            ProcessRestartState::Unknown(capability) => {
                if cfg!(debug_assertions) {
                    if let Some(path) = env::var_os("TIBER_TEST_PROCESS_RECONCILIATIONS") {
                        use std::io::Write as _;
                        fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)?
                            .write_all(b"reconcile\n")?;
                    }
                }
                let adapter = LinuxProcessAdapter::new(process_adapter_config(repository)?);
                let reconciled = adapter.reconcile(capability)?;
                let outcome = reconciliation_projection(reconciled.outcome());
                let publication =
                    decide_record_process_reconciled(history.events(), stream.clone(), reconciled)?;
                let runtime = RuntimeBuilder::new_current_thread().build()?;
                let mut publisher = TiberEventPublisher::open_at(repository, history.revision())?;
                let _revision =
                    runtime.block_on(publisher.publish_process(&stream, publication))?;
                projection.apply(outcome);
                retire_closed_process_artifacts(repository, &stream)?;
            }
            ProcessRestartState::Reconciled(outcome) => {
                projection.apply(reconciliation_projection(&outcome));
                retire_closed_process_artifacts(repository, &stream)?;
            }
            ProcessRestartState::Closed => {
                retire_closed_process_artifacts(repository, &stream)?;
            }
            _ => {
                return Err(ProcessEffectError::Service(
                    ProcessServiceError::InvalidHistory,
                ));
            }
        }
    }
    Ok(())
}

/// Re-reads exact signed lifecycle authority before durably retiring private
/// adapter artifacts. Prepared, unknown, and refusal-only histories are a
/// deliberate no-op at this boundary.
fn retire_closed_process_artifacts(
    repository: &Path,
    stream: &ProcessStream,
) -> Result<(), ProcessEffectError> {
    let store = TiberEventStore::open(repository)?;
    let history = store.read_process_history(stream)?;
    let Some(capability) = authorize_process_retirement(history.events(), stream)? else {
        return Ok(());
    };
    LinuxProcessAdapter::new(process_adapter_config(repository)?).retire(capability)?;
    Ok(())
}

/// Maps a closed content-free reconciliation result into owner-visible next state.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "the presentation mapper borrows the closed typed reconciliation result"
)]
fn reconciliation_projection(outcome: &ProcessReconciliationOutcome) -> ProjectionEvent {
    match outcome {
        ProcessReconciliationOutcome::Completed(_) => ProjectionEvent::ProcessReconciled {
            outcome: "completed".to_owned(),
        },
        ProcessReconciliationOutcome::DefinitelyNotCompleted => {
            ProjectionEvent::ProcessReconciled {
                outcome: "not-completed".to_owned(),
            }
        }
        ProcessReconciliationOutcome::StillUnknown => ProjectionEvent::ProcessUnknown {
            next_action: "inspect the configured operation before retrying".to_owned(),
        },
        _ => ProjectionEvent::ProcessUnknown {
            next_action: "upgrade Tiber before continuing".to_owned(),
        },
    }
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
                tiber_workflow_service::WorkflowFact::WorkflowStopped { state, .. } => {
                    continue_after_interruption(state)
                        .ok()
                        .map(|successor| successor.initial_effect().clone())
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

/// Durably records one unsuccessful native turn and advances its stopped workflow.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the interruption publisher selects only the latest borrowed inference request from a non-exhaustive session vocabulary"
)]
fn publish_inference_interruption(
    repository: &Path,
    code: &str,
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
    let workflow_stream = WorkflowStream::for_effect(&effect)?;
    let workflow_history = read_workflow_events_query(&store, &workflow_stream)
        .map_err(PromptPublicationError::Query)?;
    let observation = EffectObservation::Failed {
        code: EffectFailureCode::parse(code)?,
        effect_id: effect.effect_id().clone(),
        retryability: Retryability::NotRetryable,
    };
    let session = decide_interrupt_inference(&all_events, observation.clone())?;
    let workflow =
        decide_record_observation(&workflow_history, workflow_stream.clone(), observation)?;
    let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    let observed_revision = runtime
        .block_on(publisher.publish_inference_observation_with_workflow(session, workflow))?;
    if cfg!(debug_assertions)
        && let Some(path) = env::var_os("TIBER_TEST_CRASH_AFTER_NATIVE_TURN_INTERRUPTION_SENTINEL")
    {
        fs::write(path, b"crash\n")?;
        return Err(std::io::Error::other("debug crash after native turn interruption").into());
    }
    let observed_store = TiberEventStore::open(repository)?;
    let observed_history = read_workflow_events_query(&observed_store, &workflow_stream)
        .map_err(PromptPublicationError::Query)?;
    let advance = decide_advance_workflow(&observed_history, workflow_stream)?;
    let mut terminal_publisher =
        TiberEventPublisher::open_at(repository, observed_revision.revision())?;
    let _stopped = runtime.block_on(terminal_publisher.publish_workflow_advance(advance))?;
    Ok(())
}

/// Resolves one retained native turn without replaying inference or fabricating success.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "startup recovery folds only the latest borrowed session and workflow lifecycle facts"
)]
fn resolve_interrupted_native_inference(repository: &Path) -> Result<(), PromptPublicationError> {
    let store = TiberEventStore::open(repository)?;
    let all_events = read_session_events(&store).map_err(PromptPublicationError::Query)?;
    let events = active_session_events(&all_events);
    let Some(effect) = events.iter().rev().find_map(|event| match event.fact() {
        SessionFact::InferenceRequested { effect, .. } => Some(effect.clone()),
        _ => None,
    }) else {
        return Ok(());
    };
    let terminal = events.iter().any(|event| match event.fact() {
        SessionFact::InferenceObserved { effect_id, .. } => effect_id == effect.effect_id(),
        SessionFact::InferenceInterrupted { observation } => {
            observation.effect_id() == effect.effect_id()
        }
        _ => false,
    });
    let workflow_stream = WorkflowStream::for_effect(&effect)?;
    let workflow_history = read_workflow_events_query(&store, &workflow_stream)
        .map_err(PromptPublicationError::Query)?;
    match workflow_history.last().map(WorkflowEvent::fact) {
        Some(tiber_workflow_service::WorkflowFact::EffectRequested { .. }) if !terminal => {
            drop(store);
            publish_inference_interruption(repository, "native_codex_restart_interrupted")?;
        }
        Some(tiber_workflow_service::WorkflowFact::EffectObserved { .. }) if terminal => {
            let revision = store.revision().clone();
            let advance = decide_advance_workflow(&workflow_history, workflow_stream)?;
            drop(store);
            let mut publisher = TiberEventPublisher::open_at(repository, &revision)?;
            let runtime = RuntimeBuilder::new_current_thread().build()?;
            let _terminal = runtime.block_on(publisher.publish_workflow_advance(advance))?;
        }
        Some(
            tiber_workflow_service::WorkflowFact::WorkflowCompleted { .. }
            | tiber_workflow_service::WorkflowFact::WorkflowStopped { .. },
        ) => {}
        Some(_) | None => return Err(PromptPublicationError::MissingSession),
    }
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
        return Ok(RepositoryApprovalResult::Reproposed {
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
            SessionFact::InferenceInterrupted { observation } => {
                Some(observation.effect_id().as_str())
            }
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
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Typed failures from interpreting one configured process effect.
enum ProcessEffectError {
    /// Model arguments did not match the exact closed request shape.
    #[error("configured process request is malformed")]
    InvalidRequest,
    /// No trusted configured-command document exists.
    #[error("no configured command catalog is available")]
    MissingCatalog,
    /// A future adapter outcome is unsupported by this shell.
    #[error("process adapter returned an unsupported outcome")]
    UnsupportedOutcome,
    /// Local process state or helper resolution failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The signed Git store could not be opened.
    #[error(transparent)]
    Store(#[from] GitStoreError),
    /// Verified process history could not be selected.
    #[error(transparent)]
    History(#[from] tiber_store_git::ProcessHistoryError),
    /// Signed publication failed.
    #[error(transparent)]
    Publication(#[from] TiberPublicationError),
    /// Modeled process authority rejected the transition.
    #[error(transparent)]
    Service(#[from] ProcessServiceError),
    /// Modeled session authority rejected interrupted-turn closure.
    #[error(transparent)]
    Session(#[from] SessionServiceError),
    /// Verified session or workflow history could not be read.
    #[error(transparent)]
    Query(#[from] SessionQueryError),
    /// Pure workflow identity or transition construction failed.
    #[error(transparent)]
    Harness(#[from] HarnessError),
    /// Modeled workflow authority rejected interrupted-turn closure.
    #[error(transparent)]
    Workflow(#[from] WorkflowServiceError),
    /// Fixed Linux adapter configuration was invalid.
    #[error(transparent)]
    LinuxConfiguration(#[from] tiber_process_linux::LinuxProcessConfigurationError),
    /// Linux dispatch failed before producing a modeled outcome.
    #[error(transparent)]
    Linux(#[from] tiber_process_linux::LinuxProcessError),
}

impl ProcessEffectError {
    /// Returns a stable bounded machine code for the model-facing failure.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the code mapper borrows typed process failures without consuming their causes"
    )]
    const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "process_request_invalid",
            Self::MissingCatalog => "process_configuration_unavailable",
            Self::UnsupportedOutcome => "process_outcome_unsupported",
            Self::Io(_) => "process_io_failed",
            Self::Store(_) | Self::History(_) | Self::Publication(_) => {
                "process_publication_failed"
            }
            Self::Service(error) => error.code(),
            Self::Session(error) => error.code(),
            Self::Query(error) => error.code(),
            Self::Harness(error) => error.code(),
            Self::Workflow(error) => error.code(),
            Self::LinuxConfiguration(_) => "process_adapter_configuration_invalid",
            Self::Linux(error) => error.code(),
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

impl Clone for PendingRepositoryChange {
    fn clone(&self) -> Self {
        Self {
            approval_id: self.approval_id.clone(),
            assignment: self.assignment.clone(),
            expected: self.expected.clone(),
            path: self.path.clone(),
            policy: self.policy.clone(),
            proposal: RepositoryMutationProposal::write(
                self.proposal.identity().provenance().clone(),
                self.assignment.repository_id().clone(),
                self.path.clone(),
                RepositoryContent::from_bytes(&self.replacement)
                    .expect("stored replacement already passed repository content validation"),
                WritePrecondition::ExactDigest(Sha256Digest::of(&self.expected)),
            ),
            replacement: self.replacement.clone(),
            stream: self.stream.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        *self = source.clone();
    }
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
        /// Fresh proposal that now requires another explicit decision.
        pending: PendingRepositoryChange,
    },
}

/// Executes one already-bounded configured-process request through durable Tiber authority.
#[expect(
    clippy::too_many_lines,
    reason = "one imperative process shell visibly orders signed preparation, authority, dispatch, and terminal publication"
)]
fn try_execute_configured_process_request(
    repository: &Path,
    catalog: Option<&ConfiguredCommandCatalog>,
    arguments: &serde_json::Map<String, serde_json::Value>,
    invocation: &str,
    cancellation: &ProcessCancellation,
) -> Result<TiberEffectResult, ProcessEffectError> {
    let catalog = catalog.ok_or(ProcessEffectError::MissingCatalog)?;
    if arguments.len() != 2
        || arguments
            .get("operation")
            .and_then(serde_json::Value::as_str)
            != Some("run_configured_command")
    {
        return Err(ProcessEffectError::InvalidRequest);
    }
    let command_id = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .ok_or(ProcessEffectError::InvalidRequest)
        .and_then(|value| {
            ConfiguredCommandId::parse(value).map_err(|_error| ProcessEffectError::InvalidRequest)
        })?;
    let store = TiberEventStore::open(repository)?;
    let effect =
        active_inference_effect(&store).map_err(|_error| ProcessEffectError::InvalidRequest)?;
    let provenance = AssignmentWorkflowProvenance::new(
        effect.workflow_id().clone(),
        effect.assignment_id().clone(),
        effect.effect_id().clone(),
    );
    let request = ProcessRequest::for_invocation(
        command_id,
        ProcessInvocationId::parse(invocation)
            .map_err(|_error| ProcessEffectError::InvalidRequest)?,
        provenance,
    );
    let stream = ProcessStream::for_request(&request)?;
    admit_process_invocation(effect.effect_id(), store.stream_ids(), &stream)?;
    let history = store.read_process_history(&stream)?;
    let request_publication =
        decide_process_request(history.events(), stream.clone(), request.clone(), catalog)?;
    if let Some(result) = retained_process_result(history.events()) {
        retire_closed_process_artifacts(repository, &stream)?;
        return Ok(result);
    }
    if let Some(identity) = retained_prepared_identity(history.events()) {
        let unknown = decide_record_process_unknown(
            history.events(),
            stream.clone(),
            ProcessUnknown::new(identity),
        )?;
        let runtime = RuntimeBuilder::new_current_thread().build()?;
        let mut publisher = TiberEventPublisher::open_at(repository, history.revision())?;
        let _unknown_revision = runtime.block_on(publisher.publish_process(&stream, unknown))?;
        return Ok(process_terminal_failure(
            "process_outcome_unknown",
            "configured command outcome is unknown after restart",
        ));
    }
    if !history.events().is_empty() {
        return Err(ProcessEffectError::Service(
            ProcessServiceError::InvalidHistory,
        ));
    }
    let runtime = RuntimeBuilder::new_current_thread().build()?;
    if let Err(refusal) = catalog.resolve(request.command_id()) {
        let mut refusal_publisher = TiberEventPublisher::open_at(repository, history.revision())?;
        let _refused_revision =
            runtime.block_on(refusal_publisher.publish_process(&stream, request_publication))?;
        return Ok(process_terminal_failure(
            refusal.code(),
            "configured command is not present in trusted configuration",
        ));
    }
    let adapter_config = process_adapter_config(repository)?;
    let mut preparation_publisher = TiberEventPublisher::open_at(repository, history.revision())?;
    let _prepared_revision =
        runtime.block_on(preparation_publisher.publish_process(&stream, request_publication))?;

    let prepared_store = TiberEventStore::open(repository)?;
    let prepared_history = prepared_store.read_process_history(&stream)?;
    let authority =
        authorize_prepared_process(prepared_history.events(), &stream, &request, catalog)?;
    let adapter = LinuxProcessAdapter::new(adapter_config);
    let outcome = adapter.execute(authority, cancellation)?;
    if cfg!(debug_assertions)
        && let Some(sentinel) = env::var_os("TIBER_TEST_CRASH_AFTER_PROCESS_DISPATCH_SENTINEL")
    {
        fs::write(sentinel, b"dispatched\n")?;
        process::exit(87);
    }
    let terminal_store = TiberEventStore::open(repository)?;
    let terminal_history = terminal_store.read_process_history(&stream)?;
    let (terminal_publication, result) = match outcome {
        ProcessDispatchOutcome::Completed(completed) => {
            let status = completed.status();
            let stdout = completed.stdout().as_bytes();
            let stderr = completed.stderr().as_bytes();
            let ProcessExitStatus::Exited(code) = status else {
                return Err(ProcessEffectError::UnsupportedOutcome);
            };
            let output = render_completed_process_result(code, stdout, stderr);
            let completed_publication = decide_record_completed(
                terminal_history.events(),
                stream.clone(),
                completed.into_receipt()?,
            )?;
            (completed_publication, TiberEffectResult::Success { output })
        }
        ProcessDispatchOutcome::SpawnFailed(failure) => (
            decide_record_spawn_failed(terminal_history.events(), stream.clone(), failure)?,
            process_terminal_failure("process_spawn_failed", "configured command could not start"),
        ),
        ProcessDispatchOutcome::TimedOut(timed_out) => (
            decide_record_timed_out(terminal_history.events(), stream.clone(), timed_out)?,
            process_terminal_failure("process_timed_out", "configured command timed out"),
        ),
        ProcessDispatchOutcome::Cancelled(cancelled) => (
            decide_record_cancelled(terminal_history.events(), stream.clone(), cancelled)?,
            process_terminal_failure("process_cancelled", "configured command was cancelled"),
        ),
        ProcessDispatchOutcome::OutcomeUnknown(unknown) => (
            decide_record_process_unknown(terminal_history.events(), stream.clone(), unknown)?,
            process_terminal_failure(
                "process_outcome_unknown",
                "configured command outcome is unknown",
            ),
        ),
        ProcessDispatchOutcome::OutputLimitExceeded(unknown) => (
            decide_record_process_unknown(terminal_history.events(), stream.clone(), unknown)?,
            process_terminal_failure(
                "process_linux_output_limit_exceeded",
                "configured command exceeded its output bound",
            ),
        ),
        _ => return Err(ProcessEffectError::UnsupportedOutcome),
    };
    let mut terminal_publisher =
        TiberEventPublisher::open_at(repository, terminal_history.revision())?;
    let _terminal_revision =
        runtime.block_on(terminal_publisher.publish_process(&stream, terminal_publication))?;
    if cfg!(debug_assertions)
        && let Some(sentinel) =
            env::var_os("TIBER_TEST_CRASH_AFTER_PROCESS_TERMINAL_PUBLICATION_SENTINEL")
    {
        fs::write(sentinel, b"published\n")?;
        process::exit(88);
    }
    retire_closed_process_artifacts(repository, &stream)?;
    Ok(result)
}

/// Selects exact retained preparation only from atomic requested/prepared history.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "future process facts cannot be treated as redispatchable preparation"
)]
fn retained_prepared_identity(
    events: &[tiber_process_service::ProcessEvent],
) -> Option<PreparedProcessIdentity> {
    if events.len() != 2 {
        return None;
    }
    match events.get(1)?.fact().clone() {
        ProcessFact::Prepared(identity) => Some(identity),
        _ => None,
    }
}

/// Projects terminal retained history without minting new dispatch authority.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "future retained facts fail closed to no redispatch until the CLI interprets them explicitly"
)]
fn retained_process_result(
    events: &[tiber_process_service::ProcessEvent],
) -> Option<TiberEffectResult> {
    match events.last()?.fact().clone() {
        ProcessFact::Completed(_) => Some(TiberEffectResult::Success {
            output: "configured command already completed".to_owned(),
        }),
        ProcessFact::Refused { code, .. } => Some(process_terminal_failure(
            code.code(),
            "configured command is not present in trusted configuration",
        )),
        ProcessFact::SpawnFailed(_) => Some(process_terminal_failure(
            "process_spawn_failed",
            "configured command previously failed to start",
        )),
        ProcessFact::TimedOut(_) => Some(process_terminal_failure(
            "process_timed_out",
            "configured command previously timed out",
        )),
        ProcessFact::Cancelled(_) => Some(process_terminal_failure(
            "process_cancelled",
            "configured command was previously cancelled",
        )),
        ProcessFact::Unknown(_) => Some(process_terminal_failure(
            "process_outcome_unknown",
            "configured command outcome remains unknown",
        )),
        ProcessFact::Reconciled(_) => Some(process_terminal_failure(
            "process_outcome_reconciled",
            "configured command outcome was reconciled without redispatch",
        )),
        _ => None,
    }
}

/// Builds one bounded non-retryable process terminal failure.
fn process_terminal_failure(code: &str, message: &str) -> TiberEffectResult {
    TiberEffectResult::Failure {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

/// Renders bounded command output inside the app-server's exact completion envelope.
fn render_completed_process_result(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    let empty = serde_json::json!({
        "status": { "exit_code": exit_code },
        "stdout": "",
        "stdout_truncated": false,
        "stderr": "",
        "stderr_truncated": false,
    })
    .to_string();
    let available = MAX_TIBER_EFFECT_RESULT_BYTES.saturating_sub(empty.len());
    let stdout_budget = available.checked_shr(1).unwrap_or_default();
    let stderr_budget = available.saturating_sub(stdout_budget);
    let (stdout, stdout_truncated) = bounded_json_text(stdout, stdout_budget);
    let (stderr, stderr_truncated) = bounded_json_text(stderr, stderr_budget);
    let output = serde_json::json!({
        "status": { "exit_code": exit_code },
        "stdout": stdout,
        "stdout_truncated": stdout_truncated,
        "stderr": stderr,
        "stderr_truncated": stderr_truncated,
    })
    .to_string();
    if output.len() <= MAX_TIBER_EFFECT_RESULT_BYTES {
        output
    } else {
        empty
    }
}

/// Retains as many sanitized characters as fit their encoded JSON-string budget.
fn bounded_json_text(bytes: &[u8], encoded_budget: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    let mut output = String::new();
    let mut encoded_bytes: usize = 0;
    let mut truncated = false;
    for character in text.chars() {
        let sanitized = if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            '\u{fffd}'
        } else {
            character
        };
        let cost = match sanitized {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            _ => sanitized.len_utf8(),
        };
        if encoded_bytes.saturating_add(cost) > encoded_budget {
            truncated = true;
            break;
        }
        output.push(sanitized);
        encoded_bytes = encoded_bytes.saturating_add(cost);
    }
    (output, truncated)
}

/// Resolves fixed package helpers and one private repository-scoped process state root.
fn process_adapter_config(
    repository: &Path,
) -> Result<LinuxProcessAdapterConfig, ProcessEffectError> {
    let repository_root = repository.canonicalize()?;
    let executable = env::current_exe()?;
    let helper_directory = executable.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "helper directory unavailable")
    })?;
    let sibling_bubblewrap = helper_directory.join("bwrap");
    let bubblewrap = if sibling_bubblewrap.is_file() {
        sibling_bubblewrap
    } else {
        resolve_executable("bwrap").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "bubblewrap unavailable")
        })?
    };
    let installed_launcher = helper_directory.join("tiber-process-launcher");
    let configured_launcher = if cfg!(debug_assertions) {
        env::var_os("TIBER_TEST_PROCESS_LAUNCHER").map(PathBuf::from)
    } else {
        None
    };
    let (launcher, launcher_arguments) = configured_launcher.map_or_else(
        || {
            if installed_launcher.is_file() {
                (installed_launcher, Vec::new())
            } else {
                (
                    executable,
                    vec![std::ffi::OsString::from("__tiber-process-launcher")],
                )
            }
        },
        |launcher| (launcher, Vec::new()),
    );
    let state_base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "state home unavailable")
        })?;
    let repository_digest = Sha256Digest::of(repository_root.as_os_str().as_encoded_bytes());
    let state_root = state_base
        .join("tiber/process")
        .join(repository_digest.as_hex());
    fs::create_dir_all(&state_root)?;
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
    Ok(LinuxProcessAdapterConfig::new(
        repository_root,
        state_root,
        bubblewrap,
        launcher,
        MAX_TIMEOUT,
    )?
    .with_launcher_arguments(launcher_arguments))
}

/// Resolves one executable from `PATH` without invoking a shell.
fn resolve_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}
#[expect(
    clippy::print_stderr,
    reason = "invalid command usage belongs on stderr"
)]
/// Prints the supported command grammar.
fn usage() {
    eprintln!(
        "usage: tiber [session active | validate --fix | tasks <{}>]",
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
        "usage: tiber [session active | validate --fix | tasks <{}>]",
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
