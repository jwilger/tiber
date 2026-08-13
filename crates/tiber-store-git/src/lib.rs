//! Signed `EventCore` authority materialization and one-shot Tiber publication.
//!
//! This adapter resolves one fixed authority revision, verifies its signature,
//! materializes it in disposable storage, and exposes typed `EventCore` reads.
//! Its [`publication`] module supplies the deliberately separate, narrow
//! one-shot signed append boundary; this crate never implements a generic
//! writable `EventStore` or broad task authority.

#![forbid(unsafe_code)]

extern crate alloc;

/// Signed native Tiber publication boundary.
#[path = "publisher.rs"]
pub mod publication;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    string::String,
    vec::Vec,
};
use core::{error::Error, marker::PhantomData, str, time::Duration};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt as _;

use eventcore_fs::{FileEventStore, FsConfig, FsyncPolicy};
use eventcore_types::{
    BatchSize, Event, EventFilter, EventPage, EventReader as _, EventStoreError, Operation,
    StreamId, StreamPattern, StreamPosition,
};
use serde::Deserialize;
use tempfile::TempDir;
use wait_timeout::ChildExt as _;

/// The sole local authority ref, used only if no `origin` remote exists.
pub const TIBER_REF: &str = "refs/heads/tiber";

/// The only remote authority ref the adapter will query.
const ORIGIN_TIBER_REF: &str = "refs/heads/tiber";
/// The committed root of the `eventcore-fs` history.
const EVENT_STORE_DIRECTORY: &str = "eventstore";
/// The immutable `EventCore` transaction directory.
const EVENTS_DIRECTORY: &str = "events";
/// Exact `EventCore` directories that may be created or used for derived state.
const EVENTCORE_DERIVED_DIRECTORIES: [&str; 4] =
    ["tmp", "index", ".eventcore", ".eventcore/snapshots"];
/// Exact `EventCore` files that may be created or overwritten while opening a store.
const EVENTCORE_DERIVED_FILES: [&str; 6] = [
    ".lock",
    ".gitignore",
    ".gitattributes",
    ".eventcore/replica_id",
    ".eventcore/replica_fingerprint",
    "index/ingestion.log",
];
/// Short bound for local Git inspection and snapshot materialization operations.
const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound for the first isolated remote authority refresh, including a full object fetch.
const REMOTE_AUTHORITY_GIT_TIMEOUT: Duration = Duration::from_mins(1);
/// Git's expected status for an absent named remote.
const GIT_NOT_FOUND_EXIT: i32 = 2;
/// Git's expected status when a requested local configuration key is absent.
const GIT_CONFIG_NOT_FOUND_EXIT: i32 = 1;
/// The Git pathname prefix that preserves an absent optional configured file.
const OPTIONAL_GIT_PATH_PREFIX: &str = ":(optional)";

/// A canonical Git object name resolved at the fixed authority boundary.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TiberRevision(String);

impl TiberRevision {
    /// Returns this exact revision's canonical object name.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "a tiny borrowed accessor reads most clearly as its final expression"
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a SHA-1 or SHA-256 object name emitted by a fixed Git query.
    #[expect(
        clippy::implicit_return,
        reason = "the closed parse result reads most clearly as the final boundary expression"
    )]
    fn parse(input: &str) -> Result<Self, ()> {
        let valid_length = matches!(input.len(), 40 | 64);
        let valid_hex = input.bytes().all(|byte| byte.is_ascii_hexdigit());
        if valid_length && valid_hex {
            Ok(Self(input.to_owned()))
        } else {
            Err(())
        }
    }
}

/// The Git object format used to store objects in one caller repository.
///
/// Git currently exposes these two storage formats. This private boundary keeps
/// the disposable authority repository compatible with the caller without
/// widening the accepted authority protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitObjectFormat {
    /// Git's SHA-1 object storage format.
    Sha1,
    /// Git's SHA-256 object storage format.
    Sha256,
}

impl GitObjectFormat {
    /// Returns the exact fixed Git argument used when initializing a repository.
    #[expect(
        clippy::implicit_return,
        reason = "the closed storage-format mapping reads most clearly as its final expression"
    )]
    const fn init_argument(self) -> &'static str {
        match self {
            Self::Sha1 => "--object-format=sha1",
            Self::Sha256 => "--object-format=sha256",
        }
    }

    /// Parses Git's newline-terminated storage object-format output exactly.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the exact closed parser keeps unsupported object formats outside the authority boundary"
    )]
    fn parse_storage_output(output: &[u8]) -> Result<Self, ()> {
        match output {
            b"sha1\n" => Ok(Self::Sha1),
            b"sha256\n" => Ok(Self::Sha256),
            _ => Err(()),
        }
    }
}

/// The semantic class of one bounded Git invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GitOperation {
    /// Append a named `EventCore` transaction to a disposable publication stage.
    AppendTiberEvents,
    /// Materialize a verified revision in disposable storage.
    MaterializeTiberSnapshot,
    /// Publish a signed candidate transaction to the fixed Tiber authority.
    PublishTiberEvents,
    /// Resolve or fetch the remote authority ref.
    RefreshOriginTiberRef,
    /// Resolve the origin configuration or local authority ref.
    ResolveTiberRef,
    /// Sign one candidate commit before authority publication.
    SignTiberCandidate,
    /// Verify an authority commit signature.
    VerifyTiberSignature,
}

/// Whether retrying a typed adapter failure can plausibly change its outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the retryable outcome is presented before the terminal outcome to communicate recovery policy"
)]
pub enum Retryability {
    /// A bounded retry may succeed after a transient process or transport failure.
    Retryable,
    /// Retrying unchanged inputs cannot repair a rejected authority or history.
    Permanent,
}

/// The direct process-level reason a fixed Git invocation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "failure kinds are grouped by process lifecycle rather than alphabetically"
)]
pub enum GitCommandFailureKind {
    /// The host could not spawn, wait for, or collect the Git child.
    Io,
    /// The Git child exceeded the adapter's fixed deadline and was reaped.
    TimedOut,
    /// Git exited unsuccessfully without exposing its stderr.
    NonZeroExit,
}

/// Itemized, sanitized context retained for one failed Git process.
///
/// Git stderr is intentionally never captured. The optional I/O source is an
/// operating-system cause, while `exit_code` preserves only Git's numeric
/// result for owner-facing recovery policy.
#[derive(Debug)]
pub struct GitCommandFailure {
    /// A numeric Git exit code when a child exited unsuccessfully.
    exit_code: Option<i32>,
    /// An operating-system cause for spawn, wait, or collection failures.
    io_source: Option<io::Error>,
    /// The process failure class.
    kind: GitCommandFailureKind,
    /// The fixed semantic operation that invoked Git.
    operation: GitOperation,
    /// Whether a bounded retry is meaningful for this exact failure.
    retryability: Retryability,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the constructors and public inspectors follow process-failure data flow rather than alphabetical ordering"
)]
impl GitCommandFailure {
    /// Retains a host I/O cause without retaining command output.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the complete I/O context is clearest as the constructor's final expression"
    )]
    fn io(operation: GitOperation, source: io::Error) -> Self {
        Self {
            operation,
            kind: GitCommandFailureKind::Io,
            retryability: retryability_for_io(source.kind()),
            exit_code: None,
            io_source: Some(source),
        }
    }

    /// Returns the semantic operation that failed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied operation is clearest as the accessor's final expression"
    )]
    pub const fn operation(&self) -> GitOperation {
        self.operation
    }

    /// Returns the direct process-failure class.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the copied process-failure kind is clearest as the accessor's final expression"
    )]
    pub const fn kind(&self) -> GitCommandFailureKind {
        self.kind
    }

    /// Returns whether retrying this exact process failure is meaningful.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stored retryability is clearest as the accessor's final expression"
    )]
    pub const fn retryability(&self) -> Retryability {
        self.retryability
    }

    /// Returns Git's numeric unsuccessful exit code, if one was observed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the retained optional numeric exit code is clearest as the accessor's final expression"
    )]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the retained host I/O cause, if process setup or collection failed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the optional retained operating-system cause is clearest as the accessor's final expression"
    )]
    pub fn io_source(&self) -> Option<&io::Error> {
        self.io_source.as_ref()
    }

    /// Returns this failure's stable owner-facing code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the operation-specific stable code is clearest as the accessor's final expression"
    )]
    pub const fn code(&self) -> &'static str {
        match self.operation {
            GitOperation::AppendTiberEvents => "tiber_store_append_tiber_events_failed",
            GitOperation::MaterializeTiberSnapshot => "tiber_store_snapshot_materialization_failed",
            GitOperation::PublishTiberEvents => "tiber_store_publish_tiber_events_failed",
            GitOperation::RefreshOriginTiberRef => "tiber_git_refresh_origin_tiber_ref_failed",
            GitOperation::ResolveTiberRef => "tiber_git_resolve_tiber_ref_failed",
            GitOperation::SignTiberCandidate => "tiber_git_sign_tiber_candidate_failed",
            GitOperation::VerifyTiberSignature => "tiber_git_verify_tiber_signature_failed",
        }
    }
}

impl fmt::Display for GitCommandFailure {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the process failure intentionally displays only its stable sanitized code"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the only meaningful causal source is the retained operating-system process error"
)]
impl Error for GitCommandFailure {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the source mapping is clearest as one closed borrowed match"
    )]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.io_source.as_ref() {
            Some(source) => Some(source),
            None => None,
        }
    }
}

