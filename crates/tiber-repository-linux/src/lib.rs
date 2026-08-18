//! Fixed-sandbox `x86_64` Linux adapter for authorized repository mutations.

#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("tiber-repository-linux requires x86_64 Linux");

/// Closed parent-to-worker request and response framing.
mod protocol;
mod recovery;

use alloc::{string::String, sync::Arc, vec::Vec};
use core::{
    error::Error,
    fmt,
    future::ready,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use protocol::{WorkerRequest, WorkerResponse, WorkerWritePrecondition};
use recovery::{JournalDispatchReplay, JournalFact, JournalReconciliationReplay};
use rustix::fs::{FlockOperation, OFlags, fcntl_getfl, fcntl_setfl, flock};
use rustix::process::{Pid, Signal, kill_process_group};
use std::{
    env,
    fs::{DirBuilder, File, OpenOptions, canonicalize, read_link},
    io::{Error as IoError, ErrorKind, Result as IoResult},
    io::{Read as _, Write as _},
    os::unix::{
        fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
        process::CommandExt as _,
    },
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Mutex, MutexGuard, TryLockError, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};
use tiber_repository_core::{
    AuthorizedRepositoryMutation, AuthorizedRepositoryOperation, RepositoryDispatchOutcome,
    RepositoryId, RepositoryMutationFailure, RepositoryMutationFailureCode,
    RepositoryReconciliation, RepositoryReconciliationError, RepositoryReconciliationFailure,
    RepositoryReconciliationOutcome, RepositoryReconciliationState, RepositoryService,
    RepositoryServiceFuture, WritePrecondition,
};

/// Fixed in-sandbox repository mount point.
const SANDBOX_REPOSITORY: &str = "/repo";
/// Fixed in-sandbox private-worker path.
const SANDBOX_WORKER: &str = "/tiber-repository-worker";
/// Maximum accepted worker response size.
const MAX_WORKER_RESPONSE_BYTES: u64 = 16 * 1024;
/// Bounded parent-side worker polling interval.
const POLL_INTERVAL: Duration = Duration::from_millis(2);
/// Independent bound for one read-only reconciliation query.
const RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(1);

/// Stable fail-closed recovery-store error.
pub type LinuxRepositoryRecoveryError = recovery::LinuxRepositoryRecoveryError;
/// Read-only ambiguity handles projected from durable restart state.
pub type LinuxRepositoryRecoveryScan = recovery::LinuxRepositoryRecoveryScan;

/// Stable adapter-configuration failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "callers must handle every fail-closed configuration rejection"
)]
pub enum LinuxRepositoryConfigurationError {
    /// One of the trusted fixed paths was not absolute.
    PathNotAbsolute,
    /// The private receipt root overlaps the mutable working tree.
    StateRootInsideRepository,
}

impl LinuxRepositoryConfigurationError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable one-variant configuration code table is clearest as a tail match"
    )]
    pub const fn code(self) -> &'static str {
        match self {
            Self::PathNotAbsolute => "repository_linux_path_not_absolute",
            Self::StateRootInsideRepository => "repository_linux_state_root_inside_repository",
        }
    }
}

impl fmt::Display for LinuxRepositoryConfigurationError {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable configuration error code"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the closed configuration error does not wrap a lower-level source"
)]
impl Error for LinuxRepositoryConfigurationError {}

/// Cooperative shell cancellation shared with the trusted adapter owner.
#[derive(Clone, Default)]
pub struct RepositoryCancellation {
    /// Shared shell-only cancellation state.
    cancelled: Arc<AtomicBool>,
}

