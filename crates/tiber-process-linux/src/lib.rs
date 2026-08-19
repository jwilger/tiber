//! Fixed-sandbox `x86_64` Linux adapter for authorized process execution.

#![forbid(unsafe_code)]
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("tiber-process-linux requires x86_64 Linux");

extern crate alloc;

use alloc::sync::Arc;
use core::{
    error::Error,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use rustix::{
    io::Errno,
    process::{Pid, Signal, kill_process_group},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::{
        fs::{OpenOptionsExt as _, PermissionsExt as _},
        process::CommandExt as _,
    },
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Instant,
};
use tiber_process_service::{
    AuthorizedProcess, CapturedProcessBytes, PreparedProcessIdentity, ProcessCancelled,
    ProcessExitStatus, ProcessReceipt, ProcessReconciled, ProcessReconciliationCapability,
    ProcessReconciliationOutcome, ProcessRetirementCapability, ProcessServiceError,
    ProcessSpawnFailure, ProcessSpawnFailureCode, ProcessTimedOut, ProcessUnknown,
};

/// Fixed repository mount visible to every sandboxed process.
const SANDBOX_REPOSITORY: &str = "/workspace";
/// Current private operation-journal schema.
const JOURNAL_SCHEMA_VERSION: u16 = 1;
/// Private launcher environment key removed before target execution.
const HANDSHAKE_ENVIRONMENT: &str = "TIBER_LAUNCH_HANDSHAKE";
/// Private launcher status proving spawn preceded acknowledgment failure.
const ACKNOWLEDGMENT_FAILURE_EXIT_CODE: i32 = 125;
/// Conventional refusal code when the target cannot be launched or observed.
const LAUNCH_FAILURE_EXIT_CODE: i32 = 126;
/// Conservative fallback when a signal-terminated target has no numeric status.
const SIGNAL_EXIT_CODE: i32 = 128;
/// Fixed private runtime mount visible only inside the sandbox.
const SANDBOX_RUNTIME: &str = "/run/tiber";

/// Stable trusted-configuration failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "configuration failures stay grouped by validation lifecycle"
)]
pub enum LinuxProcessConfigurationError {
    /// Every trusted path must be absolute.
    PathNotAbsolute,
    /// A trusted path could not be resolved to its canonical identity.
    PathUnavailable,
    /// The private state root is not owner-only.
    StateRootNotPrivate,
    /// The operational deadline is zero.
    InvalidDeadline,
}

impl LinuxProcessConfigurationError {
    /// Returns the stable machine-readable code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::PathNotAbsolute => "process_linux_path_not_absolute",
            Self::PathUnavailable => "process_linux_path_unavailable",
            Self::StateRootNotPrivate => "process_linux_state_root_not_private",
            Self::InvalidDeadline => "process_linux_invalid_deadline",
        }
    }
}

impl fmt::Display for LinuxProcessConfigurationError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "configuration failures have no retained source"
)]
impl Error for LinuxProcessConfigurationError {}

/// Fixed trusted host configuration for one Linux process adapter.
#[derive(Clone)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "configuration fields preserve their documented construction order"
)]
pub struct LinuxProcessAdapterConfig {
    /// Canonical trusted repository root.
    repository_root: PathBuf,
    /// Canonical owner-only operation-state root.
    state_root: PathBuf,
    /// Canonical fixed Bubblewrap executable.
    bubblewrap: PathBuf,
    /// Canonical fixed direct-argv launcher executable.
    launcher: PathBuf,
    /// Fixed arguments selecting a launcher entrypoint in the executable.
    launcher_arguments: Vec<OsString>,
    /// Maximum adapter-owned operational deadline.
    max_deadline: Duration,
}

impl fmt::Debug for LinuxProcessAdapterConfig {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinuxProcessAdapterConfig(<redacted>)")
    }
}