/// Stable sanitized errors from Tiber's read-only Git history boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitStoreError {
    /// The event catalog could not establish a valid transaction-to-writer map.
    #[error("tiber_store_event_catalog_invalid")]
    EventCatalog,
    /// A signed authority snapshot omitted `eventstore/events`.
    #[error("tiber_store_snapshot_events_missing")]
    EventDirectoryMissing,
    /// `EventCore` could not open or inspect the materialized history.
    #[error("tiber_store_event_history_invalid")]
    EventHistory,
    /// `EventCore` found a transaction with an absent declared parent.
    #[error("tiber_store_event_history_dangling_transaction")]
    EventHistoryDanglingTransaction,
    /// Actual writers left an unreconciled `EventCore` consistency conflict.
    #[error("tiber_store_event_history_fork_detected")]
    EventHistoryForkDetected,
    /// `EventCore` found altered or incomplete transaction data.
    #[error("tiber_store_event_history_integrity_failed")]
    EventHistoryIntegrityFailed,
    /// One bounded Git process failed with itemized sanitized context.
    #[error(transparent)]
    GitCommand(#[from] GitCommandFailure),
    /// A disposable authority snapshot could not be created or materialized.
    #[error("tiber_store_snapshot_materialization_failed")]
    Materialization,
    /// `origin/tiber` could not be resolved or fetched exactly.
    #[error("tiber_git_refresh_origin_tiber_ref_failed")]
    RefreshOriginTiberRef,
    /// The local authority ref or `origin` configuration could not be resolved.
    #[error("tiber_git_resolve_tiber_ref_failed")]
    ResolveTiberRef,
    /// The resolved authority revision failed ordinary Git signature verification.
    #[error("tiber_git_verify_tiber_signature_failed")]
    VerifyTiberSignature,
}

impl GitStoreError {
    /// Returns this failure's stable owner-facing code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed error-code mapping is intentionally a concise borrowed match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EventCatalog => "tiber_store_event_catalog_invalid",
            Self::EventDirectoryMissing => "tiber_store_snapshot_events_missing",
            Self::EventHistory => "tiber_store_event_history_invalid",
            Self::EventHistoryDanglingTransaction => {
                "tiber_store_event_history_dangling_transaction"
            }
            Self::EventHistoryForkDetected => "tiber_store_event_history_fork_detected",
            Self::EventHistoryIntegrityFailed => "tiber_store_event_history_integrity_failed",
            Self::Materialization => "tiber_store_snapshot_materialization_failed",
            Self::GitCommand(failure) => failure.code(),
            Self::RefreshOriginTiberRef => "tiber_git_refresh_origin_tiber_ref_failed",
            Self::ResolveTiberRef => "tiber_git_resolve_tiber_ref_failed",
            Self::VerifyTiberSignature => "tiber_git_verify_tiber_signature_failed",
        }
    }

    /// Returns itemized Git process context when the process boundary failed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "only the process-failure variant owns itemized Git context"
    )]
    pub const fn git_command_failure(&self) -> Option<&GitCommandFailure> {
        match self {
            Self::GitCommand(failure) => Some(failure),
            Self::EventCatalog
            | Self::EventDirectoryMissing
            | Self::EventHistory
            | Self::EventHistoryDanglingTransaction
            | Self::EventHistoryForkDetected
            | Self::EventHistoryIntegrityFailed
            | Self::Materialization
            | Self::RefreshOriginTiberRef
            | Self::ResolveTiberRef
            | Self::VerifyTiberSignature => None,
        }
    }

    /// Returns whether retrying this failure with unchanged inputs is meaningful.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed retryability mapping is intentionally a concise borrowed match"
    )]
    pub const fn retryability(&self) -> Retryability {
        match self {
            Self::GitCommand(failure) => failure.retryability(),
            Self::Materialization => Retryability::Retryable,
            Self::EventCatalog
            | Self::EventDirectoryMissing
            | Self::EventHistory
            | Self::EventHistoryDanglingTransaction
            | Self::EventHistoryForkDetected
            | Self::EventHistoryIntegrityFailed
            | Self::RefreshOriginTiberRef
            | Self::ResolveTiberRef
            | Self::VerifyTiberSignature => Retryability::Permanent,
        }
    }
}

/// A read-only, disposable materialization of one authority snapshot.
pub struct TiberEventStore {
    /// Owns the temporary snapshot for at least as long as the reader exists.
    _temporary_directory: TempDir,
    /// Immutable transaction graph and envelopes needed for typed verification
    /// and transaction-order query replay.
    event_history: EventHistoryCatalog,
    /// The exact revision materialized for this reader.
    revision: TiberRevision,
    /// The `eventcore-fs` reader over the disposable materialization.
    store: FileEventStore,
    /// Stream identifiers observed in the committed event envelopes.
    stream_ids: Vec<StreamId>,
}

/// A typed reader whose fixed filter has been validated against its immutable snapshot.
///
/// Construction verifies every selected envelope once. Subsequent page reads
/// reuse the same filter without revalidating earlier history.
pub struct VerifiedEventReader<'store, E> {
    /// Binds the validated application event type to this reader.
    _event_type: PhantomData<fn() -> E>,
    /// The exact filter whose selected envelopes were validated.
    filter: EventFilter,
    /// The underlying read-only `EventCore` snapshot reader.
    store: &'store FileEventStore,
}

/// A bounded page over facts replayed in immutable transaction and envelope order.
///
/// Unlike [`EventPage`], this cursor is owned by Tiber's snapshot catalog. It
/// never represents an `EventCore` replica-local projection position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionEventPage {
    /// The maximum facts returned by one bounded read.
    limit: BatchSize,
    /// The exclusive count of already-returned immutable envelopes.
    offset: usize,
}

impl TransactionEventPage {
    /// Starts a bounded transaction-order replay at the first retained fact.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the semantic page constructor is clearest as its final value"
    )]
    pub const fn first(limit: BatchSize) -> Self {
        Self { offset: 0, limit }
    }

    /// Advances after one non-empty page while preserving the fixed page limit.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the bounded offset advance is clearest as one checked expression"
    )]
    pub fn next_from_results<E>(&self, events: &[E]) -> Option<Self> {
        (!events.is_empty())
            .then(|| self.offset.checked_add(events.len()))
            .flatten()
            .map(|offset| Self {
                offset,
                limit: self.limit,
            })
    }
}

/// A typed, validated reader over one causally unambiguous transaction history.
///
/// This reader deliberately differs from [`VerifiedEventReader`]: it is for
/// bounded reconstruction of a selected multi-stream history, where
/// replica-local `EventCore` projection cursors cannot define durable order.
pub struct VerifiedTransactionReader<'store, E> {
    /// Binds the validated application event type to this reader.
    _event_type: PhantomData<fn() -> E>,
    /// Selected envelopes in immutable transaction and file-envelope order.
    event_envelopes: Vec<&'store EventCatalogEntry>,
}

/// Stable failures from building or paging a transaction-ordered reader.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransactionHistoryError {
    /// The selected transaction subgraph has no unique causal replay sequence.
    #[error("tiber_store_event_history_ambiguous_transaction_order")]
    AmbiguousTransactionOrder,
    /// A selected persisted payload cannot be decoded as the requested fact.
    #[error(transparent)]
    EventStore(#[from] EventStoreError),
}

impl TransactionHistoryError {
    /// Returns this failure's stable machine-readable code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed borrowed error-code mapping is clearest as a concise final match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AmbiguousTransactionOrder => {
                "tiber_store_event_history_ambiguous_transaction_order"
            }
            Self::EventStore(_) => "tiber_store_event_history_payload_invalid",
        }
    }
}

/// Wire fields needed to associate `EventCore` fork candidates with actual writers.
#[derive(Deserialize)]
struct CatalogRecord {
    /// The serialized event payload, present on event envelopes.
    #[serde(default)]
    event_data: serde_json::Value,
    /// The stable application event type, present on event envelopes.
    #[serde(default)]
    event_type: String,
    /// The declared immutable parent transaction identities.
    #[serde(default)]
    parent_transaction_ids: Option<Vec<String>>,
    /// The JSONL record discriminator.
    record: String,
    /// Consistency input streams recorded on a transaction header.
    #[serde(default)]
    stream_bases: Option<BTreeMap<String, usize>>,
    /// The actual stream written by an event envelope.
    #[serde(default)]
    stream_id: Option<String>,
    /// The header transaction identity.
    #[serde(default)]
    transaction_id: Option<String>,
}

/// The minimal history catalog needed for compatibility-safe fork interpretation.
struct EventHistoryCatalog {
    /// Immutable transactions keyed by their file/header identity.
    transactions: BTreeMap<String, TransactionCatalogEntry>,
}

/// Parsed immutable transaction metadata needed only for fork classification.
struct TransactionCatalogEntry {
    /// Durable envelopes in their original order within this one transaction file.
    event_envelopes: Vec<EventCatalogEntry>,
    /// Parent links in the immutable `EventCore` transaction graph.
    parent_ids: Vec<String>,
    /// Stream IDs actually written by this transaction's event envelopes.
    written_stream_ids: BTreeSet<String>,
}

/// A committed envelope whose payload is available for typed verification.
struct EventCatalogEntry {
    /// The payload validated against an application event type on demand.
    event_data: serde_json::Value,
    /// The stable application event type carried by the envelope.
    event_type: String,
    /// The immutable stream identity carried by the envelope.
    stream_id: StreamId,
}

/// One exact authority revision and the repository containing its disposable snapshot objects.
struct ResolvedAuthority {
    /// The caller working directory required by a relative local SSH command.
    ///
    /// Remote authority operations keep object and ref state in the disposable
    /// repository while retaining this caller-root working directory for the
    /// owner's repository-local transport configuration.
    caller_execution_root: Option<PathBuf>,
    /// The repository used only to verify and materialize this exact revision.
    repository: PathBuf,
    /// The exact signed revision selected at the local or remote authority boundary.
    revision: TiberRevision,
}

impl fmt::Debug for TiberEventStore {
    #[expect(
        clippy::implicit_return,
        reason = "a non-exhaustive debug view intentionally reports only durable snapshot identity"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TiberEventStore")
            .field("revision", &self.revision)
            .field("stream_ids", &self.stream_ids)
            .finish_non_exhaustive()
    }
}

