//! EventCore adapter whose transaction boundary is a confirmed Git ref.
//!
//! `eventcore-fs` is a disposable staging engine, not the authority. An append
//! succeeds only after one signed candidate commit containing the immutable
//! transaction is confirmed on its selected semantic authority ref. A conclusive concurrent
//! advance is translated to EventCore's version conflict so pure command logic
//! may rerun with invocation-stable IDs and timestamps. An ambiguous push never
//! reruns domain logic: the exact candidate is retained under Git's common
//! directory and all mutations fail closed until `tiber sync` establishes
//! whether that candidate was published. Repositories without `origin` use a
//! local compare-and-swap on the same ref as their commit boundary.
//!
//! Reads rebuild their disposable file store from the authoritative ref. With
//! an origin they fetch before each operation and fail rather than serving a
//! stale cache. An absent ref is the empty EventStore required by the reusable
//! EventCore backend contract; application-level read commands may still call
//! that state "uninitialized".

use eventcore_fs::{FileEventStore, FsConfig, FsEventStoreError};
use eventcore_types::{
    CommandStateSnapshot, CommandStateSnapshotId, Event, EventFilter, EventPage, EventReader,
    EventStore, EventStoreError, EventStream, EventStreamSlice, Operation, StreamId,
    StreamPosition, StreamVersion, StreamWrites,
};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;
use uuid::Uuid;
use wait_timeout::ChildExt;

/// The default authoritative ref used for every new Tiber event stream.
pub const TIBER_BRANCH: &str = "tiber";
#[cfg(test)]
const REMOTE_REF: &str = "refs/remotes/origin/tiber";
#[cfg(test)]
const REMOTE_HEAD: &str = "refs/heads/tiber";
const STORE_DIRECTORY: &str = "eventstore";
const PUBLICATION_RETRIES: usize = 3;
// Loading a new disposable stage can briefly contend with another writer's
// ref update.  This remains deliberately bounded, but is independent from
// publication retries: retrying a read does not replay a domain command.
const STAGE_LOAD_RETRIES: usize = 8;
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PENDING_VERSION: u32 = 1;
const FALLBACK_GIT_NAME: &str = "Tiber Event Store";
const FALLBACK_GIT_EMAIL: &str = "tiber-event-store@localhost.invalid";

/// A closed set of independent Git authorities supported by this EventCore
/// adapter.  Keeping this an enum rather than accepting ref names from a
/// caller prevents an MCP request from selecting an arbitrary Git ref.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitEventStoreAuthority {
    Tiber,
    DevelopmentWorkflow,
    PluginAdvisoryFinalReview,
}