impl LinuxProcessAdapterConfig {
    /// Resolves and validates fixed owner-controlled adapter configuration.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration failure for invalid paths, permissions,
    /// or deadline.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "trusted path inputs are consumed into canonical owned identities"
    )]
    pub fn new(
        repository_root: PathBuf,
        state_root: PathBuf,
        bubblewrap: PathBuf,
        launcher: PathBuf,
        max_deadline: Duration,
    ) -> Result<Self, LinuxProcessConfigurationError> {
        if !repository_root.is_absolute()
            || !state_root.is_absolute()
            || !bubblewrap.is_absolute()
            || !launcher.is_absolute()
        {
            return Err(LinuxProcessConfigurationError::PathNotAbsolute);
        }
        if max_deadline.is_zero() {
            return Err(LinuxProcessConfigurationError::InvalidDeadline);
        }
        let canonical_repository_root = repository_root
            .canonicalize()
            .map_err(|_source| LinuxProcessConfigurationError::PathUnavailable)?;
        let canonical_state_root = state_root
            .canonicalize()
            .map_err(|_source| LinuxProcessConfigurationError::PathUnavailable)?;
        let canonical_bubblewrap = bubblewrap
            .canonicalize()
            .map_err(|_source| LinuxProcessConfigurationError::PathUnavailable)?;
        let canonical_launcher = launcher
            .canonicalize()
            .map_err(|_source| LinuxProcessConfigurationError::PathUnavailable)?;
        let mode = fs::metadata(&canonical_state_root)
            .map_err(|_source| LinuxProcessConfigurationError::PathUnavailable)?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(LinuxProcessConfigurationError::StateRootNotPrivate);
        }
        Ok(Self {
            repository_root: canonical_repository_root,
            state_root: canonical_state_root,
            bubblewrap: canonical_bubblewrap,
            launcher: canonical_launcher,
            launcher_arguments: Vec::new(),
            max_deadline,
        })
    }

    /// Selects a fixed private launcher entrypoint within the configured executable.
    #[doc(hidden)]
    #[inline]
    #[must_use]
    pub fn with_launcher_arguments<I>(mut self, launcher_arguments: I) -> Self
    where
        I: IntoIterator<Item = OsString>,
    {
        self.launcher_arguments = launcher_arguments.into_iter().collect();
        self
    }
}

/// Cooperative cancellation owned by the trusted imperative shell.
#[derive(Clone, Default)]
pub struct ProcessCancellation(Arc<AtomicBool>);

impl ProcessCancellation {
    /// Requests cancellation of an executing child tree.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Returns whether the owner requested cancellation.
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Stable adapter failures that do not contain command or output content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "adapter failures stay grouped by process lifecycle"
)]
pub enum LinuxProcessError {
    /// Private durable state could not be read or fully synced.
    StateUnavailable,
    /// Existing terminal evidence forbids reusing this exact operation identity.
    OperationAlreadyTerminal,
    /// Captured output crossed an exact configured bound.
    OutputLimitExceeded,
    /// Authorized timeout exceeded the adapter's configured operational maximum.
    DeadlineExceeded,
    /// A service-layer invariant rejected receipt construction.
    InvalidOutcome,
}

impl LinuxProcessError {
    /// Returns the stable machine-readable code.
    #[must_use]
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StateUnavailable => "process_linux_state_unavailable",
            Self::OperationAlreadyTerminal => "process_linux_operation_already_terminal",
            Self::OutputLimitExceeded => "process_linux_output_limit_exceeded",
            Self::DeadlineExceeded => "process_linux_deadline_exceeded",
            Self::InvalidOutcome => "process_linux_invalid_outcome",
        }
    }
}

impl fmt::Display for LinuxProcessError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "adapter errors deliberately retain no sensitive source"
)]
impl Error for LinuxProcessError {}

/// Ephemeral successful execution data used to create a durable service receipt.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "receipt fields preserve their process output order"
)]
pub struct CompletedProcess {
    /// Exact prepared identity consumed by this execution.
    identity: PreparedProcessIdentity,
    /// Definitive semantic exit status.
    status: ProcessExitStatus,
    /// Ephemeral exact bounded stdout.
    stdout: CapturedProcessBytes,
    /// Ephemeral exact bounded stderr.
    stderr: CapturedProcessBytes,
}

impl fmt::Debug for CompletedProcess {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CompletedProcess(<redacted>)")
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "receipt accessors preserve their field and consumption order"
)]
impl CompletedProcess {
    /// Returns the semantic exit status.
    #[must_use]
    #[inline]
    pub const fn status(&self) -> ProcessExitStatus {
        self.status
    }

    /// Returns ephemeral exact stdout bytes.
    #[must_use]
    #[inline]
    pub const fn stdout(&self) -> &CapturedProcessBytes {
        &self.stdout
    }

    /// Returns ephemeral exact stderr bytes.
    #[must_use]
    #[inline]
    pub const fn stderr(&self) -> &CapturedProcessBytes {
        &self.stderr
    }