impl<E> fmt::Debug for VerifiedEventReader<'_, E> {
    #[expect(
        clippy::implicit_return,
        reason = "the non-exhaustive debug view exposes the fixed filter but not private storage internals"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedEventReader")
            .field("filter", &self.filter)
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the read-only store API follows authority opening, snapshot inspection, generic projection reads, and specialized transaction reconstruction rather than alphabetic item names"
)]
impl TiberEventStore {
    /// Finds the selected transaction ancestry and accepts it only as one chain.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the selected transaction graph remains a small typed read boundary with explicit fail-closed propagation"
    )]
    fn selected_transaction_order<E: Event>(
        &self,
        stream_patterns: &[StreamPattern],
    ) -> Result<Vec<String>, TransactionHistoryError> {
        let selected = self
            .event_history
            .transactions
            .iter()
            .filter(|entry| {
                entry.1.event_envelopes.iter().any(|envelope| {
                    envelope.event_type == E::event_type_name()
                        && matches_stream_patterns(envelope, stream_patterns)
                })
            })
            .map(|(transaction_id, _transaction)| transaction_id.clone())
            .collect::<BTreeSet<_>>();
        let relevant = transaction_ancestry(&selected, &self.event_history.transactions)?;
        linear_transaction_order(&relevant, &self.event_history.transactions)
    }

    /// Opens a fixed read-only snapshot of the sole Tiber authority.
    ///
    /// A repository with `origin` uses only the exact revision advertised at
    /// `origin/tiber`; it does not fall back to `refs/heads/tiber`. A repository
    /// without `origin` uses only that local ref. The selected revision is
    /// signature-verified before its committed tree is materialized.
    ///
    /// # Errors
    ///
    /// Returns a stable [`GitStoreError`] if authority resolution, signature
    /// verification, materialization, or `EventCore` validation fails.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "this imperative boundary preserves each typed failure while sequencing fixed source-resolution effects"
    )]
    #[inline]
    pub fn open(repository: &Path) -> Result<Self, GitStoreError> {
        let temporary_directory =
            TempDir::new().map_err(|_source| GitStoreError::Materialization)?;
        let authority = resolve_authority_revision(repository, temporary_directory.path())?;
        let snapshot = temporary_directory.path().join("snapshot");
        fs::create_dir_all(&snapshot).map_err(|_source| GitStoreError::Materialization)?;

        verify_signed_revision(&authority.repository, &authority.revision)?;
        materialize_revision(&authority.repository, &authority.revision, &snapshot)?;
        require_safe_event_store_layout(&snapshot)?;

        let event_store_root = snapshot.join(EVENT_STORE_DIRECTORY);
        let store = FileEventStore::open_with_config(
            FsConfig::new(&event_store_root).with_fsync(FsyncPolicy::None),
        )
        .map_err(|_source| GitStoreError::EventHistory)?;
        let catalog = inspect_event_history(&event_store_root.join(EVENTS_DIRECTORY))?;
        verify_history_integrity(&store, &catalog)?;

        Ok(Self {
            revision: authority.revision,
            store,
            stream_ids: stream_ids_from_catalog(&catalog)?,
            event_history: catalog,
            _temporary_directory: temporary_directory,
        })
    }

    /// Returns the exact authority revision materialized for this reader.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "a tiny borrowed accessor reads most clearly as its final expression"
    )]
    pub fn revision(&self) -> &TiberRevision {
        &self.revision
    }

    /// Returns stream identities discovered from committed event envelopes.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "a tiny borrowed accessor reads most clearly as its final expression"
    )]
    pub fn stream_ids(&self) -> &[StreamId] {
        &self.stream_ids
    }

    /// Validates a fixed filter and event type for paged reads from this snapshot.
    ///
    /// `EventCore` deliberately filters failed payload decodes from paged
    /// cross-stream reads. This boundary validates every selected envelope
    /// before returning a reader that can page only with the same filter and
    /// application event type.
    ///
    /// # Errors
    ///
    /// Returns [`EventStoreError::DeserializationFailed`] for the first selected
    /// payload that cannot decode as `E` or whose payload stream identity differs
    /// from its persisted envelope.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the typed reader is constructed only after its fixed filter and event schema pass complete validation"
    )]
    pub fn verified_reader<E: Event>(
        &self,
        filter: EventFilter,
    ) -> Result<VerifiedEventReader<'_, E>, EventStoreError> {
        self.verify_decodes::<E>(&filter)?;
        Ok(VerifiedEventReader {
            _event_type: PhantomData,
            filter,
            store: &self.store,
        })
    }

    /// Validates and returns a bounded reader in immutable transaction order.
    ///
    /// `EventCore`'s cross-stream projection cursor is a replica-local
    /// delivery sequence. A task-board history needs one durable order across
    /// a closed union of streams instead, so this method follows the selected
    /// transactions' parent chain and preserves each transaction file's
    /// envelope order. Any selected graph without exactly one causal sequence
    /// fails closed rather than deriving order from filenames or cursor values.
    ///
    /// # Errors
    ///
    /// Returns [`TransactionHistoryError::AmbiguousTransactionOrder`] when the
    /// selected transaction ancestry has no single replay sequence, or wraps
    /// [`EventStoreError::DeserializationFailed`] for a malformed selected
    /// payload or mismatched payload stream identity.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the read-only composite boundary preserves its precise typed failure while constructing one validated immutable sequence"
    )]
    #[inline]
    pub fn verified_transaction_reader<E: Event>(
        &self,
        stream_patterns: &[StreamPattern],
    ) -> Result<VerifiedTransactionReader<'_, E>, TransactionHistoryError> {
        let transaction_ids = self.selected_transaction_order::<E>(stream_patterns)?;
        let mut event_envelopes = Vec::new();
        for transaction_id in transaction_ids {
            let transaction = self
                .event_history
                .transactions
                .get(&transaction_id)
                .ok_or(TransactionHistoryError::AmbiguousTransactionOrder)?;
            for envelope in &transaction.event_envelopes {
                if !matches_stream_patterns(envelope, stream_patterns)
                    || envelope.event_type != E::event_type_name()
                {
                    continue;
                }
                verify_envelope_decodes::<E>(envelope)?;
                event_envelopes.push(envelope);
            }
        }
        Ok(VerifiedTransactionReader {
            _event_type: PhantomData,
            event_envelopes,
        })
    }

    /// Verifies that every envelope selected by a projection filter decodes as an event type.
    ///
    /// `EventCore` deliberately filters failed payload decodes from paged
    /// cross-stream reads. Applications must use the same `filter` for this
    /// check and their projection pages so a signed but schema-incompatible
    /// fact cannot be omitted. Facts outside the filter's stream and effective
    /// event-type selection are deliberately ignored.
    ///
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the closed typed-validation result reads most clearly as the final expression"
    )]
    fn verify_decodes<E: Event>(&self, filter: &EventFilter) -> Result<(), EventStoreError> {
        let selected_type = filter
            .event_type()
            .map_or_else(|| E::event_type_name().to_owned(), str::to_owned);
        for envelope in self
            .event_history
            .transactions
            .values()
            .flat_map(|transaction| &transaction.event_envelopes)
        {
            let stream_id: &str = envelope.stream_id.as_ref();
            let matches_prefix = filter
                .stream_prefix()
                .is_none_or(|prefix| stream_id.starts_with(prefix.as_ref()));
            let matches_pattern = filter
                .stream_pattern()
                .is_none_or(|pattern| pattern.matches(stream_id));
            if envelope.event_type != selected_type || !matches_prefix || !matches_pattern {
                continue;
            }
            verify_envelope_decodes::<E>(envelope)?;
        }
        Ok(())
    }
}

impl<E: Event> VerifiedEventReader<'_, E> {
    /// Reads one typed page using the filter validated when this reader was constructed.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`EventStoreError`] if `EventCore` cannot read the page.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the validated reader forwards one page through its fixed private filter"
    )]
    pub async fn read_page(
        &self,
        page: EventPage,
    ) -> Result<Vec<(E, StreamPosition)>, EventStoreError> {
        self.store.read_events(self.filter.clone(), page).await
    }
}

impl<E> fmt::Debug for VerifiedTransactionReader<'_, E> {
    #[expect(
        clippy::implicit_return,
        reason = "the non-exhaustive debug view reports only bounded selected-fact count"
    )]
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedTransactionReader")
            .field("selected_event_count", &self.event_envelopes.len())
            .finish_non_exhaustive()
    }
}

impl<E: Event> VerifiedTransactionReader<'_, E> {
    /// Reads one bounded page from the validated immutable transaction sequence.
    ///
    /// # Errors
    ///
    /// Returns [`EventStoreError::DeserializationFailed`] if an already
    /// validated immutable payload can no longer decode, which indicates an
    /// invariant violation at this read boundary.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the bounded immutable page decodes only its selected envelope slice and retains typed propagation"
    )]
    #[inline]
    pub fn read_page(&self, page: TransactionEventPage) -> Result<Vec<E>, EventStoreError> {
        let start = page.offset.min(self.event_envelopes.len());
        let limit: usize = page.limit.into();
        let end = start.saturating_add(limit).min(self.event_envelopes.len());
        let envelopes =
            self.event_envelopes
                .get(start..end)
                .ok_or(EventStoreError::StoreFailure {
                    operation: Operation::ReadStream,
                })?;
        envelopes
            .iter()
            .map(|envelope| decode_envelope::<E>(envelope))
            .collect()
    }
}

/// Classifies a host process I/O error without depending on its diagnostic text.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    clippy::wildcard_enum_match_arm,
    reason = "known local setup faults are permanent while current and future operational I/O failures remain retryable"
)]
const fn retryability_for_io(kind: io::ErrorKind) -> Retryability {
    match kind {
        io::ErrorKind::NotFound
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::InvalidInput
        | io::ErrorKind::InvalidData
        | io::ErrorKind::Unsupported => Retryability::Permanent,
        _ => Retryability::Retryable,
    }
}