impl RepositoryCancellation {
    /// Requests cancellation of work that has not completed.
    #[inline]
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[expect(
        clippy::implicit_return,
        reason = "the cancellation token has one direct atomic observation"
    )]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Fixed absolute host paths needed by the Linux repository sandbox.
#[derive(Clone)]
pub struct LinuxRepositoryServiceConfig {
    /// Trusted absolute Bubblewrap executable.
    bubblewrap: PathBuf,
    /// Trusted repository identity bound to the configured root.
    repository_id: RepositoryId,
    /// Trusted absolute repository root.
    repository_root: PathBuf,
    /// Trusted absolute root for the private fully-fsynced receipt journal.
    state_root: Option<PathBuf>,
    /// Trusted absolute private worker executable.
    worker: PathBuf,
}

impl LinuxRepositoryServiceConfig {
    /// Parses fixed trusted paths for one adapter instance.
    ///
    /// # Errors
    ///
    /// Returns [`LinuxRepositoryConfigurationError::PathNotAbsolute`] unless
    /// the repository root, Bubblewrap executable, and private worker are absolute.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "successful parsing returns the immutable trusted path bundle directly"
    )]
    pub fn new(
        repository_id: RepositoryId,
        repository_root: PathBuf,
        bubblewrap: PathBuf,
        worker: PathBuf,
    ) -> Result<Self, LinuxRepositoryConfigurationError> {
        if !repository_root.is_absolute() || !bubblewrap.is_absolute() || !worker.is_absolute() {
            return Err(LinuxRepositoryConfigurationError::PathNotAbsolute);
        }
        Ok(Self {
            bubblewrap,
            repository_id,
            repository_root,
            state_root: None,
            worker,
        })
    }

    /// Binds this adapter to one trusted absolute durable-state root.
    ///
    /// # Errors
    ///
    /// Returns [`LinuxRepositoryConfigurationError::PathNotAbsolute`] for a relative root, or
    /// [`LinuxRepositoryConfigurationError::StateRootInsideRepository`] when the private state
    /// root equals or is contained by the mutable repository working tree.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "successful state-root validation returns the updated immutable configuration"
    )]
    pub fn with_state_root(
        mut self,
        state_root: PathBuf,
    ) -> Result<Self, LinuxRepositoryConfigurationError> {
        if !state_root.is_absolute() {
            return Err(LinuxRepositoryConfigurationError::PathNotAbsolute);
        }
        if path_contains(&self.repository_root, &state_root) {
            return Err(LinuxRepositoryConfigurationError::StateRootInsideRepository);
        }
        self.state_root = Some(state_root);
        Ok(self)
    }
}

/// Concrete `x86_64` Linux adapter using a private Bubblewrap worker.
pub struct LinuxRepositoryService {
    /// Shell-only cancellation state.
    cancellation: RepositoryCancellation,
    /// Immutable trusted path configuration.
    config: LinuxRepositoryServiceConfig,
    /// Cooperative serialization for compare-and-apply operations.
    dispatch_lock: Mutex<()>,
}

/// Launched worker plus the thread lifetime required by Bubblewrap parent-death handling.
struct SpawnedWorker {
    /// Fixed Bubblewrap child supervised by the adapter thread.
    child: Child,
    /// Keeps the actual spawning thread alive until all child cleanup is complete.
    launch: SpawnThreadGuard,
}

/// Releases and joins the fixed process-launch thread after child supervision.
struct SpawnThreadGuard {
    /// Signals that the launched child has exited or been killed and reaped.
    release: mpsc::SyncSender<()>,
    /// Joins the otherwise short-lived trusted launch supervisor.
    thread: Option<JoinHandle<()>>,
}

/// Holds the cross-process durable-state lease across receipt and worker phases.
struct RecoveryLease {
    /// Open lock-file descriptor whose advisory lock releases on drop or crash.
    _file: File,
}

impl Drop for SpawnThreadGuard {
    fn drop(&mut self) {
        let _release_result = self.release.send(());
        if let Some(thread) = self.thread.take() {
            let _join_result = thread.join();
        }
    }
}