    /// Consumes ephemeral output into a durable content-free service receipt.
    ///
    /// # Errors
    ///
    /// Returns a service failure if the prepared bounds are inconsistent.
    #[inline]
    pub fn into_receipt(self) -> Result<ProcessReceipt, ProcessServiceError> {
        ProcessReceipt::new(self.identity, self.status, &self.stdout, &self.stderr)
    }
}

/// Closed adapter outcome for one consumed process authority.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "dispatch outcomes follow the process terminal lifecycle"
)]
pub enum ProcessDispatchOutcome {
    /// The process definitively exited and both streams were captured.
    Completed(Box<CompletedProcess>),
    /// The sandbox process definitively never launched.
    SpawnFailed(ProcessSpawnFailure),
    /// The entire sandbox process tree was killed and reaped at its deadline.
    TimedOut(ProcessTimedOut),
    /// The entire sandbox process tree was killed and reaped after cancellation.
    Cancelled(ProcessCancelled),
    /// Launch may have occurred but no definitive terminal evidence was durable.
    OutcomeUnknown(ProcessUnknown),
    /// Output crossed its bound after teardown; completion is definitely excluded.
    OutputLimitExceeded(ProcessUnknown),
}

/// Synchronous Linux execution port that consumes only opaque service authority.
pub struct LinuxProcessAdapter {
    /// Fixed trusted paths and operational bound.
    config: LinuxProcessAdapterConfig,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "adapter operations follow configuration, execution, then reconciliation"
)]
impl LinuxProcessAdapter {
    /// Constructs an adapter from trusted fixed configuration.
    #[must_use]
    #[inline]
    pub const fn new(config: LinuxProcessAdapterConfig) -> Self {
        Self { config }
    }