/// Resolves the only authority revision accepted by this adapter.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the fixed origin/local split retains distinct stable authority failures"
)]
fn resolve_authority_revision(
    repository: &Path,
    temporary_directory: &Path,
) -> Result<ResolvedAuthority, GitStoreError> {
    let origin = run_git(
        repository,
        GitOperation::ResolveTiberRef,
        &["remote", "get-url", "origin"],
        None,
        None,
    )?;
    if origin.status.success() {
        let origin_url = parse_origin_url(&origin.stdout)?;
        let disposable_origin_url = origin_url_for_disposable_authority(repository, &origin_url)?;
        return resolve_remote_authority_revision(
            repository,
            temporary_directory,
            &disposable_origin_url,
        );
    }
    if origin.status.code() == Some(GIT_NOT_FOUND_EXIT) {
        let revision = resolve_local_authority_revision(repository)?;
        return Ok(ResolvedAuthority {
            repository: repository.to_path_buf(),
            revision,
            caller_execution_root: None,
        });
    }
    Err(git_nonzero_error(GitOperation::ResolveTiberRef, &origin))
}

/// Resolves the local authority only if no `origin` remote is configured.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the fixed local ref is deliberately the only local authority fallback"
)]
fn resolve_local_authority_revision(repository: &Path) -> Result<TiberRevision, GitStoreError> {
    let output = run_git(
        repository,
        GitOperation::ResolveTiberRef,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/tiber^{commit}",
        ],
        None,
        None,
    )?;
    if output.status.success() {
        return parse_revision_output(&output.stdout, GitOperation::ResolveTiberRef);
    }
    Err(git_nonzero_error(GitOperation::ResolveTiberRef, &output))
}

/// Resolves and fetches exactly the revision advertised for `origin/tiber` into disposable storage.
///
/// The parsed object name flows directly from the fixed `ls-remote` response to
/// `fetch`; `--no-write-fetch-head` and an empty refmap preserve all Git refs in
/// the caller. The bare authority repository and every object it receives live
/// below `temporary_directory` and are removed with the snapshot.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the fixed remote read sequence retains a clear exact-OID boundary"
)]
fn resolve_remote_authority_revision(
    repository: &Path,
    temporary_directory: &Path,
    origin_url: &str,
) -> Result<ResolvedAuthority, GitStoreError> {
    let advertised = run_git(
        repository,
        GitOperation::RefreshOriginTiberRef,
        &["ls-remote", "--exit-code", origin_url, ORIGIN_TIBER_REF],
        None,
        None,
    )?;
    if !advertised.status.success() {
        return Err(git_nonzero_error(
            GitOperation::RefreshOriginTiberRef,
            &advertised,
        ));
    }
    let revision = parse_ls_remote_revision(&advertised.stdout)?;
    let object_format = caller_storage_object_format(repository)?;
    let caller_execution_root = caller_repository_origin_base(repository)?;
    let authority_repository = temporary_directory.join("authority.git");
    let initialized = run_git(
        temporary_directory,
        GitOperation::RefreshOriginTiberRef,
        &[
            "init",
            "--bare",
            object_format.init_argument(),
            "authority.git",
        ],
        None,
        None,
    )?;
    require_git_success(&initialized, GitOperation::RefreshOriginTiberRef)?;
    copy_local_gpg_configuration(repository, &authority_repository)?;
    copy_local_transport_configuration(repository, &authority_repository)?;
    let remote_added = run_git(
        &authority_repository,
        GitOperation::RefreshOriginTiberRef,
        &["remote", "add", "origin", origin_url],
        None,
        None,
    )?;
    require_git_success(&remote_added, GitOperation::RefreshOriginTiberRef)?;
    let authority_git_dir = authority_repository
        .to_str()
        .map(|path| format!("--git-dir={path}"))
        .ok_or(GitStoreError::RefreshOriginTiberRef)?;
    let fetched = run_git(
        &caller_execution_root,
        GitOperation::RefreshOriginTiberRef,
        &[
            authority_git_dir.as_str(),
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--refmap=",
            "origin",
            revision.as_str(),
        ],
        None,
        None,
    )?;
    require_git_success(&fetched, GitOperation::RefreshOriginTiberRef)?;
    Ok(ResolvedAuthority {
        repository: authority_repository,
        revision,
        caller_execution_root: Some(caller_execution_root),
    })
}

/// Preserves the caller repository's bounded SSH transport policy in disposable storage.
///
/// System and global configuration remain inherited normally. Only the two
/// repository-local settings that select and classify Git's SSH command cross
/// this boundary; remotes, URL rewrites, refs, and objects remain isolated.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the closed two-key transfer preserves ordinary repository-local SSH transport semantics without widening disposable authority configuration"
)]
fn copy_local_transport_configuration(
    repository: &Path,
    authority_repository: &Path,
) -> Result<(), GitStoreError> {
    for key in ["core.sshCommand", "ssh.variant"] {
        let configuration = run_git(
            repository,
            GitOperation::RefreshOriginTiberRef,
            &["config", "--null", "--local", "--get-all", key],
            None,
            None,
        )?;
        if configuration.status.code() == Some(GIT_CONFIG_NOT_FOUND_EXIT) {
            continue;
        }
        require_git_success(&configuration, GitOperation::RefreshOriginTiberRef)?;
        for raw_value in configuration
            .stdout
            .split(|byte| *byte == 0)
            .filter(|value| !value.is_empty())
        {
            let value = str::from_utf8(raw_value)
                .map_err(|_source| GitStoreError::RefreshOriginTiberRef)?;
            let copied = run_git(
                authority_repository,
                GitOperation::RefreshOriginTiberRef,
                &["config", "--local", "--add", key, value],
                None,
                None,
            )?;
            require_git_success(&copied, GitOperation::RefreshOriginTiberRef)?;
        }
    }
    Ok(())
}

/// Gives a relative local origin the same base before crossing into a temporary repository.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "only Git's unambiguous relative filesystem URL forms need rebasing before the repository directory changes"
)]
fn origin_url_for_disposable_authority(
    repository: &Path,
    origin_url: &str,
) -> Result<String, GitStoreError> {
    if !is_relative_filesystem_origin(origin_url) {
        return Ok(origin_url.to_owned());
    }
    caller_repository_origin_base(repository)?
        .join(origin_url)
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or(GitStoreError::RefreshOriginTiberRef)
}

/// Resolves Git's repository-relative URL base for worktree and bare callers.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the fixed two-command fallback delegates repository discovery to Git without copying caller state"
)]
fn caller_repository_origin_base(repository: &Path) -> Result<PathBuf, GitStoreError> {
    let worktree = run_git(
        repository,
        GitOperation::RefreshOriginTiberRef,
        &["rev-parse", "--show-toplevel"],
        None,
        None,
    )?;
    if worktree.status.success() {
        return parse_absolute_git_path(&worktree.stdout);
    }
    let git_directory = run_git(
        repository,
        GitOperation::RefreshOriginTiberRef,
        &["rev-parse", "--absolute-git-dir"],
        None,
        None,
    )?;
    require_git_success(&git_directory, GitOperation::RefreshOriginTiberRef)?;
    parse_absolute_git_path(&git_directory.stdout)
}

/// Parses one absolute path emitted by fixed `git rev-parse` argv.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the closed path parser rejects malformed Git output before it reaches URL construction"
)]
fn parse_absolute_git_path(output: &[u8]) -> Result<PathBuf, GitStoreError> {
    let text = str::from_utf8(output).map_err(|_source| GitStoreError::RefreshOriginTiberRef)?;
    let Some(path) = text.strip_suffix('\n') else {
        return Err(GitStoreError::RefreshOriginTiberRef);
    };
    let parsed = PathBuf::from(path);
    if path.is_empty() || path.contains('\n') || !parsed.is_absolute() {
        return Err(GitStoreError::RefreshOriginTiberRef);
    }
    Ok(parsed)
}

/// Distinguishes local relative paths from schemes and scp-like SSH URLs.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "Git's explicit scheme and colon-before-slash SSH forms are the only ambiguous relative-looking URL spellings"
)]
fn is_relative_filesystem_origin(origin_url: &str) -> bool {
    if !Path::new(origin_url).is_relative() || origin_url.contains("://") {
        return false;
    }
    let first_colon = origin_url.find(':');
    let first_slash = origin_url.find('/');
    first_colon.is_none_or(|colon| first_slash.is_some_and(|slash| colon >= slash))
}

/// Reads the caller's exact object storage format for disposable authority setup.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the caller format is parsed once at the remote authority boundary before the disposable repository exists"
)]
fn caller_storage_object_format(repository: &Path) -> Result<GitObjectFormat, GitStoreError> {
    let output = run_git(
        repository,
        GitOperation::RefreshOriginTiberRef,
        &["rev-parse", "--show-object-format=storage"],
        None,
        None,
    )?;
    require_git_success(&output, GitOperation::RefreshOriginTiberRef)?;
    GitObjectFormat::parse_storage_output(&output.stdout)
        .map_err(|_source| GitStoreError::RefreshOriginTiberRef)
}

/// Copies caller-local Git verification settings into the disposable authority repository.
///
/// Global and system configuration remain available to both repositories. This preserves the
/// caller's ordinary repository-local signature policy without copying remotes, refs, objects,
/// or unrelated configuration into the temporary Git database.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the bounded signature-policy transfer keeps local verification semantics while remote objects remain disposable"
)]
fn copy_local_gpg_configuration(
    repository: &Path,
    authority_repository: &Path,
) -> Result<(), GitStoreError> {
    let configuration = run_git(
        repository,
        GitOperation::VerifyTiberSignature,
        &["config", "--null", "--local", "--get-regexp", "^gpg\\."],
        None,
        None,
    )?;
    if configuration.status.code() == Some(GIT_CONFIG_NOT_FOUND_EXIT) {
        return Ok(());
    }
    require_git_success(&configuration, GitOperation::VerifyTiberSignature)?;
    let mut caller_worktree_root = None;
    for raw_entry in configuration.stdout.split(|byte| *byte == 0) {
        if raw_entry.is_empty() {
            continue;
        }
        let entry =
            str::from_utf8(raw_entry).map_err(|_source| GitStoreError::VerifyTiberSignature)?;
        let Some((key, value)) = entry.split_once('\n') else {
            return Err(GitStoreError::VerifyTiberSignature);
        };
        if key.is_empty() {
            return Err(GitStoreError::VerifyTiberSignature);
        }
        let authority_value = gpg_configuration_value_for_authority(
            repository,
            key,
            value,
            &mut caller_worktree_root,
        )?;
        let copied = run_git(
            authority_repository,
            GitOperation::VerifyTiberSignature,
            &["config", "--local", "--add", key, authority_value.as_str()],
            None,
            None,
        )?;
        require_git_success(&copied, GitOperation::VerifyTiberSignature)?;
    }
    Ok(())
}