/// Closed transfer result used to preserve pre- versus post-dispatch semantics.
#[derive(Clone, Copy, Eq, PartialEq)]
enum RequestTransferOutcome {
    /// The complete bounded request frame reached the worker pipe.
    Complete,
    /// The worker was stopped before request transfer began.
    NotStarted,
    /// Transfer began, so the mutation outcome is no longer knowable from transport state.
    OutcomeUnknown,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "public constructors precede the private dispatch lifecycle in adapter-use order"
)]
impl LinuxRepositoryService {
    /// Creates an adapter from fixed trusted configuration.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the default constructor delegates to the cancellation-aware constructor"
    )]
    pub fn new(config: LinuxRepositoryServiceConfig) -> Self {
        Self::with_cancellation(config, RepositoryCancellation::default())
    }

    /// Creates an adapter with a trusted cooperative cancellation handle.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the constructor directly assembles immutable adapter state"
    )]
    pub const fn with_cancellation(
        config: LinuxRepositoryServiceConfig,
        cancellation: RepositoryCancellation,
    ) -> Self {
        Self {
            cancellation,
            config,
            dispatch_lock: Mutex::new(()),
        }
    }

    /// Scans durable state after restart and returns only read-only recovery handles.
    ///
    /// # Errors
    ///
    /// Returns a typed fail-closed error for unavailable, corrupt, or stale journal state.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the public scan is a short fail-closed lease and projection pipeline"
    )]
    pub fn scan_recovery(
        &self,
    ) -> Result<LinuxRepositoryRecoveryScan, LinuxRepositoryRecoveryError> {
        let started = Instant::now();
        let state_root = self
            .config
            .state_root
            .as_ref()
            .ok_or(LinuxRepositoryRecoveryError::StateUnavailable)?;
        let _lease = acquire_recovery_lease(
            state_root,
            started,
            RECONCILIATION_TIMEOUT,
            &self.cancellation,
        )?;
        recovery::load(&state_root.join("journal"), &self.config.repository_id)
            .map(recovery::JournalProjection::scan)
    }

    /// Runs one authorized mutation through the fixed private worker.
    #[expect(
        clippy::implicit_return,
        clippy::result_large_err,
        clippy::too_many_lines,
        reason = "the closed response match returns directly and the core port fixes the safe transcript result shape"
    )]
    fn dispatch_now(
        &self,
        mutation: AuthorizedRepositoryMutation,
    ) -> Result<RepositoryDispatchOutcome, RepositoryMutationFailure> {
        let started = Instant::now();
        let deadline = Duration::from_millis(
            mutation
                .identity()
                .provenance()
                .deadline_milliseconds()
                .get(),
        );
        if mutation.identity().repository_id() != &self.config.repository_id {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        }
        if budget_expired(started, deadline, &self.cancellation) {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        }
        let Ok(_guard) =
            acquire_dispatch_lock(&self.dispatch_lock, started, deadline, &self.cancellation)
        else {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        };
        if budget_expired(started, deadline, &self.cancellation) {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        }

        let identity = mutation.identity();
        let Some(state_root) = self.config.state_root.as_ref() else {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        };
        let Ok(_recovery_lease) =
            acquire_recovery_lease(state_root, started, deadline, &self.cancellation)
        else {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        };
        let journal_root = state_root.join("journal");
        let Ok(projection) = recovery::load(&journal_root, &self.config.repository_id) else {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        };
        match projection.dispatch_replay(&identity) {
            Ok(JournalDispatchReplay::New) => {}
            Ok(JournalDispatchReplay::Applied(receipt)) => {
                return Ok(RepositoryDispatchOutcome::Applied(receipt));
            }
            Ok(JournalDispatchReplay::Failed(failure)) => return Err(failure),
            Ok(JournalDispatchReplay::Unknown(reconciliation)) => {
                return Ok(RepositoryDispatchOutcome::OutcomeUnknown(reconciliation));
            }
            Err(_) => {
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                );
            }
        }
        if recovery::append(
            &journal_root,
            &self.config.repository_id,
            &projection,
            JournalFact::Prepared(&identity),
        )
        .is_err()
        {
            return Err(mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected));
        }
        let Ok(prepared_projection) = recovery::load(&journal_root, &self.config.repository_id)
        else {
            return Ok(mutation.into_ambiguity());
        };

        let outcome = (|| {
            let (request, content) = mutation_request(&mutation);
            let Ok(frame) = encode_request(&request, content) else {
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                );
            };
            if budget_expired(started, deadline, &self.cancellation) {
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                );
            }
            if cfg!(debug_assertions)
                && env::var("TIBER_TEST_REPOSITORY_FAILURE_CODE").as_deref()
                    == Ok("precondition_not_met")
            {
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreconditionNotMet)
                );
            }
            let spawned = self.spawn_worker(false, started, deadline);
            let Ok(SpawnedWorker {
                mut child,
                launch: _launch,
            }) = spawned
            else {
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                );
            };
            if budget_expired(started, deadline, &self.cancellation) {
                kill_and_reap(&mut child);
                return Err(
                    mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                );
            }
            match supervise_request_transfer(
                &mut child,
                &frame,
                started,
                deadline,
                &self.cancellation,
            ) {
                RequestTransferOutcome::Complete => {}
                RequestTransferOutcome::NotStarted => {
                    return Err(
                        mutation.into_failure(RepositoryMutationFailureCode::PreDispatchRejected)
                    );
                }
                RequestTransferOutcome::OutcomeUnknown => return Ok(mutation.into_ambiguity()),
            }
            let waited = wait_for_worker(&mut child, started, deadline, &self.cancellation);
            let Ok(status) = waited else {
                kill_and_reap(&mut child);
                return Ok(mutation.into_ambiguity());
            };
            if !status.success() {
                return Ok(mutation.into_ambiguity());
            }
            let Some(response) = read_response(&mut child) else {
                return Ok(mutation.into_ambiguity());
            };
            match response {
                WorkerResponse::Applied => Ok(RepositoryDispatchOutcome::Applied(
                    mutation.into_applied_receipt(),
                )),
                WorkerResponse::Rejected { code } => Err(mutation.into_failure(code)),
                WorkerResponse::StillUnknown => Ok(mutation.into_ambiguity()),
            }
        })();
        if recovery::append(
            &journal_root,
            &self.config.repository_id,
            &prepared_projection,
            JournalFact::Dispatch(&outcome),
        )
        .is_err()
        {
            return Ok(RepositoryDispatchOutcome::OutcomeUnknown(
                RepositoryReconciliation::from_durable_identity(identity),
            ));
        }
        outcome
    }

    /// Runs one conservative read-only query in a separate sandbox.
    #[expect(
        clippy::implicit_return,
        clippy::needless_pass_by_value,
        clippy::question_mark_used,
        clippy::result_large_err,
        clippy::too_many_lines,
        reason = "the core port consumes the ambiguity handle when binding either outcome or failure"
    )]
    fn reconcile_now(
        &self,
        reconciliation: RepositoryReconciliation,
    ) -> Result<RepositoryReconciliationOutcome, RepositoryReconciliationFailure> {
        if reconciliation.identity().repository_id() != &self.config.repository_id {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        }
        let started = Instant::now();
        let journal = if let Some(state_root) = self.config.state_root.as_ref() {
            let lease = acquire_recovery_lease(
                state_root,
                started,
                RECONCILIATION_TIMEOUT,
                &self.cancellation,
            )
            .map_err(|_recovery_error| {
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            })?;
            let root = state_root.join("journal");
            let projection =
                recovery::load(&root, &self.config.repository_id).map_err(|_recovery_error| {
                    reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
                })?;
            match projection.reconciliation_replay(reconciliation.identity()) {
                Ok(JournalReconciliationReplay::Applied) => {
                    return Ok(reconciliation.bind_outcome(RepositoryReconciliationState::Applied));
                }
                Ok(JournalReconciliationReplay::NotApplied) => {
                    return Ok(
                        reconciliation.bind_outcome(RepositoryReconciliationState::NotApplied)
                    );
                }
                Ok(JournalReconciliationReplay::Untracked) => None,
                Ok(JournalReconciliationReplay::Query) => Some((lease, root, projection)),
                Err(_) => {
                    return Err(reconciliation
                        .bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed));
                }
            }
        } else {
            None
        };
        let request = reconciliation_request(&reconciliation);
        let Ok(frame) = encode_request(&request, &[]) else {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        };
        let spawned = self.spawn_worker(true, started, RECONCILIATION_TIMEOUT);
        let Ok(SpawnedWorker {
            mut child,
            launch: _launch,
        }) = spawned
        else {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        };
        if supervise_request_transfer(
            &mut child,
            &frame,
            started,
            RECONCILIATION_TIMEOUT,
            &self.cancellation,
        ) != RequestTransferOutcome::Complete
        {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        }
        let waited = wait_for_worker(
            &mut child,
            started,
            RECONCILIATION_TIMEOUT,
            &self.cancellation,
        );
        let Ok(status) = waited else {
            kill_and_reap(&mut child);
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        };
        if !status.success() {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        }
        let state = match read_response(&mut child) {
            Some(WorkerResponse::Applied) => RepositoryReconciliationState::Applied,
            Some(WorkerResponse::Rejected {
                code: RepositoryMutationFailureCode::DefinitelyNotApplied,
            }) => RepositoryReconciliationState::NotApplied,
            Some(WorkerResponse::StillUnknown) => RepositoryReconciliationState::StillUnknown,
            Some(WorkerResponse::Rejected { .. }) | None => {
                return Err(
                    reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
                );
            }
        };
        let outcome = reconciliation.bind_outcome(state);
        if let Some((_lease, root, projection)) = journal
            && recovery::append(
                &root,
                &self.config.repository_id,
                &projection,
                JournalFact::Reconciled(&outcome),
            )
            .is_err()
        {
            return Err(
                reconciliation.bind_failure(RepositoryReconciliationError::ReadOnlyQueryFailed)
            );
        }
        Ok(outcome)
    }

    /// Spawns only the configured worker under the caller's immutable budget.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the supervised launch loop returns the fixed child's result directly"
    )]
    fn spawn_worker(
        &self,
        read_only: bool,
        started: Instant,
        deadline: Duration,
    ) -> IoResult<SpawnedWorker> {
        let config = self.config.clone();
        let (sender, receiver) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let spawn_thread = thread::Builder::new()
            .name("tiber-repository-spawn".to_owned())
            .spawn(move || {
                let spawned = spawn_fixed_worker(&config, read_only);
                match sender.send(spawned) {
                    Ok(()) => {
                        let _release_result = released.recv();
                    }
                    Err(unsent) => {
                        if let Ok(mut child) = unsent.0 {
                            kill_and_reap(&mut child);
                        }
                    }
                }
            })?;
        loop {
            match receiver.try_recv() {
                Ok(spawned) => {
                    return spawned.map(|child| SpawnedWorker {
                        child,
                        launch: SpawnThreadGuard {
                            release,
                            thread: Some(spawn_thread),
                        },
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(IoError::other("worker spawn supervisor stopped"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if budget_expired(started, deadline, &self.cancellation) {
                return Err(IoError::new(
                    ErrorKind::TimedOut,
                    "worker launch exceeded dispatch budget",
                ));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

impl RepositoryService for LinuxRepositoryService {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the runtime-neutral port returns a ready future around synchronous worker supervision"
    )]
    fn dispatch(
        &self,
        mutation: AuthorizedRepositoryMutation,
    ) -> RepositoryServiceFuture<'_, Result<RepositoryDispatchOutcome, RepositoryMutationFailure>>
    {
        Box::pin(ready(self.dispatch_now(mutation)))
    }

    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the runtime-neutral port returns a ready future around synchronous read-only supervision"
    )]
    fn reconcile(
        &self,
        reconciliation: RepositoryReconciliation,
    ) -> RepositoryServiceFuture<
        '_,
        Result<RepositoryReconciliationOutcome, RepositoryReconciliationFailure>,
    > {
        Box::pin(ready(self.reconcile_now(reconciliation)))
    }
}

/// Builds the one closed Bubblewrap invocation on a supervised launch thread.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the fixed command builder is intentionally isolated from lifecycle supervision"
)]
fn spawn_fixed_worker(config: &LinuxRepositoryServiceConfig, read_only: bool) -> IoResult<Child> {
    let mut command = Command::new(&config.bubblewrap);
    command
        .args([
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--setenv",
            "TIBER_SANDBOX",
            "1",
            "--ro-bind",
            "/nix/store",
            "/nix/store",
            "--ro-bind",
        ])
        .arg(&config.worker)
        .arg(SANDBOX_WORKER)
        .args([
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--dir",
            SANDBOX_REPOSITORY,
        ]);
    if read_only {
        command.arg("--ro-bind");
    } else {
        command.arg("--bind");
    }
    command
        .arg(&config.repository_root)
        .arg(SANDBOX_REPOSITORY)
        .args(["--chdir", "/", "--", SANDBOX_WORKER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    command.spawn()
}

/// Projects one opaque authorized operation into the closed worker frame.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the closed projection reads clearly as a total tail match at its sole dispatch boundary"
)]
fn mutation_request(mutation: &AuthorizedRepositoryMutation) -> (WorkerRequest, &[u8]) {
    let parent_network_namespace = parent_network_namespace();
    match mutation.operation() {
        AuthorizedRepositoryOperation::Write {
            content,
            path,
            precondition,
        } => {
            let worker_precondition = match precondition {
                WritePrecondition::Absent => WorkerWritePrecondition::Absent,
                WritePrecondition::ExactDigest(digest) => {
                    WorkerWritePrecondition::ExactDigest(digest.as_hex())
                }
            };
            (
                WorkerRequest::Write {
                    content_digest: content.digest().as_hex(),
                    content_length: content.as_bytes().len(),
                    parent_network_namespace,
                    path: path.as_str().to_owned(),
                    precondition: worker_precondition,
                },
                content.as_bytes(),
            )
        }
        AuthorizedRepositoryOperation::Delete { path, precondition } => (
            WorkerRequest::Delete {
                parent_network_namespace,
                path: path.as_str().to_owned(),
                precondition: precondition.as_hex(),
            },
            &[],
        ),
    }
}

/// Projects one safe ambiguity handle into a content-free read-only frame.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the safe identity has one direct closed-frame projection"
)]
fn reconciliation_request(reconciliation: &RepositoryReconciliation) -> WorkerRequest {
    let identity = reconciliation.identity();
    WorkerRequest::Reconcile {
        content_digest: identity.content_digest().map(|digest| digest.as_hex()),
        kind: identity.kind(),
        parent_network_namespace: parent_network_namespace(),
        path: identity.path().as_str().to_owned(),
        precondition: identity.precondition(),
    }
}

/// Reads a safe parent namespace identity used only to prove network isolation.
#[expect(
    clippy::implicit_return,
    reason = "failure becomes an empty value that the worker rejects closed"
)]
fn parent_network_namespace() -> String {
    read_link("/proc/self/ns/net").map_or_else(
        |_| String::new(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Encodes one bounded header-plus-content frame before process launch.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "bounded in-memory framing propagates serialization failure before launch"
)]
fn encode_request(request: &WorkerRequest, content: &[u8]) -> IoResult<Vec<u8>> {
    let mut frame = Vec::with_capacity(content.len().saturating_add(16 * 1024));
    serde_json::to_writer(&mut frame, request)?;
    frame.push(b'\n');
    frame.extend_from_slice(content);
    Ok(frame)
}

/// Supervises request transfer under the same dispatch budget as lock and worker execution.
fn supervise_request_transfer(
    child: &mut Child,
    frame: &[u8],
    started: Instant,
    deadline: Duration,
    cancellation: &RepositoryCancellation,
) -> RequestTransferOutcome {
    let Some(mut stdin) = child.stdin.take() else {
        kill_and_reap(child);
        return RequestTransferOutcome::NotStarted;
    };
    let Ok(flags) = fcntl_getfl(&stdin) else {
        kill_and_reap(child);
        return RequestTransferOutcome::NotStarted;
    };
    if fcntl_setfl(&stdin, flags | OFlags::NONBLOCK).is_err() {
        kill_and_reap(child);
        return RequestTransferOutcome::NotStarted;
    }
    let mut offset: usize = 0;
    let mut transferred = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return if transferred {
                    RequestTransferOutcome::OutcomeUnknown
                } else {
                    RequestTransferOutcome::NotStarted
                };
            }
            Err(_) => {
                kill_and_reap(child);
                return if transferred {
                    RequestTransferOutcome::OutcomeUnknown
                } else {
                    RequestTransferOutcome::NotStarted
                };
            }
            Ok(None) => {}
        }
        if budget_expired(started, deadline, cancellation) {
            kill_and_reap(child);
            return if transferred {
                RequestTransferOutcome::OutcomeUnknown
            } else {
                RequestTransferOutcome::NotStarted
            };
        }
        match stdin.write(frame.get(offset..).unwrap_or_default()) {
            Ok(0) => {
                kill_and_reap(child);
                return if transferred {
                    RequestTransferOutcome::OutcomeUnknown
                } else {
                    RequestTransferOutcome::NotStarted
                };
            }
            Ok(written) => {
                transferred = true;
                let Some(next) = offset.checked_add(written) else {
                    kill_and_reap(child);
                    return RequestTransferOutcome::OutcomeUnknown;
                };
                offset = next;
                if offset == frame.len() {
                    drop(stdin);
                    return RequestTransferOutcome::Complete;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => {
                kill_and_reap(child);
                return if transferred {
                    RequestTransferOutcome::OutcomeUnknown
                } else {
                    RequestTransferOutcome::NotStarted
                };
            }
        }
    }
}