    /// Executes one authorized process under fixed Bubblewrap containment.
    ///
    /// # Errors
    ///
    /// Returns only stable content-free local-state or output-bound failures.
    #[inline]
    #[expect(
        clippy::too_many_lines,
        reason = "one imperative adapter boundary keeps the launch and durable terminal protocol visibly ordered"
    )]
    pub fn execute(
        &self,
        authority: AuthorizedProcess,
        cancellation: &ProcessCancellation,
    ) -> Result<ProcessDispatchOutcome, LinuxProcessError> {
        let plan = authority.into_adapter_execution_plan();
        if plan.timeout() > self.config.max_deadline {
            return Err(LinuxProcessError::DeadlineExceeded);
        }
        let identity = plan.prepared_identity().clone();
        let journal = Journal::new(&self.config.state_root, &identity)?;
        match journal.reserve()? {
            Reservation::Existing(Some(existing))
                if matches!(
                    existing.as_ref(),
                    JournalState::Prepared { .. } | JournalState::Unknown { .. }
                ) =>
            {
                return Ok(ProcessDispatchOutcome::OutcomeUnknown(ProcessUnknown::new(
                    identity,
                )));
            }
            Reservation::Existing(None) => {
                return Ok(ProcessDispatchOutcome::OutcomeUnknown(ProcessUnknown::new(
                    identity,
                )));
            }
            Reservation::Existing(Some(_)) => {
                return Err(LinuxProcessError::OperationAlreadyTerminal);
            }
            Reservation::Acquired => journal.write(&JournalState::Prepared {
                schema_version: JOURNAL_SCHEMA_VERSION,
                identity: identity.clone(),
            })?,
        }
        journal.prepare_handshake()?;

        let mut command = Command::new(&self.config.bubblewrap);
        command
            .env_clear()
            .args(["--die-with-parent", "--unshare-all", "--new-session"])
            .args(["--dir", SANDBOX_REPOSITORY])
            .args(["--dir", SANDBOX_RUNTIME])
            .args(["--ro-bind", "/nix", "/nix"])
            .args(["--ro-bind", "/bin", "/bin"])
            .args(["--proc", "/proc"])
            .args(["--dev", "/dev"])
            .args(["--tmpfs", "/tmp"])
            .arg("--bind")
            .arg(&self.config.repository_root)
            .arg(SANDBOX_REPOSITORY)
            .arg("--bind")
            .arg(&journal.handshake_root)
            .arg(SANDBOX_RUNTIME)
            .args(["--ro-bind"])
            .arg(&self.config.launcher)
            .arg("/run/tiber/launcher")
            .arg("--chdir")
            .arg(Path::new(SANDBOX_REPOSITORY).join(plan.repository_relative_cwd()))
            .arg("--clearenv");
        command.args(["--setenv", HANDSHAKE_ENVIRONMENT, "/run/tiber/launched"]);
        command.arg("--").arg("/run/tiber/launcher");
        command.args(&self.config.launcher_arguments);
        for (key, value) in plan.fixed_environment() {
            command.args(["--env", key, value]);
        }
        command.arg("--").arg(plan.program()).args(plan.argv());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(source) => {
                let failure = ProcessSpawnFailure::new(identity, classify_spawn_failure(&source));
                journal.write(&JournalState::SpawnFailed {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    failure: failure.clone(),
                })?;
                return Ok(ProcessDispatchOutcome::SpawnFailed(failure));
            }
        };
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or(LinuxProcessError::InvalidOutcome)?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or(LinuxProcessError::InvalidOutcome)?;
        let stdout_reader = bounded_reader(stdout_pipe, plan.stdout_limit_bytes());
        let stderr_reader = bounded_reader(stderr_pipe, plan.stderr_limit_bytes());
        let deadline = plan.timeout();
        let started = Instant::now();

        let observed_terminal = loop {
            let launched = journal.handshake_exists();
            if cancellation.is_cancelled() {
                break if kill_and_reap(&mut child) {
                    let launch_observed = if launched == Ok(false) {
                        journal.handshake_exists()
                    } else {
                        launched
                    };
                    if launch_observed == Ok(true) {
                        ProcessTerminal::Cancelled
                    } else {
                        ProcessTerminal::Unknown
                    }
                } else {
                    ProcessTerminal::Unknown
                };
            }
            if started.elapsed() >= deadline {
                break if kill_and_reap(&mut child) {
                    let launch_observed = if launched == Ok(false) {
                        journal.handshake_exists()
                    } else {
                        launched
                    };
                    if launch_observed == Ok(true) {
                        ProcessTerminal::TimedOut
                    } else {
                        ProcessTerminal::Unknown
                    }
                } else {
                    ProcessTerminal::Unknown
                };
            }
            if reader_exceeded(&stdout_reader) || reader_exceeded(&stderr_reader) {
                break if kill_and_reap(&mut child) {
                    ProcessTerminal::OutputLimit
                } else {
                    ProcessTerminal::Unknown
                };
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let launch_observed = journal.handshake_exists();
                    break match (launch_observed, status.code()) {
                        (Ok(true), _) => ProcessTerminal::Exited(status),
                        (Ok(false), Some(LAUNCH_FAILURE_EXIT_CODE)) => {
                            ProcessTerminal::PrelaunchFailed
                        }
                        (Ok(false) | Err(()), _) => ProcessTerminal::Unknown,
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(_) => {
                    let _reaped = kill_and_reap(&mut child);
                    break ProcessTerminal::Unknown;
                }
            }
        };
        let stdout_completion = finish_reader(&stdout_reader);
        let stderr_completion = finish_reader(&stderr_reader);
        let terminal = match observed_terminal {
            ProcessTerminal::Exited(_)
                if matches!(stdout_completion, ReaderCompletion::Exceeded)
                    || matches!(stderr_completion, ReaderCompletion::Exceeded) =>
            {
                ProcessTerminal::OutputLimit
            }
            ProcessTerminal::Exited(status) => ProcessTerminal::Exited(status),
            ProcessTerminal::TimedOut => ProcessTerminal::TimedOut,
            ProcessTerminal::Cancelled => ProcessTerminal::Cancelled,
            ProcessTerminal::OutputLimit => ProcessTerminal::OutputLimit,
            ProcessTerminal::PrelaunchFailed => ProcessTerminal::PrelaunchFailed,
            ProcessTerminal::Unknown => ProcessTerminal::Unknown,
        };

        match terminal {
            ProcessTerminal::Exited(status)
                if matches!(stdout_completion, ReaderCompletion::Captured(_))
                    && matches!(stderr_completion, ReaderCompletion::Captured(_)) =>
            {
                let ReaderCompletion::Captured(stdout) = stdout_completion else {
                    return Err(LinuxProcessError::InvalidOutcome);
                };
                let ReaderCompletion::Captured(stderr) = stderr_completion else {
                    return Err(LinuxProcessError::InvalidOutcome);
                };
                let completed = CompletedProcess {
                    identity: identity.clone(),
                    status: ProcessExitStatus::Exited(exit_code(status)),
                    stdout,
                    stderr,
                };
                let receipt = ProcessReceipt::new(
                    identity,
                    completed.status,
                    &completed.stdout,
                    &completed.stderr,
                )
                .map_err(|_source| LinuxProcessError::InvalidOutcome)?;
                journal.write(&JournalState::Completed {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    receipt,
                })?;
                Ok(ProcessDispatchOutcome::Completed(Box::new(completed)))
            }
            ProcessTerminal::Exited(_) | ProcessTerminal::Unknown => {
                let unknown = ProcessUnknown::new(identity.clone());
                journal.write(&JournalState::Unknown {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    identity,
                })?;
                Ok(ProcessDispatchOutcome::OutcomeUnknown(unknown))
            }
            ProcessTerminal::TimedOut => {
                let timed_out = ProcessTimedOut::new(identity);
                journal.write(&JournalState::TimedOut {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    terminal: timed_out.clone(),
                })?;
                Ok(ProcessDispatchOutcome::TimedOut(timed_out))
            }
            ProcessTerminal::Cancelled => {
                let cancelled = ProcessCancelled::new(identity);
                journal.write(&JournalState::Cancelled {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    terminal: cancelled.clone(),
                })?;
                Ok(ProcessDispatchOutcome::Cancelled(cancelled))
            }
            ProcessTerminal::OutputLimit => {
                journal.write(&JournalState::OutputLimit {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    identity: identity.clone(),
                })?;
                Ok(ProcessDispatchOutcome::OutputLimitExceeded(
                    ProcessUnknown::new(identity),
                ))
            }
            ProcessTerminal::PrelaunchFailed => {
                let failure = ProcessSpawnFailure::new(
                    identity,
                    ProcessSpawnFailureCode::ResourceUnavailable,
                );
                journal.write(&JournalState::SpawnFailed {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    failure: failure.clone(),
                })?;
                Ok(ProcessDispatchOutcome::SpawnFailed(failure))
            }
        }
    }

    /// Reconciles exact durable state without launching any process.
    ///
    /// # Errors
    ///
    /// Returns a content-free state failure if the private journal is unavailable.
    #[inline]
    pub fn reconcile(
        &self,
        capability: ProcessReconciliationCapability,
    ) -> Result<ProcessReconciled, LinuxProcessError> {
        let journal = Journal::new(&self.config.state_root, capability.prepared_identity())?;
        let outcome = match journal.read()? {
            Some(JournalState::Completed { receipt, .. }) => {
                ProcessReconciliationOutcome::Completed(Box::new(receipt))
            }
            Some(
                JournalState::SpawnFailed { .. }
                | JournalState::TimedOut { .. }
                | JournalState::Cancelled { .. }
                | JournalState::OutputLimit { .. },
            ) => ProcessReconciliationOutcome::DefinitelyNotCompleted,
            _ => ProcessReconciliationOutcome::StillUnknown,
        };
        Ok(capability.into_reconciled(outcome))
    }

    /// Durably retires private artifacts for one signed closed lifecycle.
    ///
    /// # Errors
    ///
    /// Returns a content-free state failure when exact artifact removal or the
    /// parent-directory durability barrier fails.
    #[inline]
    pub fn retire(&self, capability: ProcessRetirementCapability) -> Result<(), LinuxProcessError> {
        let identity = capability.into_prepared_identity();
        Journal::new(&self.config.state_root, &identity)?.retire()
    }
}