/// Preserves one caller-local GPG pathname value when the authority snapshot has another Git dir.
///
/// Git documents `gpg.program`, `gpg.<format>.program`,
/// `gpg.ssh.allowedSignersFile`, and `gpg.ssh.revocationFile` as pathname
/// configuration. SSH signer and revocation files resolve a relative pathname
/// from the caller worktree top level. Program names without a path component
/// remain PATH lookups, while explicit relative program paths retain the same
/// caller-worktree interpretation.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the narrow configuration transfer preserves only Git-documented GPG pathname semantics across disposable Git directories"
)]
fn gpg_configuration_value_for_authority(
    repository: &Path,
    key: &str,
    value: &str,
    caller_worktree_root: &mut Option<PathBuf>,
) -> Result<String, GitStoreError> {
    let Some(path_value) = gpg_path_value_requiring_rebase(key, value) else {
        return Ok(value.to_owned());
    };
    if caller_worktree_root.is_none() {
        let resolved = caller_worktree_top_level(repository, GitOperation::VerifyTiberSignature)?;
        *caller_worktree_root = Some(resolved);
    }
    let worktree_root = caller_worktree_root
        .as_deref()
        .ok_or(GitStoreError::VerifyTiberSignature)?;
    let rebased_path = worktree_root.join(path_value);
    let rebased = rebased_path
        .to_str()
        .ok_or(GitStoreError::VerifyTiberSignature)?;
    let optional_prefix = value
        .strip_prefix(OPTIONAL_GIT_PATH_PREFIX)
        .map_or("", |_| OPTIONAL_GIT_PATH_PREFIX);
    Ok(format!("{optional_prefix}{rebased}"))
}

/// Returns the relative pathname portion that needs caller-worktree rebasing.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the closed config-key selection is clearest as a short pure predicate"
)]
fn gpg_path_value_requiring_rebase<'path_value>(
    key: &str,
    value: &'path_value str,
) -> Option<&'path_value Path> {
    let path_value = value
        .strip_prefix(OPTIONAL_GIT_PATH_PREFIX)
        .unwrap_or(value);
    let path = Path::new(path_value);
    if path_value.is_empty() || path.is_absolute() || git_path_is_self_resolving(path_value) {
        return None;
    }
    if is_gpg_ssh_file_key(key) {
        return Some(path);
    }
    if is_gpg_program_key(key) && is_explicit_relative_program_path(path) {
        return Some(path);
    }
    None
}

/// Identifies Git's documented SSH verification-file pathname configuration keys.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the exact closed key list should remain visible beside the transfer boundary"
)]
fn is_gpg_ssh_file_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("gpg.ssh.allowedSignersFile")
        || key.eq_ignore_ascii_case("gpg.ssh.revocationFile")
}

/// Identifies Git's documented GPG program pathname configuration keys.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "Git currently exposes these three signature formats through gpg.<format>.program"
)]
fn is_gpg_program_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("gpg.program")
        || key.eq_ignore_ascii_case("gpg.openpgp.program")
        || key.eq_ignore_ascii_case("gpg.ssh.program")
        || key.eq_ignore_ascii_case("gpg.x509.program")
}

/// Leaves Git's tilde and installation-prefix path interpolation to Git itself.
#[expect(
    clippy::implicit_return,
    reason = "these documented path forms are already independent of a Git worktree directory"
)]
fn git_path_is_self_resolving(value: &str) -> bool {
    value == "~"
        || (value.starts_with('~') && value.contains('/'))
        || value.starts_with("%(prefix)/")
}

/// Distinguishes a caller-relative executable pathname from a bare PATH command name.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "bare program names intentionally retain Git's ordinary PATH lookup semantics"
)]
fn is_explicit_relative_program_path(path: &Path) -> bool {
    path.components().count() > 1
}

/// Resolves the caller worktree root required by Git's relative SSH signer-file semantics.
#[expect(
    clippy::implicit_return,
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    reason = "the worktree lookup is a separate fixed Git boundary shared with publication, while rustfmt canonicalizes the crate visibility spelling"
)]
pub(crate) fn caller_worktree_top_level(
    repository: &Path,
    operation: GitOperation,
) -> Result<PathBuf, GitStoreError> {
    let output = run_git(
        repository,
        operation,
        &["rev-parse", "--show-toplevel"],
        None,
        None,
    )?;
    require_git_success(&output, operation)?;
    let text = str::from_utf8(&output.stdout).map_err(|_source| git_semantic_error(operation))?;
    let Some(path) = text.strip_suffix('\n') else {
        return Err(git_semantic_error(operation));
    };
    let worktree = PathBuf::from(path);
    if path.is_empty() || path.contains('\n') || !worktree.is_absolute() {
        return Err(git_semantic_error(operation));
    }
    Ok(worktree)
}

/// Parses the one exact remote URL emitted by fixed `git remote get-url origin` argv.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the parser accepts one opaque Git URL without retaining malformed command output"
)]
fn parse_origin_url(output: &[u8]) -> Result<String, GitStoreError> {
    let text = str::from_utf8(output).map_err(|_source| GitStoreError::ResolveTiberRef)?;
    let Some(url) = text.strip_suffix('\n') else {
        return Err(GitStoreError::ResolveTiberRef);
    };
    if url.is_empty() || url.contains('\n') {
        return Err(GitStoreError::ResolveTiberRef);
    }
    Ok(url.to_owned())
}

/// Parses the exact one-row authority response from fixed `git ls-remote` argv.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the parser maps all malformed remote output into the one stable refresh failure"
)]
fn parse_ls_remote_revision(output: &[u8]) -> Result<TiberRevision, GitStoreError> {
    let text = str::from_utf8(output).map_err(|_source| GitStoreError::RefreshOriginTiberRef)?;
    let mut lines = text.lines();
    let Some(line) = lines.next() else {
        return Err(GitStoreError::RefreshOriginTiberRef);
    };
    if lines.next().is_some() {
        return Err(GitStoreError::RefreshOriginTiberRef);
    }
    let Some((object_name, reference)) = line.split_once('\t') else {
        return Err(GitStoreError::RefreshOriginTiberRef);
    };
    if reference != ORIGIN_TIBER_REF {
        return Err(GitStoreError::RefreshOriginTiberRef);
    }
    TiberRevision::parse(object_name).map_err(|_source| GitStoreError::RefreshOriginTiberRef)
}

/// Parses the one exact revision emitted by fixed `git rev-parse` argv.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the parser maps all malformed local output into the one stable resolution failure"
)]
fn parse_revision_output(
    output: &[u8],
    operation: GitOperation,
) -> Result<TiberRevision, GitStoreError> {
    let text = str::from_utf8(output).map_err(|_source| git_semantic_error(operation))?;
    let Some(object_name) = text.strip_suffix('\n') else {
        return Err(git_semantic_error(operation));
    };
    if object_name.contains('\n') {
        return Err(git_semantic_error(operation));
    }
    TiberRevision::parse(object_name).map_err(|_source| git_semantic_error(operation))
}

/// Maps malformed fixed-command output to its permanent semantic operation error.
#[expect(
    clippy::implicit_return,
    reason = "the closed semantic mapping intentionally excludes untrusted process diagnostics"
)]
const fn git_semantic_error(operation: GitOperation) -> GitStoreError {
    match operation {
        GitOperation::AppendTiberEvents
        | GitOperation::MaterializeTiberSnapshot
        | GitOperation::PublishTiberEvents
        | GitOperation::SignTiberCandidate => GitStoreError::Materialization,
        GitOperation::RefreshOriginTiberRef => GitStoreError::RefreshOriginTiberRef,
        GitOperation::ResolveTiberRef => GitStoreError::ResolveTiberRef,
        GitOperation::VerifyTiberSignature => GitStoreError::VerifyTiberSignature,
    }
}

/// Verifies one selected revision using the owner's ordinary Git trust setup.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "signature verification is intentionally an explicit boundary before materialization"
)]
fn verify_signed_revision(
    repository: &Path,
    revision: &TiberRevision,
) -> Result<(), GitStoreError> {
    let output = run_git(
        repository,
        GitOperation::VerifyTiberSignature,
        &["verify-commit", revision.as_str()],
        None,
        None,
    )?;
    require_git_success(&output, GitOperation::VerifyTiberSignature)?;
    Ok(())
}

/// Materializes exactly the `EventCore` subtree from one verified revision into a temporary work tree.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the fixed Git object check and two plumbing commands preserve an explicit disposable subtree-materialization sequence"
)]
fn materialize_revision(
    repository: &Path,
    revision: &TiberRevision,
    destination: &Path,
) -> Result<(), GitStoreError> {
    let index = destination.join("tiber-temporary.index");
    let event_store_tree = format!("{}:{EVENT_STORE_DIRECTORY}", revision.as_str());
    let object_type = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["cat-file", "-t", event_store_tree.as_str()],
        None,
        None,
    )?;
    if !object_type.status.success() {
        return Err(GitStoreError::EventDirectoryMissing);
    }
    if object_type.stdout != b"tree\n" {
        return Err(GitStoreError::EventHistory);
    }
    let read_tree = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["read-tree", event_store_tree.as_str()],
        None,
        Some(&index),
    )?;
    require_git_success(&read_tree, GitOperation::MaterializeTiberSnapshot)?;
    let checkout = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["checkout-index", "--all", "--force", "--prefix=eventstore/"],
        Some(destination),
        Some(&index),
    )?;
    require_git_success(&checkout, GitOperation::MaterializeTiberSnapshot)?;
    Ok(())
}