#[derive(Clone, Copy, Debug)]
struct AuthorityConfig {
    local_ref: &'static str,
    remote: Option<RemoteAuthorityConfig>,
    state_directory: &'static str,
    commit_message: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct RemoteAuthorityConfig {
    tracking_ref: &'static str,
    head: &'static str,
}

impl GitEventStoreAuthority {
    fn config(self) -> AuthorityConfig {
        match self {
            Self::Tiber => AuthorityConfig {
                local_ref: "refs/heads/tiber",
                remote: Some(RemoteAuthorityConfig {
                    tracking_ref: "refs/remotes/origin/tiber",
                    head: "refs/heads/tiber",
                }),
                state_directory: "tiber",
                commit_message: "tiber event transaction",
            },
            Self::DevelopmentWorkflow => AuthorityConfig {
                local_ref: "refs/heads/development-workflow",
                remote: Some(RemoteAuthorityConfig {
                    tracking_ref: "refs/remotes/origin/development-workflow",
                    head: "refs/heads/development-workflow",
                }),
                state_directory: "development-workflow",
                commit_message: "development workflow event transaction",
            },
            Self::PluginAdvisoryFinalReview => AuthorityConfig {
                local_ref: "refs/tiber/plugin-advisory-final-review",
                remote: None,
                state_directory: "plugin-advisory-final-review",
                commit_message: "plugin advisory final review event transaction",
            },
        }
    }
}

/// Failure to open or refresh the Git-backed store.
#[derive(Debug, thiserror::Error)]
pub enum GitEventStoreOpenError {
    #[error(transparent)]
    FileStore(#[from] FsEventStoreError),
    #[error("Git event store operation failed: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
struct Stage {
    _directory: TempDir,
    work_tree: PathBuf,
    store: FileEventStore,
    base: Option<String>,
    has_remote_authority: bool,
}

/// EventCore store whose successful append means the candidate is confirmed
/// on its selected semantic authority ref.
#[derive(Clone, Debug)]
pub struct GitEventStore {
    repository: PathBuf,
    common_directory: PathBuf,
    authority: GitEventStoreAuthority,
    stage: Arc<Mutex<Stage>>,
    operation: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynchronizeOutcome {
    Current,
    PublishedPending,
    DiscardedUnpublished,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingPublication {
    version: u32,
    candidate: String,
    base: Option<String>,
    authority: String,
}

/// A bounded, local recovery marker. This is deliberately not an event-store
/// fact: the authoritative append is either confirmed on the selected Git ref
/// or unresolved. Synchronization re-derives and removes this marker only
/// after resolving that authority.
#[derive(Debug, Serialize)]
struct PublicationBlocker<'a> {
    schema_version: u8,
    kind: &'a str,
    error_code: &'a str,
    required_action: &'a str,
    created_at: u64,
}

/// Serializes Git ref updates for stores that share one Git common directory.
/// Remote compare-and-swap remains the cross-clone authority boundary; this
/// lock only prevents two cooperative local store instances from racing over
/// the shared remote-tracking ref while they perform fetch/push reconciliation.
struct EventStoreOperationLock {
    _file: fs::File,
}

impl EventStoreOperationLock {
    fn acquire(
        common_directory: &Path,
        authority: GitEventStoreAuthority,
    ) -> Result<Self, GitEventStoreOpenError> {
        let directory = common_directory.join(authority.config().state_directory);
        fs::create_dir_all(&directory)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("eventstore-operation.lock"))?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

impl GitEventStore {
    /// Opens a repository-backed store. An absent branch is an empty store.
    pub fn open(repository: impl AsRef<Path>) -> Result<Self, GitEventStoreOpenError> {
        Self::open_for_authority(repository, GitEventStoreAuthority::Tiber)
    }

    /// Opens a store for one fixed semantic authority.  Authorities never
    /// share refs, replica IDs, pending-publication receipts, or state paths.
    pub fn open_for_authority(
        repository: impl AsRef<Path>,
        authority: GitEventStoreAuthority,
    ) -> Result<Self, GitEventStoreOpenError> {
        let repository = repository.as_ref().to_path_buf();
        let common_directory = git_path(&repository, ["rev-parse", "--git-common-dir"])?;
        let common_directory = if common_directory.is_absolute() {
            common_directory
        } else {
            repository.join(common_directory)
        };
        let stage = load_stage(&repository, &common_directory, authority)?;
        Ok(Self {
            repository,
            common_directory,
            authority,
            stage: Arc::new(Mutex::new(stage)),
            operation: Arc::new(Mutex::new(())),
        })
    }

    fn pending_publication_path(&self) -> PathBuf {
        self.common_directory
            .join(self.authority.config().state_directory)
            .join("pending-publication")
    }

    fn publication_blocker_path(&self) -> PathBuf {
        self.common_directory
            .join(self.authority.config().state_directory)
            .join("workflow-blocker.json")
    }

    fn command_state_snapshot_path(&self, snapshot_id: &CommandStateSnapshotId) -> PathBuf {
        let encoded = snapshot_id
            .as_ref()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.common_directory
            .join(self.authority.config().state_directory)
            .join("command-state-snapshots")
            .join(encoded)
    }

    async fn lock_common_operation(
        &self,
    ) -> Result<EventStoreOperationLock, GitEventStoreOpenError> {
        let common_directory = self.common_directory.clone();
        let authority = self.authority;
        tokio::task::spawn_blocking(move || {
            EventStoreOperationLock::acquire(&common_directory, authority)
        })
        .await
        .map_err(|error| GitEventStoreOpenError::Git(format!("operation lock failed: {error}")))?
    }

    async fn refresh(&self, operation: Operation) -> Result<(), EventStoreError> {
        let refreshed = load_stage(&self.repository, &self.common_directory, self.authority)
            .map_err(|_| store_failure(operation))?;
        *self.stage.lock().await = refreshed;
        Ok(())
    }

    pub async fn synchronize(&self) -> Result<SynchronizeOutcome, GitEventStoreOpenError> {
        let _operation = self.operation.lock().await;
        let _common_operation = self.lock_common_operation().await?;
        let path = self.pending_publication_path();
        if !path.exists() {
            *self.stage.lock().await =
                load_stage(&self.repository, &self.common_directory, self.authority)?;
            clear_publication_failure(&self.publication_blocker_path())?;
            return Ok(SynchronizeOutcome::Current);
        }
        let pending = read_pending(&path)?;
        validate_pending(&self.repository, &pending)?;
        let has_remote_authority = remote_authority_is_available(&self.repository, self.authority);
        let expected_authority = if has_remote_authority {
            "origin"
        } else {
            "local"
        };
        if pending.authority != expected_authority {
            return Err(GitEventStoreOpenError::Git(
                "pending publication authority changed".into(),
            ));
        }
        let head = if has_remote_authority {
            refresh_remote(&self.repository, self.authority)?
        } else {
            resolve_optional_ref(&self.repository, self.authority.config().local_ref)?
        };
        let pending_is_noop = if let Some(base) = pending.base.as_deref() {
            commit_tree_id(&self.repository, &pending.candidate)?
                == commit_tree_id(&self.repository, base)?
        } else {
            false
        };
        let outcome = if pending_is_noop {
            SynchronizeOutcome::Current
        } else if head
            .as_deref()
            .is_some_and(|head| is_ancestor(&self.repository, &pending.candidate, head))
        {
            SynchronizeOutcome::PublishedPending
        } else if head == pending.base {
            match if has_remote_authority {
                publish_remote(
                    &self.repository,
                    &pending.candidate,
                    pending.base.as_deref(),
                    self.authority,
                )?
            } else {
                publish_local(
                    &self.repository,
                    &pending.candidate,
                    pending.base.as_deref(),
                    self.authority,
                )?
            } {
                Publication::Confirmed => SynchronizeOutcome::PublishedPending,
                Publication::Conflict => SynchronizeOutcome::DiscardedUnpublished,
            }
        } else {
            SynchronizeOutcome::DiscardedUnpublished
        };
        remove_pending(&path)?;
        *self.stage.lock().await =
            load_stage(&self.repository, &self.common_directory, self.authority)?;
        clear_publication_failure(&self.publication_blocker_path())?;
        Ok(outcome)
    }
}

impl EventStore for GitEventStore {
    async fn read_stream<E: Event>(
        &self,
        stream_id: StreamId,
    ) -> Result<EventStream<E>, EventStoreError> {
        let _operation = self.operation.lock().await;
        let _common_operation = self
            .lock_common_operation()
            .await
            .map_err(|_| store_failure(Operation::ReadStream))?;
        self.refresh(Operation::ReadStream).await?;
        self.stage.lock().await.store.read_stream(stream_id).await
    }

    async fn append_events(
        &self,
        writes: StreamWrites,
    ) -> Result<EventStreamSlice, EventStoreError> {
        let _operation = self.operation.lock().await;
        let _common_operation = self
            .lock_common_operation()
            .await
            .map_err(|error| diagnosed_store_failure(Operation::AppendEvents, &error))?;
        if self.pending_publication_path().exists() {
            return Err(store_failure(Operation::AppendEvents));
        }

        let expected_versions = writes
            .expected_versions()
            .iter()
            .map(|(stream_id, version)| (stream_id.clone(), *version))
            .collect::<Vec<_>>();
        let stage = load_stage_with_retry(&self.repository, &self.common_directory, self.authority)
            .map_err(|error| diagnosed_store_failure(Operation::AppendEvents, &error))?;
        let appended = stage.store.append_events(writes).await?;
        let Some(candidate) = create_candidate(&self.repository, &stage, self.authority)
            .map_err(|error| diagnosed_store_failure(Operation::AppendEvents, &error))?
        else {
            return Ok(appended);
        };
        #[cfg(test)]
        run_before_initial_publish_hook(&self.repository);

        let publication = if stage.has_remote_authority {
            publish_remote(
                &self.repository,
                &candidate,
                stage.base.as_deref(),
                self.authority,
            )
        } else {
            publish_local(
                &self.repository,
                &candidate,
                stage.base.as_deref(),
                self.authority,
            )
        };

        match publication {
            Ok(Publication::Confirmed) => {
                *self.stage.lock().await =
                    load_stage(&self.repository, &self.common_directory, self.authority)
                        .map_err(|_| store_failure(Operation::AppendEvents))?;
                Ok(appended)
            }
            Ok(Publication::Conflict) => {
                let mut refreshed =
                    load_stage(&self.repository, &self.common_directory, self.authority)
                        .map_err(|_| store_failure(Operation::AppendEvents))?;
                for _ in 0..PUBLICATION_RETRIES {
                    match actual_conflict(&refreshed.store, &expected_versions).await {
                        Ok(conflict) => {
                            *self.stage.lock().await = refreshed;
                            return Err(conflict);
                        }
                        Err(EventStoreError::StoreFailure { .. }) => {}
                        Err(error) => return Err(error),
                    }
                    // The conflict probe intentionally appends to its disposable stage.
                    // Reload the exact authority that will be unioned, and repeat the
                    // version check if authority moved between the probe and reload.
                    let merge_base =
                        load_stage(&self.repository, &self.common_directory, self.authority)
                            .map_err(|_| store_failure(Operation::AppendEvents))?;
                    if merge_base.base != refreshed.base {
                        refreshed = merge_base;
                        continue;
                    }
                    let merged = merge_disjoint_stage(
                        &self.common_directory,
                        merge_base,
                        &stage,
                        self.authority,
                    )
                    .map_err(|_| store_failure(Operation::AppendEvents))?;
                    let Some(rebased_candidate) =
                        create_candidate(&self.repository, &merged, self.authority)
                            .map_err(|_| store_failure(Operation::AppendEvents))?
                    else {
                        *self.stage.lock().await = merged;
                        return Ok(appended);
                    };
                    #[cfg(test)]
                    run_before_rebased_publish_hook(&self.repository);
                    match if merged.has_remote_authority {
                        publish_remote(
                            &self.repository,
                            &rebased_candidate,
                            merged.base.as_deref(),
                            self.authority,
                        )
                    } else {
                        publish_local(
                            &self.repository,
                            &rebased_candidate,
                            merged.base.as_deref(),
                            self.authority,
                        )
                    } {
                        Ok(Publication::Confirmed) => {
                            *self.stage.lock().await = load_stage(
                                &self.repository,
                                &self.common_directory,
                                self.authority,
                            )
                            .map_err(|_| store_failure(Operation::AppendEvents))?;
                            return Ok(appended);
                        }
                        Ok(Publication::Conflict) => {
                            refreshed = load_stage(
                                &self.repository,
                                &self.common_directory,
                                self.authority,
                            )
                            .map_err(|_| store_failure(Operation::AppendEvents))?;
                        }
                        Err(_) => {
                            persist_indeterminate(self, &rebased_candidate, &merged)?;
                            *self.stage.lock().await = merged;
                            return Err(store_failure(Operation::AppendEvents));
                        }
                    }
                }
                Err(store_failure(Operation::AppendEvents))
            }
            Err(_) => {
                persist_indeterminate(self, &candidate, &stage)?;
                *self.stage.lock().await = stage;
                Err(store_failure(Operation::AppendEvents))
            }
        }
    }

    async fn load_command_state_snapshot(
        &self,
        snapshot_id: CommandStateSnapshotId,
    ) -> Result<Option<CommandStateSnapshot>, EventStoreError> {
        let _operation = self.operation.lock().await;
        let _common_operation = self
            .lock_common_operation()
            .await
            .map_err(|_| store_failure(Operation::ReadStream))?;
        let path = self.command_state_snapshot_path(&snapshot_id);
        match fs::read_to_string(path) {
            Ok(body) => match serde_json::from_str(&body) {
                Ok(snapshot) => Ok(Some(snapshot)),
                Err(_) => Ok(None),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(store_failure(Operation::ReadStream)),
        }
    }

    async fn save_command_state_snapshot(
        &self,
        snapshot_id: CommandStateSnapshotId,
        snapshot: CommandStateSnapshot,
    ) -> Result<(), EventStoreError> {
        let _operation = self.operation.lock().await;
        let _common_operation = self
            .lock_common_operation()
            .await
            .map_err(|_| store_failure(Operation::AppendEvents))?;
        let path = self.command_state_snapshot_path(&snapshot_id);
        if let Ok(body) = fs::read_to_string(&path) {
            if let Ok(stored) = serde_json::from_str::<CommandStateSnapshot>(&body) {
                if !snapshot.covers(&stored) {
                    return Ok(());
                }
            }
        }
        let parent = path
            .parent()
            .ok_or_else(|| store_failure(Operation::AppendEvents))?;
        fs::create_dir_all(parent).map_err(|_| store_failure(Operation::AppendEvents))?;
        let snapshot_name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| store_failure(Operation::AppendEvents))?;
        let temporary = parent.join(format!(".{snapshot_name}.staging"));
        let body =
            serde_json::to_vec(&snapshot).map_err(|_| store_failure(Operation::AppendEvents))?;
        fs::write(&temporary, body).map_err(|_| store_failure(Operation::AppendEvents))?;
        fs::rename(&temporary, path).map_err(|_| store_failure(Operation::AppendEvents))
    }
}

#[cfg(test)]
type RebasedPublishHook = Box<dyn FnMut(&Path) + Send>;

#[cfg(test)]
static BEFORE_INITIAL_PUBLISH_HOOK: std::sync::Mutex<Option<RebasedPublishHook>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
static BEFORE_REBASED_PUBLISH_HOOK: std::sync::Mutex<Option<RebasedPublishHook>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
fn run_before_initial_publish_hook(repository: &Path) {
    if let Some(mut hook) = BEFORE_INITIAL_PUBLISH_HOOK.lock().unwrap().take() {
        hook(repository);
    }
}

#[cfg(test)]
fn run_before_rebased_publish_hook(repository: &Path) {
    if let Some(mut hook) = BEFORE_REBASED_PUBLISH_HOOK.lock().unwrap().take() {
        hook(repository);
    }
}

impl EventReader for GitEventStore {
    type Error = EventStoreError;

    async fn read_events<E: Event>(
        &self,
        filter: EventFilter,
        page: EventPage,
    ) -> Result<Vec<(E, StreamPosition)>, Self::Error> {
        let _operation = self.operation.lock().await;
        let _common_operation = self
            .lock_common_operation()
            .await
            .map_err(|_| store_failure(Operation::ReadStream))?;
        self.refresh(Operation::ReadStream).await?;
        self.stage
            .lock()
            .await
            .store
            .read_events(filter, page)
            .await
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Publication {
    Confirmed,
    Conflict,
}

fn remote_authority_is_available(repository: &Path, authority: GitEventStoreAuthority) -> bool {
    authority.config().remote.is_some()
        && git(repository, ["remote", "get-url", "origin"])
            .is_ok_and(|output| output.status.success())
}

fn load_stage(
    repository: &Path,
    common_directory: &Path,
    authority: GitEventStoreAuthority,
) -> Result<Stage, GitEventStoreOpenError> {
    let has_remote_authority = remote_authority_is_available(repository, authority);
    let base = if has_remote_authority {
        refresh_remote(repository, authority)?
    } else {
        resolve_optional_ref(repository, authority.config().local_ref)?
    };

    let directory = TempDir::new()?;
    let work_tree = directory.path().join("work-tree");
    fs::create_dir_all(&work_tree)?;
    if let Some(commit) = &base {
        checkout_tree(repository, commit, &work_tree)?;
    }
    let store = FileEventStore::open_with_config(
        FsConfig::new(work_tree.join(STORE_DIRECTORY))
            .with_replica_id(load_or_create_replica_id(common_directory, authority)?),
    )?;
    Ok(Stage {
        _directory: directory,
        work_tree,
        store,
        base,
        has_remote_authority,
    })
}

fn load_stage_with_retry(
    repository: &Path,
    common_directory: &Path,
    authority: GitEventStoreAuthority,
) -> Result<Stage, GitEventStoreOpenError> {
    let mut last = None;
    for _ in 0..STAGE_LOAD_RETRIES {
        match load_stage(repository, common_directory, authority) {
            Ok(stage) => return Ok(stage),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }
    Err(last.expect("retry loop always records an error"))
}

fn merge_disjoint_stage(
    common_directory: &Path,
    refreshed: Stage,
    original: &Stage,
    authority: GitEventStoreAuthority,
) -> Result<Stage, GitEventStoreOpenError> {
    let source = original.work_tree.join(STORE_DIRECTORY).join("events");
    let destination = refreshed.work_tree.join(STORE_DIRECTORY).join("events");
    fs::create_dir_all(&destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let target = destination.join(entry.file_name());
            if !target.exists() {
                fs::copy(entry.path(), target)?;
            }
        }
    }
    let Stage {
        _directory,
        work_tree,
        store,
        base,
        has_remote_authority,
    } = refreshed;
    drop(store);
    let store = FileEventStore::open_with_config(
        FsConfig::new(work_tree.join(STORE_DIRECTORY))
            .with_replica_id(load_or_create_replica_id(common_directory, authority)?),
    )?;
    Ok(Stage {
        _directory,
        work_tree,
        store,
        base,
        has_remote_authority,
    })
}

fn persist_indeterminate(
    event_store: &GitEventStore,
    candidate: &str,
    stage: &Stage,
) -> Result<(), EventStoreError> {
    persist_pending(
        &event_store.pending_publication_path(),
        &PendingPublication {
            version: PENDING_VERSION,
            candidate: candidate.to_owned(),
            base: stage.base.clone(),
            authority: if stage.has_remote_authority {
                "origin"
            } else {
                "local"
            }
            .to_owned(),
        },
    )
    .map_err(|_| store_failure(Operation::AppendEvents))?;
    record_publication_failure(
        &event_store.publication_blocker_path(),
        event_store.authority,
    )
    .map_err(|_| store_failure(Operation::AppendEvents))
}

fn record_publication_failure(
    path: &Path,
    authority: GitEventStoreAuthority,
) -> Result<(), GitEventStoreOpenError> {
    let (error_code, required_action) = match authority {
        GitEventStoreAuthority::Tiber => (
            "tiber.publication_failed",
            "run Tiber sync until authoritative publication is resolved",
        ),
        GitEventStoreAuthority::DevelopmentWorkflow => (
            "development_workflow.publication_failed",
            "synchronize Development Workflow until authoritative publication is resolved",
        ),
        GitEventStoreAuthority::PluginAdvisoryFinalReview => (
            "plugin_advisory_final_review.publication_failed",
            "synchronize plugin advisory final review until local publication is resolved",
        ),
    };
    let marker = PublicationBlocker {
        schema_version: 1,
        kind: "publication_failed",
        error_code,
        required_action,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_secs(),
    };
    let parent = path.parent().ok_or_else(|| {
        GitEventStoreOpenError::Git("publication blocker has no parent directory".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&marker).map_err(std::io::Error::other)?,
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn clear_publication_failure(path: &Path) -> Result<(), GitEventStoreOpenError> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let kind = serde_json::from_slice::<serde_json::Value>(&contents)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    if kind.as_deref() == Some("publication_failed") {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn refresh_remote(
    repository: &Path,
    authority: GitEventStoreAuthority,
) -> Result<Option<String>, GitEventStoreOpenError> {
    let authority = authority.config().remote.ok_or_else(|| {
        GitEventStoreOpenError::Git("selected authority has no remote publication ref".to_owned())
    })?;
    let advertised = git(
        repository,
        ["ls-remote", "--exit-code", "origin", authority.head],
    )?;
    match advertised.status.code() {
        Some(0) => {
            require_success(git(
                repository,
                [
                    "fetch",
                    "--no-tags",
                    "origin",
                    &format!("+{}:{}", authority.head, authority.tracking_ref),
                ],
            )?)?;
            resolve_optional_ref(repository, authority.tracking_ref)
        }
        Some(2) => Ok(None),
        _ => Err(git_error("refresh authoritative tiber ref", &advertised)),
    }
}

fn checkout_tree(
    repository: &Path,
    commit: &str,
    work_tree: &Path,
) -> Result<(), GitEventStoreOpenError> {
    let index = work_tree.join("git-index");
    require_success(git_with(
        repository,
        None,
        [("GIT_INDEX_FILE", index.as_os_str())],
        ["read-tree", commit],
    )?)?;
    require_success(git_with(
        repository,
        Some(work_tree),
        [("GIT_INDEX_FILE", index.as_os_str())],
        ["checkout-index", "--all", "--force"],
    )?)?;
    let _ = fs::remove_file(index);
    Ok(())
}

fn create_candidate(
    repository: &Path,
    stage: &Stage,
    authority: GitEventStoreAuthority,
) -> Result<Option<String>, GitEventStoreOpenError> {
    let index = stage.work_tree.join("candidate-index");
    let index_env = [("GIT_INDEX_FILE", index.as_os_str())];
    if let Some(base) = &stage.base {
        require_success(git_with(repository, None, index_env, ["read-tree", base])?)?;
    } else {
        require_success(git_with(
            repository,
            None,
            index_env,
            ["read-tree", "--empty"],
        )?)?;
    }
    let baseline_tree = output_text(require_success(git_with(
        repository,
        None,
        index_env,
        ["write-tree"],
    )?)?);
    require_success(git_with(
        repository,
        Some(&stage.work_tree),
        index_env,
        ["add", "--all", "--", &format!("{STORE_DIRECTORY}/events")],
    )?)?;
    let tree = output_text(require_success(git_with(
        repository,
        None,
        index_env,
        ["write-tree"],
    )?)?);
    if tree == baseline_tree {
        let _ = fs::remove_file(index);
        return Ok(None);
    }
    let signing = git(repository, ["config", "--bool", "commit.gpgsign"])?;
    let mut arguments = vec!["commit-tree", tree.as_str()];
    if signing.status.success() && output_text(signing) == "true" {
        arguments.push("-S");
    }
    arguments.extend(["-m", authority.config().commit_message]);
    if let Some(base) = &stage.base {
        arguments.extend(["-p", base.as_str()]);
    }
    let author_identity = git(repository, ["var", "GIT_AUTHOR_IDENT"])?;
    let committer_identity = git(repository, ["var", "GIT_COMMITTER_IDENT"])?;
    let mut commit_environment = vec![("GIT_INDEX_FILE", index.as_os_str())];
    if !author_identity.status.success() {
        commit_environment.extend([
            ("GIT_AUTHOR_NAME", OsStr::new(FALLBACK_GIT_NAME)),
            ("GIT_AUTHOR_EMAIL", OsStr::new(FALLBACK_GIT_EMAIL)),
        ]);
    }
    if !committer_identity.status.success() {
        commit_environment.extend([
            ("GIT_COMMITTER_NAME", OsStr::new(FALLBACK_GIT_NAME)),
            ("GIT_COMMITTER_EMAIL", OsStr::new(FALLBACK_GIT_EMAIL)),
        ]);
    }
    let candidate = output_text(require_success(git_with(
        repository,
        None,
        commit_environment,
        arguments,
    )?)?);
    let _ = fs::remove_file(index);
    Ok(Some(candidate))
}

fn commit_tree_id(repository: &Path, commit: &str) -> Result<String, GitEventStoreOpenError> {
    let tree_expression = format!("{commit}^{{tree}}");
    Ok(output_text(require_success(git(
        repository,
        ["rev-parse", "--verify", tree_expression.as_str()],
    )?)?))
}

fn publish_remote(
    repository: &Path,
    candidate: &str,
    base: Option<&str>,
    authority: GitEventStoreAuthority,
) -> Result<Publication, GitEventStoreOpenError> {
    let authority_config = authority.config().remote.ok_or_else(|| {
        GitEventStoreOpenError::Git("selected authority has no remote publication ref".to_owned())
    })?;
    for _ in 0..PUBLICATION_RETRIES {
        let push = git(
            repository,
            [
                "push",
                "origin",
                &format!("{candidate}:{}", authority_config.head),
            ],
        )?;
        let remote = refresh_remote(repository, authority)?;
        if remote
            .as_deref()
            .is_some_and(|head| is_ancestor(repository, candidate, head))
        {
            return Ok(Publication::Confirmed);
        }
        if remote.as_deref() != base {
            return Ok(Publication::Conflict);
        }
        if push.status.success() {
            break;
        }
    }
    Err(GitEventStoreOpenError::Git(
        "publication outcome remained indeterminate".to_owned(),
    ))
}

fn publish_local(
    repository: &Path,
    candidate: &str,
    base: Option<&str>,
    authority: GitEventStoreAuthority,
) -> Result<Publication, GitEventStoreOpenError> {
    let authority = authority.config();
    let expected = base.unwrap_or("0000000000000000000000000000000000000000");
    let update = git(
        repository,
        ["update-ref", authority.local_ref, candidate, expected],
    )?;
    if update.status.success() {
        return Ok(Publication::Confirmed);
    }
    let current = resolve_optional_ref(repository, authority.local_ref)?;
    if current
        .as_deref()
        .is_some_and(|head| is_ancestor(repository, candidate, head))
    {
        Ok(Publication::Confirmed)
    } else {
        Ok(Publication::Conflict)
    }
}

fn is_ancestor(repository: &Path, ancestor: &str, descendant: &str) -> bool {
    git(
        repository,
        ["merge-base", "--is-ancestor", ancestor, descendant],
    )
    .is_ok_and(|output| output.status.success())
}

fn resolve_optional_ref(
    repository: &Path,
    reference: &str,
) -> Result<Option<String>, GitEventStoreOpenError> {
    let output = git(repository, ["rev-parse", "--verify", reference])?;
    if output.status.success() {
        Ok(Some(output_text(output)))
    } else {
        Ok(None)
    }
}

fn persist_pending(path: &Path, pending: &PendingPublication) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let encoded = serde_json::to_vec(pending).map_err(std::io::Error::other)?;
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, path)
}

fn read_pending(path: &Path) -> Result<PendingPublication, GitEventStoreOpenError> {
    let pending: PendingPublication =
        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
            GitEventStoreOpenError::Git(format!("pending publication is invalid: {error}"))
        })?;
    if pending.version != PENDING_VERSION {
        return Err(GitEventStoreOpenError::Git(format!(
            "unsupported pending publication version {}",
            pending.version
        )));
    }
    Ok(pending)
}

fn validate_pending(
    repository: &Path,
    pending: &PendingPublication,
) -> Result<(), GitEventStoreOpenError> {
    if !is_full_object_id(&pending.candidate)
        || pending
            .base
            .as_deref()
            .is_some_and(|base| !is_full_object_id(base))
    {
        return Err(GitEventStoreOpenError::Git(
            "pending publication contains an invalid object id".into(),
        ));
    }
    let object = format!("{}^{{commit}}", pending.candidate);
    require_success(git(repository, ["cat-file", "-e", &object])?)?;
    let parents = output_text(require_success(git(
        repository,
        ["rev-list", "--parents", "-n", "1", &pending.candidate],
    )?)?);
    let fields = parents.split_whitespace().collect::<Vec<_>>();
    if fields.is_empty() || fields.len() > 2 || fields.get(1).copied() != pending.base.as_deref() {
        return Err(GitEventStoreOpenError::Git(
            "pending publication parent mismatch".into(),
        ));
    }
    Ok(())
}

fn is_full_object_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn remove_pending(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn load_or_create_replica_id(
    common_directory: &Path,
    authority: GitEventStoreAuthority,
) -> Result<Uuid, GitEventStoreOpenError> {
    let directory = common_directory.join(authority.config().state_directory);
    fs::create_dir_all(&directory)?;
    let path = directory.join("replica-id");
    if let Ok(value) = fs::read_to_string(&path) {
        return Uuid::parse_str(value.trim()).map_err(|error| {
            GitEventStoreOpenError::Git(format!("invalid replica identity: {error}"))
        });
    }
    let replica_id = Uuid::now_v7();
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            writeln!(file, "{replica_id}")?;
            file.sync_all()?;
            Ok(replica_id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let value = fs::read_to_string(path)?;
            Uuid::parse_str(value.trim()).map_err(|source| {
                GitEventStoreOpenError::Git(format!("invalid replica identity: {source}"))
            })
        }
        Err(error) => Err(error.into()),
    }
}

async fn actual_conflict(
    store: &FileEventStore,
    expected_versions: &[(StreamId, StreamVersion)],
) -> Result<EventStoreError, EventStoreError> {
    for (stream_id, expected) in expected_versions {
        let probe = StreamWrites::new()
            .register_stream(stream_id.clone(), *expected)?
            .append(ConflictProbe {
                stream_id: stream_id.clone(),
            })?;
        match store.append_events(probe).await {
            Err(conflict @ EventStoreError::VersionConflict { .. }) => return Ok(conflict),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Err(store_failure(Operation::AppendEvents))
}

#[derive(Clone, Deserialize, Serialize)]
struct ConflictProbe {
    stream_id: StreamId,
}

impl Event for ConflictProbe {
    fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }
    fn event_type_name() -> &'static str {
        "tiber.git_event_store.conflict_probe"
    }
}

fn git_path<const N: usize>(
    repository: &Path,
    arguments: [&str; N],
) -> Result<PathBuf, GitEventStoreOpenError> {
    Ok(PathBuf::from(output_text(require_success(git(
        repository, arguments,
    )?)?)))
}

fn git<I, S>(repository: &Path, arguments: I) -> Result<Output, GitEventStoreOpenError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_with(
        repository,
        None,
        std::iter::empty::<(&str, &OsStr)>(),
        arguments,
    )
}

fn git_with<I, S, E, K, V>(
    repository: &Path,
    work_tree: Option<&Path>,
    environment: E,
    arguments: I,
) -> Result<Output, GitEventStoreOpenError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.arg("-C").arg(repository);
    if let Some(work_tree) = work_tree {
        command.arg(format!("--work-tree={}", work_tree.display()));
    }
    command
        .envs(environment)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(arguments);
    let mut child = command.spawn()?;
    match child.wait_timeout(GIT_TIMEOUT)? {
        Some(_) => child.wait_with_output().map_err(GitEventStoreOpenError::Io),
        None => {
            child.kill()?;
            let _ = child.wait();
            Err(GitEventStoreOpenError::Git(
                "Git command timed out".to_owned(),
            ))
        }
    }
}

fn require_success(output: Output) -> Result<Output, GitEventStoreOpenError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error("execute Git command", &output))
    }
}

fn git_error(operation: &str, output: &Output) -> GitEventStoreOpenError {
    GitEventStoreOpenError::Git(format!(
        "{operation}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn output_text(output: Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn store_failure(operation: Operation) -> EventStoreError {
    EventStoreError::StoreFailure { operation }
}

fn diagnosed_store_failure(
    operation: Operation,
    error: &GitEventStoreOpenError,
) -> EventStoreError {
    if std::env::var_os("TIBER_EVENT_STORE_DIAGNOSTICS").is_some() {
        eprintln!("tiber_git.event_store_failure operation={operation:?} source={error}");
    }
    store_failure(operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eventcore_types::collect_events;

    static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Clone, Deserialize, Serialize)]
    struct TestEvent {
        stream_id: StreamId,
    }

    impl Event for TestEvent {
        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }

        fn event_type_name() -> &'static str {
            "tiber.git_event_store.test"
        }
    }