/// Private observed child lifecycle before durable terminal classification.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "terminal states follow the process lifecycle"
)]
enum ProcessTerminal {
    /// Direct child exited with an OS status.
    Exited(ExitStatus),
    /// Trusted execution deadline elapsed after proven teardown.
    TimedOut,
    /// Owner cancellation occurred after proven teardown.
    Cancelled,
    /// An exact output bound was crossed after proven teardown.
    OutputLimit,
    /// Bubblewrap started but the configured target never exec'd.
    PrelaunchFailed,
    /// Teardown or completion could not be proven.
    Unknown,
}

/// Concurrent bounded pipe reader and its prompt overflow signal.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "reader fields preserve initialization and capture order"
)]
struct Reader {
    /// One-shot completion channel from the reader thread.
    receiver: mpsc::Receiver<ReaderCompletion>,
    /// Prompt independent overflow observation.
    exceeded: Arc<AtomicBool>,
}

/// Closed completion result from one bounded pipe reader.
enum ReaderCompletion {
    /// Exact bytes were captured within the configured bound.
    Captured(CapturedProcessBytes),
    /// The configured bound was crossed.
    Exceeded,
    /// Pipe reading failed without a definitive byte result.
    Failed,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
/// Private fully durable operation state stored without raw command or output content.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "journal states preserve the durable process lifecycle schema"
)]
enum JournalState {
    /// Authority identity is durable before any child launch.
    Prepared {
        /// Private schema version.
        schema_version: u16,
        /// Exact prepared identity.
        identity: PreparedProcessIdentity,
    },
    /// Definitive completed receipt is durable.
    Completed {
        /// Private schema version.
        schema_version: u16,
        /// Durable content-free completed receipt.
        receipt: ProcessReceipt,
    },
    /// Definitive prelaunch failure is durable.
    SpawnFailed {
        /// Private schema version.
        schema_version: u16,
        /// Durable content-free spawn failure.
        failure: ProcessSpawnFailure,
    },
    /// Definitive timeout after teardown is durable.
    TimedOut {
        /// Private schema version.
        schema_version: u16,
        /// Durable timeout terminal.
        terminal: ProcessTimedOut,
    },
    /// Definitive owner cancellation after teardown is durable.
    Cancelled {
        /// Private schema version.
        schema_version: u16,
        /// Durable cancellation terminal.
        terminal: ProcessCancelled,
    },
    /// Definitive output-bound termination is durable.
    OutputLimit {
        /// Private schema version.
        schema_version: u16,
        /// Exact terminated process identity.
        identity: PreparedProcessIdentity,
    },
    /// Postlaunch outcome remains uncertain.
    Unknown {
        /// Private schema version.
        schema_version: u16,
        /// Exact uncertain process identity.
        identity: PreparedProcessIdentity,
    },
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "journal state accessors preserve schema then identity order"
)]
impl JournalState {
    /// Returns the encoded private schema version.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the closed projection borrows one shared schema field across all variants"
    )]
    fn schema_version(&self) -> u16 {
        match self {
            Self::Prepared { schema_version, .. }
            | Self::Completed { schema_version, .. }
            | Self::SpawnFailed { schema_version, .. }
            | Self::TimedOut { schema_version, .. }
            | Self::Cancelled { schema_version, .. }
            | Self::OutputLimit { schema_version, .. }
            | Self::Unknown { schema_version, .. } => *schema_version,
        }
    }

    /// Returns the exact prepared identity carried by every journal state.
    #[expect(
        clippy::pattern_type_mismatch,
        reason = "the closed projection borrows payload identities across all variants"
    )]
    fn identity(&self) -> &PreparedProcessIdentity {
        match self {
            Self::Prepared { identity, .. }
            | Self::OutputLimit { identity, .. }
            | Self::Unknown { identity, .. } => identity,
            Self::Completed { receipt, .. } => receipt.identity(),
            Self::SpawnFailed { failure, .. } => failure.identity(),
            Self::TimedOut { terminal, .. } => terminal.identity(),
            Self::Cancelled { terminal, .. } => terminal.identity(),
        }
    }
}