/// Builds a root-tree index while materializing only the committed `EventCore` subtree.
///
/// The publication index retains every path from the signed base revision, while a
/// second disposable index limits checkout to `eventstore`. This keeps unrelated
/// repository files out of the temporary work tree without dropping them from the
/// signed candidate tree.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the two-index publication materialization keeps full-tree preservation and subtree isolation explicit"
)]
fn materialize_publishable_revision(
    repository: &Path,
    revision: &TiberRevision,
    destination: &Path,
    publication_index: &Path,
    materialization_index: &Path,
) -> Result<(), GitStoreError> {
    let event_store_tree = format!("{}:{EVENT_STORE_DIRECTORY}", revision.as_str());
    let object_type = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["cat-file", "-t", event_store_tree.as_str()],
        None,
        None,
    )?;
    if !object_type.status.success() {
        return Err(GitStoreError::EventDirectoryMissing);
    }
    if object_type.stdout != b"tree\n" {
        return Err(GitStoreError::EventHistory);
    }
    let root_index = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["read-tree", revision.as_str()],
        None,
        Some(publication_index),
    )?;
    require_git_success(&root_index, GitOperation::MaterializeTiberSnapshot)?;
    let subtree_index = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["read-tree", event_store_tree.as_str()],
        None,
        Some(materialization_index),
    )?;
    require_git_success(&subtree_index, GitOperation::MaterializeTiberSnapshot)?;
    let checkout = run_git(
        repository,
        GitOperation::MaterializeTiberSnapshot,
        &["checkout-index", "--all", "--force", "--prefix=eventstore/"],
        Some(destination),
        Some(materialization_index),
    )?;
    require_git_success(&checkout, GitOperation::MaterializeTiberSnapshot)?;
    Ok(())
}

/// Requires a signed snapshot to retain its committed `EventCore` transaction directory.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the one-purpose boundary preserves distinct missing and malformed snapshot errors while validating both committed directories"
)]
fn require_safe_event_store_layout(snapshot: &Path) -> Result<(), GitStoreError> {
    let event_store_directory = snapshot.join(EVENT_STORE_DIRECTORY);
    require_directory(&event_store_directory, true)?;

    let events_directory = event_store_directory.join(EVENTS_DIRECTORY);
    require_directory(&events_directory, true)?;
    for relative_path in EVENTCORE_DERIVED_DIRECTORIES {
        require_directory(&event_store_directory.join(relative_path), false)?;
    }
    for relative_path in EVENTCORE_DERIVED_FILES {
        require_regular_file(&event_store_directory.join(relative_path))?;
    }
    Ok(())
}

/// Requires a fixed snapshot path to be a real directory when present.
#[expect(
    clippy::implicit_return,
    reason = "the fixed path-kind check keeps missing required history distinct from malformed derived state"
)]
fn require_directory(path: &Path, required: bool) -> Result<(), GitStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_metadata) => Err(GitStoreError::EventHistory),
        Err(source) if source.kind() == io::ErrorKind::NotFound && required => {
            Err(GitStoreError::EventDirectoryMissing)
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_source) => Err(GitStoreError::EventHistory),
    }
}

/// Requires a fixed derived write target to be a real regular file when present.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the bounded target list rejects symlinks and non-files before EventCore can create or overwrite local state"
)]
fn require_regular_file(path: &Path) -> Result<(), GitStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_metadata) => Err(GitStoreError::EventHistory),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_source) => Err(GitStoreError::EventHistory),
    }
}

/// Runs fixed Git argv with normal host SSH configuration and no shell.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    reason = "the process boundary propagates only stable semantic errors while using temporary work-tree and index paths"
)]
fn run_git(
    repository: &Path,
    operation: GitOperation,
    arguments: &[&str],
    work_tree: Option<&Path>,
    index: Option<&Path>,
) -> Result<Output, GitStoreError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository);
    if let Some(work_tree) = work_tree {
        command.arg("--work-tree").arg(work_tree);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command.args(arguments);
    #[cfg(target_os = "linux")]
    command.process_group(0);

    let mut child = command
        .spawn()
        .map_err(|source| GitCommandFailure::io(operation, source))?;
    let timed_status = child
        .wait_timeout(timeout_for(operation))
        .map_err(|source| GitCommandFailure::io(operation, source))?;
    if timed_status.is_none() {
        #[cfg(target_os = "linux")]
        let process_group_termination = terminate_process_group(child.id());
        let _killed = child.kill();
        let _waited = child.wait();
        #[cfg(target_os = "linux")]
        return match process_group_termination {
            Ok(()) => Err(timed_out_failure(operation)),
            Err(source) => Err(GitCommandFailure::io(operation, source).into()),
        };
        #[cfg(not(target_os = "linux"))]
        return Err(timed_out_failure(operation));
    }
    child
        .wait_with_output()
        .map_err(|source| GitCommandFailure::io(operation, source).into())
}

/// Returns the finite deadline proportionate to one fixed Git operation.
///
/// Remote authority refresh includes an isolated first fetch, which can
/// legitimately transfer a complete signed history. Local inspection and
/// materialization remain on the shorter deadline so stalled local helpers are
/// still cleaned up promptly.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the closed operation mapping makes the exceptional remote-fetch deadline explicit"
)]
const fn timeout_for(operation: GitOperation) -> Duration {
    match operation {
        GitOperation::PublishTiberEvents | GitOperation::RefreshOriginTiberRef => {
            REMOTE_AUTHORITY_GIT_TIMEOUT
        }
        GitOperation::AppendTiberEvents
        | GitOperation::MaterializeTiberSnapshot
        | GitOperation::ResolveTiberRef
        | GitOperation::SignTiberCandidate
        | GitOperation::VerifyTiberSignature => LOCAL_GIT_TIMEOUT,
    }
}

/// Forces every member of one still-unreaped Git child's dedicated Linux process group to exit.
///
/// The child PID remains reserved until `run_git` reaps it, so its equal process-group ID cannot
/// be reused between the timeout decision and this direct, no-shell signal invocation.
#[cfg(target_os = "linux")]
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the one-purpose Linux process-group boundary keeps signal argv separate from Git command construction and propagates a failed cleanup command"
)]
fn terminate_process_group(process_group_id: u32) -> io::Result<()> {
    let target = format!("-{process_group_id}");
    let status = Command::new("kill")
        .args(["-KILL", "--", target.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("process-group termination command failed"))
    }
}

/// Builds the stable timeout outcome after bounded process cleanup completed.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the timeout outcome is a closed sanitized process-failure value"
)]
fn timed_out_failure(operation: GitOperation) -> GitStoreError {
    GitCommandFailure {
        exit_code: None,
        io_source: None,
        kind: GitCommandFailureKind::TimedOut,
        operation,
        retryability: Retryability::Retryable,
    }
    .into()
}

/// Turns a non-success Git output into the operation's stable error.
#[expect(
    clippy::implicit_return,
    reason = "the one-purpose status check retains a closed Git-to-domain error mapping"
)]
fn require_git_success(output: &Output, operation: GitOperation) -> Result<(), GitStoreError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(git_nonzero_error(operation, output))
    }
}

/// Retains a numeric Git status without capturing untrusted command output.
#[expect(
    clippy::implicit_return,
    reason = "the stable process failure preserves only operation, status, and retryability"
)]
fn git_nonzero_error(operation: GitOperation, output: &Output) -> GitStoreError {
    let exit_code = output.status.code();
    let retryability = match (operation, exit_code) {
        (GitOperation::RefreshOriginTiberRef, Some(GIT_NOT_FOUND_EXIT))
        | (
            GitOperation::AppendTiberEvents
            | GitOperation::MaterializeTiberSnapshot
            | GitOperation::ResolveTiberRef
            | GitOperation::SignTiberCandidate
            | GitOperation::VerifyTiberSignature,
            _,
        ) => Retryability::Permanent,
        (GitOperation::PublishTiberEvents | GitOperation::RefreshOriginTiberRef, _) => {
            Retryability::Retryable
        }
    };
    GitCommandFailure {
        exit_code,
        io_source: None,
        kind: GitCommandFailureKind::NonZeroExit,
        operation,
        retryability,
    }
    .into()
}

/// Builds the minimal transaction catalog needed to interpret `EventCore` forks.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    reason = "the catalog parser keeps transient file and envelope data local while mapping all failures to one stable source error"
)]
fn inspect_event_history(events_directory: &Path) -> Result<EventHistoryCatalog, GitStoreError> {
    let mut paths = fs::read_dir(events_directory)
        .map_err(|_source| GitStoreError::EventCatalog)?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|_source| GitStoreError::EventCatalog)
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();

    let mut transactions = BTreeMap::new();
    for path in paths {
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let metadata =
            fs::symlink_metadata(&path).map_err(|_source| GitStoreError::EventHistory)?;
        if !metadata.is_file() {
            return Err(GitStoreError::EventHistory);
        }
        let (transaction_id, entry) = inspect_transaction_file(&path)?;
        if transactions.insert(transaction_id, entry).is_some() {
            return Err(GitStoreError::EventCatalog);
        }
    }
    Ok(EventHistoryCatalog { transactions })
}

/// Parses one immutable `EventCore` JSONL transaction file.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    reason = "header and envelope parsing retains a small local state machine with one stable invalid-catalog outcome"
)]
fn inspect_transaction_file(
    path: &Path,
) -> Result<(String, TransactionCatalogEntry), GitStoreError> {
    let contents = fs::read_to_string(path).map_err(|_source| GitStoreError::EventCatalog)?;
    let file_stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or(GitStoreError::EventCatalog)?;
    let mut transaction_id = None;
    let mut parent_ids = None;
    let mut declared_streams = None;
    let mut event_envelopes = Vec::new();
    let mut written_stream_ids = BTreeSet::new();

    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<CatalogRecord>(line)
            .map_err(|_source| GitStoreError::EventCatalog)?;
        match record.record.as_str() {
            "header" => parse_catalog_header(
                record,
                file_stem,
                &mut transaction_id,
                &mut parent_ids,
                &mut declared_streams,
            )?,
            "event" => parse_catalog_event(
                record,
                declared_streams.as_ref(),
                &mut event_envelopes,
                &mut written_stream_ids,
            )?,
            _ => return Err(GitStoreError::EventCatalog),
        }
    }

    let transaction_id = transaction_id.ok_or(GitStoreError::EventCatalog)?;
    let parent_ids = parent_ids.ok_or(GitStoreError::EventCatalog)?;
    let _declared_streams = declared_streams.ok_or(GitStoreError::EventCatalog)?;
    Ok((
        transaction_id,
        TransactionCatalogEntry {
            event_envelopes,
            parent_ids,
            written_stream_ids,
        },
    ))
}