/// Acquires the receipt-store lease before any worker can acquire the repository-root lock.
fn acquire_recovery_lease(
    state_root: &Path,
    started: Instant,
    deadline: Duration,
    cancellation: &RepositoryCancellation,
) -> Result<RecoveryLease, LinuxRepositoryRecoveryError> {
    match DirBuilder::new().mode(0o700).create(state_root) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(LinuxRepositoryRecoveryError::StateUnavailable),
    }
    let state_metadata = match state_root.metadata() {
        Ok(metadata) => metadata,
        Err(_error) => return Err(LinuxRepositoryRecoveryError::StateUnavailable),
    };
    if !state_metadata.is_dir() || state_metadata.mode() & 0o777 != 0o700 {
        return Err(LinuxRepositoryRecoveryError::StateUnavailable);
    }
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(state_root.join(".tiber-repository.lock"))
    {
        Ok(file) => file,
        Err(_error) => return Err(LinuxRepositoryRecoveryError::StateUnavailable),
    };
    let lock_metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_error) => return Err(LinuxRepositoryRecoveryError::StateUnavailable),
    };
    if lock_metadata.mode() & 0o777 != 0o600 {
        return Err(LinuxRepositoryRecoveryError::StateUnavailable);
    }
    loop {
        if flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok() {
            return Ok(RecoveryLease { _file: file });
        }
        if budget_expired(started, deadline, cancellation) {
            return Err(LinuxRepositoryRecoveryError::StateUnavailable);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Returns whether the candidate path resolves lexically or canonically inside the repository.
#[expect(
    clippy::implicit_return,
    clippy::needless_return,
    clippy::single_call_fn,
    reason = "configuration containment is one direct lexical then canonical decision"
)]
fn path_contains(repository_root: &Path, candidate: &Path) -> bool {
    let lexical_repository = lexical_absolute_path(repository_root);
    let lexical_candidate = lexical_absolute_path(candidate);
    if lexical_candidate.starts_with(&lexical_repository) {
        return true;
    }
    let Ok(canonical_repository) = canonicalize(repository_root) else {
        return false;
    };
    resolve_existing_prefix(candidate).is_some_and(|resolved_candidate| {
        return resolved_candidate.starts_with(canonical_repository);
    })
}