/// One identity-bound private fully-fsynced operation journal.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "journal fields preserve durable path construction order"
)]
struct Journal {
    /// Canonical owner-only state root.
    root: PathBuf,
    /// Identity-derived current state path.
    path: PathBuf,
    /// Identity-derived persistent no-replace reservation path.
    reservation: PathBuf,
    /// Identity-derived private launcher handshake directory.
    handshake_root: PathBuf,
    /// Exact identity every decoded state must carry.
    expected_identity: PreparedProcessIdentity,
}

/// Atomic no-replace reservation result for one operation identity.
enum Reservation {
    /// This caller atomically acquired first-dispatch authority.
    Acquired,
    /// A prior reservation exists with optional published state.
    Existing(Option<Box<JournalState>>),
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "journal operations follow construction, handshake, reservation, then persistence"
)]
impl Journal {
    /// Derives private journal paths from one exact prepared identity.
    fn new(root: &Path, identity: &PreparedProcessIdentity) -> Result<Self, LinuxProcessError> {
        let encoded =
            serde_json::to_vec(identity).map_err(|_source| LinuxProcessError::StateUnavailable)?;
        let digest = Sha256::digest(encoded);
        let name = format!("{digest:x}");
        Ok(Self {
            root: root.to_path_buf(),
            path: root.join(format!("{name}.json")),
            reservation: root.join(format!("{name}.lock")),
            handshake_root: root.join(format!("{name}.launch")),
            expected_identity: identity.clone(),
        })
    }