/// Validates and captures a header for one immutable transaction file.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the header is deliberately parsed as one atomic state transition"
)]
fn parse_catalog_header(
    record: CatalogRecord,
    file_stem: &str,
    transaction_id: &mut Option<String>,
    parent_ids: &mut Option<Vec<String>>,
    declared_streams: &mut Option<BTreeMap<String, usize>>,
) -> Result<(), GitStoreError> {
    let Some(id) = record.transaction_id else {
        return Err(GitStoreError::EventCatalog);
    };
    let Some(parents) = record.parent_transaction_ids else {
        return Err(GitStoreError::EventCatalog);
    };
    let Some(stream_bases) = record.stream_bases else {
        return Err(GitStoreError::EventCatalog);
    };
    if id != file_stem
        || transaction_id.replace(id).is_some()
        || parent_ids.replace(parents).is_some()
        || declared_streams.replace(stream_bases).is_some()
    {
        return Err(GitStoreError::EventCatalog);
    }
    Ok(())
}

/// Confirms each actual event stream was declared by its transaction header.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "this envelope-level boundary validates typed envelope fields while retaining one stable invalid-catalog outcome"
)]
fn parse_catalog_event(
    record: CatalogRecord,
    declared_streams: Option<&BTreeMap<String, usize>>,
    event_envelopes: &mut Vec<EventCatalogEntry>,
    written_stream_ids: &mut BTreeSet<String>,
) -> Result<(), GitStoreError> {
    let Some(stream_id) = record.stream_id else {
        return Err(GitStoreError::EventCatalog);
    };
    let Some(header_streams) = declared_streams else {
        return Err(GitStoreError::EventCatalog);
    };
    if !header_streams.contains_key(&stream_id) {
        return Err(GitStoreError::EventCatalog);
    }
    let parsed_stream_id =
        StreamId::try_new(stream_id.clone()).map_err(|_source| GitStoreError::EventCatalog)?;
    let _written_inserted = written_stream_ids.insert(stream_id);
    event_envelopes.push(EventCatalogEntry {
        stream_id: parsed_stream_id,
        event_type: record.event_type,
        event_data: record.event_data,
    });
    Ok(())
}

/// Collects distinct persisted stream identities without assigning them a replay order.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the immutable catalog-to-stream projection is a compact closed read-only transformation"
)]
fn stream_ids_from_catalog(catalog: &EventHistoryCatalog) -> Result<Vec<StreamId>, GitStoreError> {
    catalog
        .transactions
        .values()
        .flat_map(|transaction| &transaction.event_envelopes)
        .map(|envelope| {
            let stream_id: &str = envelope.stream_id.as_ref();
            stream_id.to_owned()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|stream_id| {
            StreamId::try_new(stream_id).map_err(|_source| GitStoreError::EventCatalog)
        })
        .collect()
}

/// Returns the exact append version for one stream in an immutable validated catalog.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "an EventCore stream version is the count of validated envelopes in that exact stream"
)]
fn stream_version_from_catalog(
    catalog: &EventHistoryCatalog,
    stream: &StreamId,
) -> eventcore_types::StreamVersion {
    let count = catalog
        .transactions
        .values()
        .flat_map(|transaction| &transaction.event_envelopes)
        .filter(|envelope| envelope.stream_id == *stream)
        .count();
    eventcore_types::StreamVersion::new(count)
}

/// Checks whether an immutable envelope belongs to any member of a closed stream union.
#[expect(
    clippy::implicit_return,
    reason = "the stream-pattern union is one pure selection predicate shared by validation and reconstruction"
)]
fn matches_stream_patterns(envelope: &EventCatalogEntry, patterns: &[StreamPattern]) -> bool {
    patterns
        .iter()
        .any(|pattern| pattern.matches(envelope.stream_id.as_ref()))
}

/// Decodes one immutable envelope while preserving the selected stream's stable failure context.
#[expect(
    clippy::implicit_return,
    reason = "the typed decode boundary maps implementation detail to EventCore's durable selected-stream error"
)]
fn decode_envelope<E: Event>(envelope: &EventCatalogEntry) -> Result<E, EventStoreError> {
    serde_json::from_value::<E>(envelope.event_data.clone()).map_err(|_source| {
        EventStoreError::DeserializationFailed {
            stream_id: envelope.stream_id.clone(),
            detail: "event payload is incompatible with the requested event type".to_owned(),
        }
    })
}

/// Verifies one selected payload and its durable envelope stream identity.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the complete typed envelope check remains one compact fail-closed boundary"
)]
fn verify_envelope_decodes<E: Event>(envelope: &EventCatalogEntry) -> Result<(), EventStoreError> {
    let event = decode_envelope::<E>(envelope)?;
    if event.stream_id() == &envelope.stream_id {
        Ok(())
    } else {
        Err(EventStoreError::DeserializationFailed {
            stream_id: envelope.stream_id.clone(),
            detail: "event payload stream identity differs from the event envelope".to_owned(),
        })
    }
}

/// Includes every immutable parent needed to establish selected facts' causal order.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the iterative ancestry walk keeps the bounded selected subgraph explicit and fail-closed"
)]
fn transaction_ancestry(
    selected: &BTreeSet<String>,
    transactions: &BTreeMap<String, TransactionCatalogEntry>,
) -> Result<BTreeSet<String>, TransactionHistoryError> {
    let mut relevant = selected.clone();
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(transaction_id) = pending.pop() {
        let transaction = transactions
            .get(&transaction_id)
            .ok_or(TransactionHistoryError::AmbiguousTransactionOrder)?;
        for parent_id in &transaction.parent_ids {
            if !transactions.contains_key(parent_id) {
                return Err(TransactionHistoryError::AmbiguousTransactionOrder);
            }
            if relevant.insert(parent_id.clone()) {
                pending.push(parent_id.clone());
            }
        }
    }
    Ok(relevant)
}

/// Returns one immutable transaction sequence only when the selected ancestry is a chain.
///
/// `EventCore`'s filesystem adapter can canonically linearize concurrent writes,
/// but exposes no public global canonical replay API. Tiber's bounded task
/// slice intentionally accepts its retained single-writer history only: a
/// selected fork, merge, cycle, or independent root has no single product
/// chronology and is rejected instead of receiving a filename-based order.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the selected graph is deliberately accepted only through an explicit one-root one-child chain proof"
)]
fn linear_transaction_order(
    relevant: &BTreeSet<String>,
    transactions: &BTreeMap<String, TransactionCatalogEntry>,
) -> Result<Vec<String>, TransactionHistoryError> {
    if relevant.is_empty() {
        return Ok(Vec::new());
    }
    let mut children = BTreeMap::new();
    let mut roots = Vec::new();
    for transaction_id in relevant {
        let transaction = transactions
            .get(transaction_id)
            .ok_or(TransactionHistoryError::AmbiguousTransactionOrder)?;
        let Some(parent_id) = transaction.parent_ids.first() else {
            roots.push(transaction_id.clone());
            continue;
        };
        if transaction.parent_ids.get(1).is_some() || !relevant.contains(parent_id) {
            return Err(TransactionHistoryError::AmbiguousTransactionOrder);
        }
        if children
            .insert(parent_id.clone(), transaction_id.clone())
            .is_some()
        {
            return Err(TransactionHistoryError::AmbiguousTransactionOrder);
        }
    }
    let Some(root) = roots.first() else {
        return Err(TransactionHistoryError::AmbiguousTransactionOrder);
    };
    if roots.get(1).is_some() {
        return Err(TransactionHistoryError::AmbiguousTransactionOrder);
    }
    let mut ordered = Vec::with_capacity(relevant.len());
    let mut visited = BTreeSet::new();
    let mut current = root.clone();
    while ordered.len() < relevant.len() {
        if !relevant.contains(&current) || !visited.insert(current.clone()) {
            return Err(TransactionHistoryError::AmbiguousTransactionOrder);
        }
        ordered.push(current.clone());
        let Some(next) = children.get(&current) else {
            break;
        };
        current.clone_from(next);
    }
    if ordered.len() == relevant.len() {
        Ok(ordered)
    } else {
        Err(TransactionHistoryError::AmbiguousTransactionOrder)
    }
}

/// Rejects `EventCore` integrity/dangling failures and true actual-writer forks.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "EventCore status and fork inspection remain explicit fail-closed source validation steps"
)]
fn verify_history_integrity(
    store: &FileEventStore,
    catalog: &EventHistoryCatalog,
) -> Result<(), GitStoreError> {
    let status = store
        .status()
        .map_err(|_source| GitStoreError::EventHistory)?;
    if !status.integrity_failures().is_empty() {
        return Err(GitStoreError::EventHistoryIntegrityFailed);
    }
    if !status.dangling().is_empty() {
        return Err(GitStoreError::EventHistoryDanglingTransaction);
    }
    let forks = store
        .detect_forks()
        .map_err(|_source| GitStoreError::EventHistory)?;
    for fork in forks {
        verify_fork(&fork, catalog)?;
    }
    Ok(())
}