/// Eliminates ordinary `.` and `..` components from one already-absolute trusted path.
#[expect(
    clippy::implicit_return,
    reason = "the normalized trusted path is the direct fold result"
)]
fn lexical_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                let _removed = normalized.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    normalized
}

/// Resolves symlinks in the nearest existing ancestor before restoring a missing suffix.
#[expect(
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "nearest-existing-ancestor resolution terminates cleanly when no parent or name remains"
)]
fn resolve_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut cursor = path;
    let mut suffix = Vec::new();
    loop {
        if let Ok(mut resolved) = canonicalize(cursor) {
            for segment in suffix.iter().rev() {
                resolved.push(segment);
            }
            return Some(resolved);
        }
        suffix.push(cursor.file_name()?.to_owned());
        cursor = cursor.parent()?;
    }
}

/// Acquires the cooperative dispatch lock without exceeding cancellation or deadline.
#[expect(
    clippy::single_call_fn,
    reason = "the bounded lock lifecycle is kept explicit at the only serialized dispatch boundary"
)]
fn acquire_dispatch_lock<'lock>(
    lock: &'lock Mutex<()>,
    started: Instant,
    deadline: Duration,
    cancellation: &RepositoryCancellation,
) -> Result<MutexGuard<'lock, ()>, ()> {
    loop {
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => return Err(()),
            Err(TryLockError::WouldBlock) => {}
        }
        if budget_expired(started, deadline, cancellation) {
            return Err(());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Returns whether the one immutable dispatch budget has expired or was cancelled.
#[expect(
    clippy::implicit_return,
    reason = "the shared budget predicate is one direct monotonic observation"
)]
fn budget_expired(
    started: Instant,
    deadline: Duration,
    cancellation: &RepositoryCancellation,
) -> bool {
    cancellation.is_cancelled() || started.elapsed() >= deadline
}

/// Waits within the immutable deadline while observing cooperative cancellation.
#[expect(
    clippy::question_mark_used,
    reason = "wait errors cross the same post-launch ambiguity boundary as timeout"
)]
fn wait_for_worker(
    child: &mut Child,
    start: Instant,
    deadline: Duration,
    cancellation: &RepositoryCancellation,
) -> IoResult<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancellation.is_cancelled() || start.elapsed() >= deadline {
            return Err(IoError::new(
                ErrorKind::TimedOut,
                "worker outcome became unknown",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Kills Bubblewrap, which kills and reaps its isolated command tree, then reaps it.
#[expect(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    reason = "cleanup is best-effort after ambiguity and both attempts are deliberately exhausted"
)]
fn kill_and_reap(child: &mut Child) {
    let pid = Pid::from_child(child);
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads and validates one bounded closed worker response.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "any missing, oversized, or malformed response uniformly becomes post-launch ambiguity"
)]
fn read_response(child: &mut Child) -> Option<WorkerResponse> {
    let mut bytes = Vec::new();
    let stdout = child.stdout.as_mut()?;
    stdout
        .take(MAX_WORKER_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_WORKER_RESPONSE_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