    fn writes(stream: &StreamId) -> StreamWrites {
        StreamWrites::new()
            .register_stream(stream.clone(), StreamVersion::new(0))
            .unwrap()
            .append(TestEvent {
                stream_id: stream.clone(),
            })
            .unwrap()
    }

    fn empty_writes(stream: &StreamId) -> StreamWrites {
        StreamWrites::new()
            .register_stream(stream.clone(), StreamVersion::new(0))
            .unwrap()
    }

    fn require_git(repository: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[tokio::test]
    async fn event_transactions_use_the_repository_git_identity() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Verified Owner"]);
        require_git(
            &repository,
            &["config", "user.email", "verified-owner@example.com"],
        );

        let stream = StreamId::try_new("tiber:task:adapter-identity").unwrap();
        GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&stream))
            .await
            .unwrap();

        let expected_author_name =
            std::env::var("GIT_AUTHOR_NAME").unwrap_or_else(|_| "Verified Owner".to_owned());
        let expected_author_email = std::env::var("GIT_AUTHOR_EMAIL")
            .unwrap_or_else(|_| "verified-owner@example.com".to_owned());
        assert_eq!(
            require_git(
                &repository,
                &[
                    "show",
                    "--no-patch",
                    "--no-show-signature",
                    "--format=%an <%ae>|%cn <%ce>",
                    "refs/heads/tiber",
                ],
            ),
            format!(
                "{expected_author_name} <{expected_author_email}>|\
                 Verified Owner <verified-owner@example.com>"
            )
        );
    }

    #[tokio::test]
    async fn second_same_stream_advance_is_reprobed_before_rebased_publication() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        let origin = directory.path().join("origin.git");
        require_git(
            directory.path(),
            &["init", "--bare", origin.to_str().unwrap()],
        );
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Tiber Test"]);
        require_git(
            &repository,
            &["config", "user.email", "tiber@example.invalid"],
        );
        require_git(
            &repository,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );

        let disjoint_stream = StreamId::try_new("tiber:task:disjoint").unwrap();
        let contested_stream = StreamId::try_new("tiber:task:contested").unwrap();
        GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&disjoint_stream))
            .await
            .unwrap();
        let disjoint_head = require_git(&repository, &["rev-parse", REMOTE_REF]);

        GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&contested_stream))
            .await
            .unwrap();
        let winner_head = require_git(&repository, &["rev-parse", REMOTE_REF]);
        require_git(&origin, &["update-ref", "-d", REMOTE_HEAD, &winner_head]);

        let initial_origin = origin.clone();
        let initial_disjoint = disjoint_head.clone();
        *BEFORE_INITIAL_PUBLISH_HOOK.lock().unwrap() = Some(Box::new(move |_| {
            require_git(
                &initial_origin,
                &["update-ref", REMOTE_HEAD, &initial_disjoint],
            );
        }));

        let hook_origin = origin.clone();
        let hook_winner = winner_head.clone();
        let hook_expected = disjoint_head.clone();
        *BEFORE_REBASED_PUBLISH_HOOK.lock().unwrap() = Some(Box::new(move |_| {
            require_git(
                &hook_origin,
                &["update-ref", REMOTE_HEAD, &hook_winner, &hook_expected],
            );
        }));

        let result = GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&contested_stream))
            .await;
        assert!(matches!(
            result,
            Err(EventStoreError::VersionConflict { .. })
        ));
        let reopened = GitEventStore::open(&repository).unwrap();
        assert_eq!(
            collect_events(
                reopened
                    .read_stream::<TestEvent>(contested_stream)
                    .await
                    .unwrap()
            )
            .await
            .unwrap()
            .len(),
            1,
            "the losing candidate must not create an eventcore-fs fork"
        );
        assert_eq!(
            collect_events(
                reopened
                    .read_stream::<TestEvent>(disjoint_stream)
                    .await
                    .unwrap()
            )
            .await
            .unwrap()
            .len(),
            1
        );
    }

    #[tokio::test]
    async fn ambiguous_rebased_publication_persists_the_exact_candidate() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        let origin = directory.path().join("origin.git");
        require_git(
            directory.path(),
            &["init", "--bare", origin.to_str().unwrap()],
        );
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Tiber Test"]);
        require_git(
            &repository,
            &["config", "user.email", "tiber@example.invalid"],
        );
        require_git(
            &repository,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );

        let authority_stream = StreamId::try_new("tiber:task:authority").unwrap();
        GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&authority_stream))
            .await
            .unwrap();
        let authority = require_git(&repository, &["rev-parse", REMOTE_REF]);
        require_git(&origin, &["update-ref", "-d", REMOTE_HEAD, &authority]);

        let initial_origin = origin.clone();
        let initial_authority = authority.clone();
        *BEFORE_INITIAL_PUBLISH_HOOK.lock().unwrap() = Some(Box::new(move |_| {
            require_git(
                &initial_origin,
                &["update-ref", REMOTE_HEAD, &initial_authority],
            );
        }));
        let receive_hook = origin.join("hooks/pre-receive");
        fs::write(&receive_hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receive_hook, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let pending_stream = StreamId::try_new("tiber:task:pending").unwrap();
        assert!(GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&pending_stream))
            .await
            .is_err());

        let marker: PendingPublication = serde_json::from_slice(
            &fs::read(repository.join(".git/tiber/pending-publication")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker.base.as_deref(), Some(authority.as_str()));
        let parent = require_git(
            &repository,
            &["rev-parse", &format!("{}^", marker.candidate)],
        );
        assert_eq!(parent, authority);
        assert!(repository.join(".git/tiber/workflow-blocker.json").exists());
    }

    #[tokio::test]
    async fn no_op_append_does_not_create_an_empty_authority() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Tiber Test"]);
        require_git(
            &repository,
            &["config", "user.email", "tiber@example.invalid"],
        );
        require_git(&repository, &["config", "commit.gpgsign", "false"]);

        let stream = StreamId::try_new("tiber:task:no-op").unwrap();
        let store = GitEventStore::open(&repository).unwrap();
        store.append_events(empty_writes(&stream)).await.unwrap();
        assert!(!require_git(
            &repository,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objecttype)"
            ]
        )
        .lines()
        .any(|object_type| object_type == "commit"));
        assert!(!git(
            &repository,
            ["show-ref", "--verify", store.authority.config().local_ref]
        )
        .unwrap()
        .status
        .success());
        assert!(!store.pending_publication_path().exists());
        assert!(!store.publication_blocker_path().exists());
    }

    #[tokio::test]
    async fn synchronize_clears_a_legacy_pending_candidate_with_an_unchanged_tree() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Tiber Test"]);
        require_git(
            &repository,
            &["config", "user.email", "tiber@example.invalid"],
        );
        require_git(&repository, &["config", "commit.gpgsign", "false"]);

        let stream = StreamId::try_new("tiber:task:authority").unwrap();
        let store = GitEventStore::open(&repository).unwrap();
        store.append_events(writes(&stream)).await.unwrap();
        let local_ref = store.authority.config().local_ref;
        let authority = require_git(&repository, &["rev-parse", local_ref]);
        let tree = require_git(
            &repository,
            &["rev-parse", &format!("{authority}^{{tree}}")],
        );
        let candidate = require_git(
            &repository,
            &[
                "commit-tree",
                &tree,
                "-p",
                &authority,
                "-m",
                "legacy no-op candidate",
            ],
        );
        persist_pending(
            &store.pending_publication_path(),
            &PendingPublication {
                version: PENDING_VERSION,
                candidate,
                base: Some(authority.clone()),
                authority: "local".to_owned(),
            },
        )
        .unwrap();
        record_publication_failure(&store.publication_blocker_path(), store.authority).unwrap();

        assert_eq!(
            store.synchronize().await.unwrap(),
            SynchronizeOutcome::Current
        );
        assert_eq!(
            require_git(&repository, &["rev-parse", local_ref]),
            authority
        );
        assert!(!store.pending_publication_path().exists());
        assert!(!store.publication_blocker_path().exists());
    }

    #[tokio::test]
    async fn authorities_keep_refs_replica_state_and_streams_independent() {
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Event Store Test"]);
        require_git(
            &repository,
            &["config", "user.email", "event-store@example.invalid"],
        );

        let stream = StreamId::try_new("workflow:shared-stream-name").unwrap();
        GitEventStore::open(&repository)
            .unwrap()
            .append_events(writes(&stream))
            .await
            .unwrap();
        GitEventStore::open_for_authority(&repository, GitEventStoreAuthority::DevelopmentWorkflow)
            .unwrap()
            .append_events(writes(&stream))
            .await
            .unwrap();

        assert!(!require_git(&repository, &["rev-parse", "refs/heads/tiber"]).is_empty());
        assert!(!require_git(
            &repository,
            &["rev-parse", "refs/heads/development-workflow"],
        )
        .is_empty());
        assert!(repository.join(".git/tiber/replica-id").exists());
        assert!(repository
            .join(".git/development-workflow/replica-id")
            .exists());

        let workflow_blocker = repository.join(".git/development-workflow/workflow-blocker.json");
        record_publication_failure(
            &workflow_blocker,
            GitEventStoreAuthority::DevelopmentWorkflow,
        )
        .unwrap();
        assert!(workflow_blocker.exists());
        assert!(!repository.join(".git/tiber/workflow-blocker.json").exists());
        clear_publication_failure(&workflow_blocker).unwrap();
        assert!(!workflow_blocker.exists());

        let development_workflow = GitEventStore::open_for_authority(
            &repository,
            GitEventStoreAuthority::DevelopmentWorkflow,
        )
        .unwrap();
        assert_eq!(
            collect_events(
                development_workflow
                    .read_stream::<TestEvent>(stream)
                    .await
                    .unwrap()
            )
            .await
            .unwrap()
            .len(),
            1
        );
    }

    #[tokio::test]
    async fn plugin_advisory_final_review_stays_local_while_workflow_publishes() {
        let _serial = TEST_SERIAL.lock().await;
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("repository");
        let origin = directory.path().join("origin.git");
        require_git(
            directory.path(),
            &["init", "--bare", origin.to_str().unwrap()],
        );
        require_git(directory.path(), &["init", repository.to_str().unwrap()]);
        require_git(&repository, &["config", "user.name", "Event Store Test"]);
        require_git(
            &repository,
            &["config", "user.email", "event-store@example.invalid"],
        );
        require_git(
            &repository,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );

        let advisory_stream = StreamId::try_new("review:plugin-advisory").unwrap();
        GitEventStore::open_for_authority(
            &repository,
            GitEventStoreAuthority::PluginAdvisoryFinalReview,
        )
        .unwrap()
        .append_events(writes(&advisory_stream))
        .await
        .unwrap();

        assert!(!require_git(
            &repository,
            &["rev-parse", "refs/tiber/plugin-advisory-final-review"],
        )
        .is_empty());
        assert!(
            git(&origin, ["show-ref"]).unwrap().stdout.is_empty(),
            "advisory review must not publish any remote ref"
        );
        assert!(repository
            .join(".git/plugin-advisory-final-review/replica-id")
            .exists());
        assert!(!repository
            .join(".git/development-workflow/replica-id")
            .exists());

        let workflow_stream = StreamId::try_new("workflow:remote-authority").unwrap();
        GitEventStore::open_for_authority(&repository, GitEventStoreAuthority::DevelopmentWorkflow)
            .unwrap()
            .append_events(writes(&workflow_stream))
            .await
            .unwrap();

        assert!(
            !require_git(&origin, &["rev-parse", "refs/heads/development-workflow"],).is_empty()
        );
        assert!(!require_git(
            &repository,
            &["rev-parse", "refs/remotes/origin/development-workflow"],
        )
        .is_empty());
        assert!(repository
            .join(".git/development-workflow/replica-id")
            .exists());

        let advisory = GitEventStore::open_for_authority(
            &repository,
            GitEventStoreAuthority::PluginAdvisoryFinalReview,
        )
        .unwrap();
        assert_eq!(
            collect_events(
                advisory
                    .read_stream::<TestEvent>(advisory_stream)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            collect_events(
                advisory
                    .read_stream::<TestEvent>(workflow_stream)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap()
            .len(),
            0
        );
    }
}