/// Accepts header-only historic read contexts while rejecting actual write conflicts.
///
/// `EventCore` reports all transactions that shared a stream base, including a
/// transaction that only read that stream. Zero actual writers are replayable;
/// two writers conflict. With exactly one writer, every other candidate must be
/// in its parent graph, proving the writer was causally after the reader.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the fork classifier deliberately preserves EventCore's causal-read semantics in a compact closed case analysis"
)]
fn verify_fork(
    fork: &eventcore_fs::Fork,
    catalog: &EventHistoryCatalog,
) -> Result<(), GitStoreError> {
    let candidates = fork
        .transactions()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let stream_id = fork.stream_id().as_ref();
    let writers = candidates
        .iter()
        .filter(|transaction_id| {
            catalog
                .transactions
                .get(transaction_id.as_str())
                .is_some_and(|entry| entry.written_stream_ids.contains(stream_id))
        })
        .cloned()
        .collect::<Vec<_>>();
    if writers.len() > 1 {
        return Err(GitStoreError::EventHistoryForkDetected);
    }
    let Some(writer) = writers.first() else {
        return Ok(());
    };
    for candidate in candidates {
        if candidate != *writer && !is_transaction_ancestor(&candidate, writer, catalog)? {
            return Err(GitStoreError::EventHistoryForkDetected);
        }
    }
    Ok(())
}

/// Returns whether `ancestor` occurs in `descendant`'s immutable parent graph.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the iterative graph walk avoids recursion while retaining typed invalid-catalog handling"
)]
fn is_transaction_ancestor(
    ancestor: &str,
    descendant: &str,
    catalog: &EventHistoryCatalog,
) -> Result<bool, GitStoreError> {
    let mut pending = vec![descendant.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let Some(entry) = catalog.transactions.get(&current) else {
            return Err(GitStoreError::EventCatalog);
        };
        for parent in &entry.parent_ids {
            if parent == ancestor {
                return Ok(true);
            }
            pending.push(parent.clone());
        }
    }
    Ok(false)
}

#[cfg(test)]
mod git_error_tests {
    use core::error::Error as _;
    use std::{io, process::Command};

    use super::{
        GitCommandFailure, GitCommandFailureKind, GitOperation, GitStoreError, Retryability,
    };

    #[cfg(target_os = "linux")]
    use core::time::Duration;

    #[cfg(target_os = "linux")]
    use std::{fs, path::Path, process::Stdio, thread, time::Instant};

    #[cfg(target_os = "linux")]
    use tempfile::TempDir;

    #[cfg(target_os = "linux")]
    struct FixtureProcessCleanup {
        process_ids: Vec<u32>,
    }

    #[cfg(target_os = "linux")]
    impl Drop for FixtureProcessCleanup {
        fn drop(&mut self) {
            for process_id in &self.process_ids {
                let _status = Command::new("kill")
                    .args(["-KILL", "--", process_id.to_string().as_str()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the focused failure-contract test fails fast when the known process variant is absent"
    )]
    fn classifies_a_timed_out_refresh_as_retryable_without_output() {
        let error = GitStoreError::from(GitCommandFailure {
            exit_code: None,
            io_source: None,
            kind: GitCommandFailureKind::TimedOut,
            operation: GitOperation::RefreshOriginTiberRef,
            retryability: Retryability::Retryable,
        });

        assert_eq!(error.code(), "tiber_git_refresh_origin_tiber_ref_failed");
        assert_eq!(
            error.to_string(),
            "tiber_git_refresh_origin_tiber_ref_failed"
        );
        assert_eq!(error.retryability(), Retryability::Retryable);
        let failure = error
            .git_command_failure()
            .expect("timeout must retain Git process context");
        assert_eq!(failure.operation(), GitOperation::RefreshOriginTiberRef);
        assert_eq!(failure.kind(), GitCommandFailureKind::TimedOut);
        assert_eq!(failure.exit_code(), None);
        assert!(failure.io_source().is_none());
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the focused failure-contract test fails fast when the known process variant is absent"
    )]
    fn retains_a_retryable_io_cause_without_displaying_it() {
        let error = GitStoreError::from(GitCommandFailure::io(
            GitOperation::RefreshOriginTiberRef,
            io::Error::new(io::ErrorKind::TimedOut, "fixture process detail"),
        ));

        assert_eq!(error.code(), "tiber_git_refresh_origin_tiber_ref_failed");
        assert_eq!(
            error.to_string(),
            "tiber_git_refresh_origin_tiber_ref_failed"
        );
        assert_eq!(error.retryability(), Retryability::Retryable);
        assert!(error.source().is_some());
        let failure = error
            .git_command_failure()
            .expect("I/O failure must retain Git process context");
        assert_eq!(failure.operation(), GitOperation::RefreshOriginTiberRef);
        assert_eq!(failure.kind(), GitCommandFailureKind::Io);
        assert_eq!(failure.exit_code(), None);
        assert!(failure.io_source().is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the focused timeout-cleanup contract test fails fast when the typed Git process context is absent"
    )]
    fn reports_a_process_group_cleanup_failure_as_a_typed_process_error() {
        let error = GitStoreError::from(GitCommandFailure::io(
            GitOperation::ResolveTiberRef,
            io::Error::from(io::ErrorKind::NotFound),
        ));

        assert_eq!(error.code(), "tiber_git_resolve_tiber_ref_failed");
        assert_eq!(error.to_string(), "tiber_git_resolve_tiber_ref_failed");
        assert_eq!(error.retryability(), Retryability::Permanent);
        let failure = error
            .git_command_failure()
            .expect("cleanup failure must retain Git process context");
        assert_eq!(failure.operation(), GitOperation::ResolveTiberRef);
        assert_eq!(failure.kind(), GitCommandFailureKind::Io);
        assert_eq!(failure.exit_code(), None);
        assert_eq!(
            failure.io_source().map(io::Error::kind),
            Some(io::ErrorKind::NotFound)
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "the focused fixed-Git failure fixture fails fast if the test process cannot run"
    )]
    #[test]
    fn classifies_candidate_signing_failure_as_permanent() {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "definitely-not-a-tiber-revision"])
            .output()
            .expect("the fixed Git fixture should start");
        assert!(!output.status.success());
        let error = super::git_nonzero_error(GitOperation::SignTiberCandidate, &output);
        assert_eq!(error.code(), "tiber_git_sign_tiber_candidate_failed");
        assert_eq!(error.retryability(), Retryability::Permanent,);
    }

    #[cfg(target_os = "linux")]
    #[expect(
        clippy::expect_used,
        reason = "the focused Linux process-boundary fixture intentionally fails fast while proving local timeout cleanup"
    )]
    #[test]
    fn local_timeout_terminates_git_helpers_and_their_descendants() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let repository = directory.path().join("repository");
        let helper = directory.path().join("persistent-ssh-helper");
        let process_marker = directory.path().join("helper-processes");
        fixture_git(
            directory.path(),
            &["init", repository.to_str().expect("UTF-8 repository path")],
        );
        create_persistent_ssh_helper(&helper);
        fixture_git(
            &repository,
            &[
                "config",
                "core.sshCommand",
                &format!(
                    "{} {}",
                    helper.to_str().expect("UTF-8 helper path"),
                    process_marker.to_str().expect("UTF-8 marker path"),
                ),
            ],
        );
        fixture_git(&repository, &["config", "ssh.variant", "simple"]);

        let error = super::run_git(
            &repository,
            GitOperation::ResolveTiberRef,
            &["ls-remote", "ssh://fixture.invalid/tiber.git"],
            None,
            None,
        )
        .expect_err("the persistent SSH transport must reach the local timeout");
        let process_ids = fixture_process_ids(&process_marker);
        let _cleanup = FixtureProcessCleanup {
            process_ids: process_ids.clone(),
        };

        assert_eq!(error.code(), "tiber_git_resolve_tiber_ref_failed");
        let failure = error
            .git_command_failure()
            .expect("the timeout must retain Git process context");
        assert_eq!(failure.operation(), GitOperation::ResolveTiberRef);
        assert_eq!(failure.kind(), GitCommandFailureKind::TimedOut);
        assert!(
            wait_until_processes_exit(&process_ids, Duration::from_secs(2)),
            "no Git transport helper or descendant may survive timeout cleanup: {process_ids:?}",
        );
    }

    #[cfg(target_os = "linux")]
    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the isolated transport fixture compiles one no-shell helper with the pinned Rust toolchain"
    )]
    fn create_persistent_ssh_helper(helper: &Path) {
        let source = helper.with_extension("rs");
        fs::write(
            &source,
            r#"
use std::{env, fs, process::{self, Command, Stdio}};

fn main() {
    let marker = env::args_os().nth(1).expect("marker argument");
    let mut descendant = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("persistent descendant");
    fs::write(marker, format!("{}\n{}\n", process::id(), descendant.id()))
        .expect("process marker");
    let _status = descendant.wait();
}
"#,
        )
        .expect("persistent SSH helper source should write");
        let output = Command::new("rustc")
            .args(["--edition", "2024"])
            .arg(&source)
            .arg("-o")
            .arg(helper)
            .output()
            .expect("persistent SSH helper compilation should start");
        assert!(
            output.status.success(),
            "persistent SSH helper should compile: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[cfg(target_os = "linux")]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the process fixture deliberately fails fast and returns the exact helper PID set it created"
    )]
    fn fixture_process_ids(marker: &Path) -> Vec<u32> {
        let process_ids = fs::read_to_string(marker)
            .expect("persistent SSH helper should record its processes")
            .lines()
            .map(|line| line.parse().expect("fixture process ID should be numeric"))
            .collect::<Vec<_>>();
        assert_eq!(
            process_ids.len(),
            2,
            "the helper must create one persistent descendant",
        );
        process_ids
    }

    #[cfg(target_os = "linux")]
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the focused Linux fixture polls its observable process boundary until every recorded process is reaped"
    )]
    fn wait_until_processes_exit(process_ids: &[u32], timeout: Duration) -> bool {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            if process_ids
                .iter()
                .all(|process_id| !Path::new("/proc").join(process_id.to_string()).exists())
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    #[expect(
        clippy::expect_used,
        reason = "fixture Git commands intentionally expose stderr only through fail-fast assertion diagnostics"
    )]
    fn fixture_git(repository: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(
            output.status.success(),
            "fixture Git command should succeed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