    /// Creates and fully syncs the private per-operation launcher handshake directory.
    fn prepare_handshake(&self) -> Result<(), LinuxProcessError> {
        fs::create_dir_all(&self.handshake_root)
            .map_err(|_source| LinuxProcessError::StateUnavailable)?;
        fs::set_permissions(&self.handshake_root, fs::Permissions::from_mode(0o700))
            .map_err(|_source| LinuxProcessError::StateUnavailable)?;
        let launcher_mount = self.handshake_root.join("launcher");
        let launcher_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(launcher_mount)
            .map_err(|_source| LinuxProcessError::StateUnavailable)?;
        launcher_file
            .sync_all()
            .map_err(|_source| LinuxProcessError::StateUnavailable)?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_source| LinuxProcessError::StateUnavailable)
    }

    /// Returns whether the private launcher proved target spawn success.
    fn handshake_exists(&self) -> Result<bool, ()> {
        match fs::read(self.handshake_root.join("launched")) {
            Ok(contents) if contents == b"launched\n" => Ok(true),
            Ok(_invalid) => Err(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(_source) => Err(()),
        }
    }

    /// Atomically reserves first dispatch or returns existing durable state.
    fn reserve(&self) -> Result<Reservation, LinuxProcessError> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.reservation)
        {
            Ok(file) => {
                file.sync_all()
                    .map_err(|_source| LinuxProcessError::StateUnavailable)?;
                File::open(&self.root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|_source| LinuxProcessError::StateUnavailable)?;
                Ok(Reservation::Acquired)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(Reservation::Existing(self.read()?.map(Box::new)))
            }
            Err(_source) => Err(LinuxProcessError::StateUnavailable),
        }
    }

    /// Reads and validates current identity-bound journal state.
    fn read(&self) -> Result<Option<JournalState>, LinuxProcessError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let state: JournalState = serde_json::from_slice(&bytes)
                    .map_err(|_source| LinuxProcessError::StateUnavailable)?;
                if state.schema_version() != JOURNAL_SCHEMA_VERSION
                    || state.identity() != &self.expected_identity
                {
                    return Err(LinuxProcessError::StateUnavailable);
                }
                Ok(Some(state))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_source) => Err(LinuxProcessError::StateUnavailable),
        }
    }

    /// Idempotently removes every exact identity-derived private artifact and
    /// makes the removals durable before returning.
    fn retire(&self) -> Result<(), LinuxProcessError> {
        let mut removal_failed = false;
        for path in [&self.path, &self.reservation] {
            if let Err(source) = fs::remove_file(path)
                && source.kind() != io::ErrorKind::NotFound
            {
                removal_failed = true;
            }
        }
        match fs::symlink_metadata(&self.handshake_root) {
            Ok(metadata) => {
                let result = if metadata.file_type().is_dir() {
                    for name in ["launcher", "launched", "launched.pending"] {
                        if let Err(source) = fs::remove_file(self.handshake_root.join(name))
                            && source.kind() != io::ErrorKind::NotFound
                        {
                            removal_failed = true;
                        }
                    }
                    fs::remove_dir(&self.handshake_root)
                } else {
                    fs::remove_file(&self.handshake_root)
                };
                if result.is_err() {
                    removal_failed = true;
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(_source) => removal_failed = true,
        }
        let sync_failed = File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .is_err();
        if removal_failed || sync_failed {
            Err(LinuxProcessError::StateUnavailable)
        } else {
            Ok(())
        }
    }

    /// Atomically replaces and fully syncs one durable lifecycle state.
    fn write(&self, state: &JournalState) -> Result<(), LinuxProcessError> {
        let bytes =
            serde_json::to_vec(state).map_err(|_source| LinuxProcessError::StateUnavailable)?;
        let temporary = self.path.with_extension(format!("tmp-{}", process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_source| LinuxProcessError::StateUnavailable)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _remove_result = fs::remove_file(&temporary);
            return Err(LinuxProcessError::StateUnavailable);
        }
        if fs::rename(&temporary, &self.path).is_err() {
            let _remove_result = fs::remove_file(&temporary);
            return Err(LinuxProcessError::StateUnavailable);
        }
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_source| LinuxProcessError::StateUnavailable)
    }
}

/// Runs the private direct-argv launcher protocol inside the process sandbox.
#[doc(hidden)]
#[inline]
pub fn run_private_launcher<I>(arguments: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let mut argument_iterator = arguments.into_iter();
    let mut environment = Vec::new();
    loop {
        let Some(argument) = argument_iterator.next() else {
            return LAUNCH_FAILURE_EXIT_CODE;
        };
        if argument == "--" {
            break;
        }
        if argument != "--env" {
            return LAUNCH_FAILURE_EXIT_CODE;
        }
        let Some(key) = argument_iterator.next() else {
            return LAUNCH_FAILURE_EXIT_CODE;
        };
        let Some(value) = argument_iterator.next() else {
            return LAUNCH_FAILURE_EXIT_CODE;
        };
        environment.push((key, value));
    }
    let Some(program) = argument_iterator.next() else {
        return LAUNCH_FAILURE_EXIT_CODE;
    };
    let Some(handshake_value) = env::var_os(HANDSHAKE_ENVIRONMENT) else {
        return LAUNCH_FAILURE_EXIT_CODE;
    };
    let mut child = match Command::new(program)
        .args(argument_iterator)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(_source) => return LAUNCH_FAILURE_EXIT_CODE,
    };
    let handshake = Path::new(&handshake_value);
    let pending_handshake = handshake.with_extension("pending");
    let handshake_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending_handshake)?;
        file.write_all(b"launched\n")?;
        file.sync_all()?;
        fs::rename(&pending_handshake, handshake)?;
        File::open(handshake.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "handshake has no parent")
        })?)?
        .sync_all()
    })();
    if handshake_result.is_err() {
        let _pending_cleanup = fs::remove_file(pending_handshake);
        let _handshake_cleanup = fs::remove_file(handshake);
        let _kill_result = child.kill();
        let _wait_result = child.wait();
        return ACKNOWLEDGMENT_FAILURE_EXIT_CODE;
    }
    match child.wait() {
        Ok(status) => status.code().unwrap_or(SIGNAL_EXIT_CODE),
        Err(_source) => LAUNCH_FAILURE_EXIT_CODE,
    }
}

/// Drains one pipe concurrently while enforcing its independent exact cap.
fn bounded_reader(mut input: impl io::Read + Send + 'static, limit: usize) -> Reader {
    let exceeded = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&exceeded);
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
        let mut buffer = [u8::default(); 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) if bytes.len().saturating_add(count) <= limit => {
                    bytes.extend(buffer.iter().take(count).copied());
                }
                Ok(_) => {
                    signal.store(true, Ordering::Release);
                    let _send_result = sender.send(ReaderCompletion::Exceeded);
                    return;
                }
                Err(_) => {
                    let _send_result = sender.send(ReaderCompletion::Failed);
                    return;
                }
            }
        }
        let completion = CapturedProcessBytes::new(bytes)
            .map_or(ReaderCompletion::Failed, ReaderCompletion::Captured);
        let _send_result = sender.send(completion);
    });
    Reader { receiver, exceeded }
}

/// Observes prompt overflow without waiting for reader completion.
fn reader_exceeded(reader: &Reader) -> bool {
    reader.exceeded.load(Ordering::Acquire)
}

/// Joins one logical pipe read through its bounded channel result.
fn finish_reader(reader: &Reader) -> ReaderCompletion {
    reader.receiver.recv().unwrap_or(ReaderCompletion::Failed)
}

/// Kills the sandbox process group and proves the direct Bubblewrap child reaped.
fn kill_and_reap(child: &mut Child) -> bool {
    let group_signalled = Pid::from_raw(child.id().cast_signed())
        .is_some_and(|process_group| kill_process_group(process_group, Signal::KILL).is_ok());
    let child_signalled = child.kill().is_ok();
    let reaped = child.wait().is_ok();
    (group_signalled || child_signalled) && reaped
}

/// Maps an OS spawn refusal into a stable content-free service category.
#[expect(
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "all non-semantic OS error kinds conservatively collapse to local resource unavailability"
)]
fn classify_spawn_failure(error: &io::Error) -> ProcessSpawnFailureCode {
    if error.raw_os_error() == Some(Errno::NOEXEC.raw_os_error()) {
        return ProcessSpawnFailureCode::ExecutableUnavailable;
    }
    match error.kind() {
        io::ErrorKind::NotFound => ProcessSpawnFailureCode::ExecutableUnavailable,
        io::ErrorKind::PermissionDenied => ProcessSpawnFailureCode::PermissionDenied,
        _ => ProcessSpawnFailureCode::ResourceUnavailable,
    }
}

/// Projects a portable numeric exit identity, including signal termination.
#[expect(
    clippy::single_call_fn,
    reason = "named exit projection isolates platform status handling from durable outcome construction"
)]
fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(128)
}
