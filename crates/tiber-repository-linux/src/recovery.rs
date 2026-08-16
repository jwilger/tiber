//! Private fully-fsynced recovery journal for repository mutation receipts.

use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::{error::Error, fmt, future::Future};
use eventcore_fs::{FileEventStore, FsConfig, FsyncPolicy};
use eventcore_types::{
    Event, EventStore as _, StreamId, StreamVersion, StreamWrites, collect_events,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, thread};
use tiber_repository_core::{
    RepositoryDispatchOutcome, RepositoryId, RepositoryMutationFailure, RepositoryMutationIdentity,
    RepositoryMutationReceipt, RepositoryReconciliation, RepositoryReconciliationOutcome,
};
use tokio::runtime::Builder as RuntimeBuilder;

/// Version of the private repository receipt fact schema.
const JOURNAL_SCHEMA_VERSION: u16 = 1;

/// Stable owner-facing recovery-store failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::enum_variant_names,
    reason = "owner recovery code must distinguish the three closed fail-closed states"
)]
pub enum LinuxRepositoryRecoveryError {
    /// The fully durable journal could not be opened, read, or appended.
    StateUnavailable,
    /// The journal failed integrity checks or contains an impossible transition.
    StateCorrupt,
    /// The journal belongs to a different repository or unsupported schema.
    StateStale,
}

impl LinuxRepositoryRecoveryError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable recovery error code table reads clearest as a tail match"
    )]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StateUnavailable => "repository_linux_recovery_state_unavailable",
            Self::StateCorrupt => "repository_linux_recovery_state_corrupt",
            Self::StateStale => "repository_linux_recovery_state_stale",
        }
    }
}

impl fmt::Display for LinuxRepositoryRecoveryError {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "display delegates directly to the stable recovery error code"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the closed recovery error does not wrap a lower-level source"
)]
impl Error for LinuxRepositoryRecoveryError {}

/// Read-only restart projection containing only ambiguity-derived handles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinuxRepositoryRecoveryScan {
    /// Safe ambiguity handles projected from prepared or unknown facts.
    pending: Vec<RepositoryReconciliation>,
}

impl LinuxRepositoryRecoveryScan {
    /// Returns mutations that may only be reconciled and must never auto-replay.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the read-only recovery scan exposes one direct borrowed slice"
    )]
    pub fn pending(&self) -> &[RepositoryReconciliation] {
        &self.pending
    }
}

/// One immutable, fully-fsynced journal fact.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "fact", rename_all = "snake_case")]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the private serde fact schema is documented as one closed journal unit"
)]
enum RepositoryJournalEvent {
    /// Authority identity persisted before any worker request bytes are dispatched.
    Prepared {
        /// Private fact schema version.
        schema_version: u16,
        /// Fixed journal stream.
        stream: StreamId,
        /// Full safe identity, without mutation authority or raw content.
        identity: RepositoryMutationIdentity,
    },
    /// Durable terminal proof that the worker applied the mutation.
    Applied {
        /// Private fact schema version.
        schema_version: u16,
        /// Fixed journal stream.
        stream: StreamId,
        /// Safe applied receipt.
        receipt: RepositoryMutationReceipt,
    },
    /// Durable terminal proof that mutation application did not occur.
    Failed {
        /// Private fact schema version.
        schema_version: u16,
        /// Fixed journal stream.
        stream: StreamId,
        /// Safe terminal failure receipt.
        failure: RepositoryMutationFailure,
    },
    /// Durable ambiguity after worker dispatch may have begun.
    Unknown {
        /// Private fact schema version.
        schema_version: u16,
        /// Fixed journal stream.
        stream: StreamId,
        /// Safe read-only reconciliation handle.
        reconciliation: RepositoryReconciliation,
    },
    /// Durable result of a read-only reconciliation query.
    Reconciled {
        /// Private fact schema version.
        schema_version: u16,
        /// Fixed journal stream.
        stream: StreamId,
        /// Safe reconciliation outcome.
        outcome: RepositoryReconciliationOutcome,
    },
}

#[expect(
    clippy::implicit_return,
    clippy::arbitrary_source_item_ordering,
    clippy::pattern_type_mismatch,
    reason = "private journal event projections are total closed matches"
)]
impl RepositoryJournalEvent {
    /// Returns the private fact schema version.
    fn schema_version(&self) -> u16 {
        match self {
            Self::Prepared { schema_version, .. }
            | Self::Applied { schema_version, .. }
            | Self::Failed { schema_version, .. }
            | Self::Unknown { schema_version, .. }
            | Self::Reconciled { schema_version, .. } => *schema_version,
        }
    }

    /// Returns the full safe mutation identity carried by this fact.
    fn identity(&self) -> &RepositoryMutationIdentity {
        match self {
            Self::Prepared { identity, .. } => identity,
            Self::Applied { receipt, .. } => receipt.identity(),
            Self::Failed { failure, .. } => failure.identity(),
            Self::Unknown { reconciliation, .. } => reconciliation.identity(),
            Self::Reconciled { outcome, .. } => reconciliation_identity(outcome),
        }
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "Event's closed stream projection and fixed type name are direct values"
)]
impl Event for RepositoryJournalEvent {
    fn stream_id(&self) -> &StreamId {
        match self {
            Self::Prepared { stream, .. }
            | Self::Applied { stream, .. }
            | Self::Failed { stream, .. }
            | Self::Unknown { stream, .. }
            | Self::Reconciled { stream, .. } => stream,
        }
    }

    fn event_type_name() -> &'static str {
        "RepositoryJournalEvent"
    }
}

/// Replayed durable state for one idempotency identity.
#[derive(Clone, Debug)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "private durable variants mirror the fully documented journal transition states"
)]
enum DurableState {
    /// Mutation intent is durable but no terminal fact exists.
    Prepared(RepositoryMutationIdentity),
    /// Applied terminal receipt is durable.
    Applied(RepositoryMutationReceipt),
    /// Definitely-not-applied terminal failure is durable.
    Failed(RepositoryMutationFailure),
    /// Mutation outcome remains ambiguous.
    Unknown(RepositoryReconciliation),
    /// Latest read-only reconciliation result is durable.
    Reconciled(RepositoryReconciliationOutcome),
}

#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "private durable identity projection is one total match"
)]
impl DurableState {
    /// Returns the full safe identity shared by every durable state.
    fn identity(&self) -> &RepositoryMutationIdentity {
        match self {
            Self::Prepared(identity) => identity,
            Self::Applied(receipt) => receipt.identity(),
            Self::Failed(failure) => failure.identity(),
            Self::Unknown(reconciliation) => reconciliation.identity(),
            Self::Reconciled(outcome) => reconciliation_identity(outcome),
        }
    }
}

/// Result of looking up a dispatch idempotency identity before process launch.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "private replay variants follow dispatch lifecycle order"
)]
#[expect(
    clippy::pub_with_shorthand,
    reason = "the recovery module exposes replay decisions only to its parent adapter module"
)]
pub(super) enum JournalDispatchReplay {
    /// No fact exists for this idempotency identity.
    New,
    /// Replay a durable applied receipt without launching.
    Applied(RepositoryMutationReceipt),
    /// Replay a durable terminal failure without launching.
    Failed(RepositoryMutationFailure),
    /// Preserve prepared or ambiguous state without launching.
    Unknown(RepositoryReconciliation),
}

/// State-derived action for one read-only reconciliation request.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "reconciliation replay variants follow decision order rather than alphabetic order"
)]
#[expect(
    clippy::pub_with_shorthand,
    reason = "the recovery module exposes replay decisions only to its parent adapter module"
)]
pub(super) enum JournalReconciliationReplay {
    /// The handle predates this journal and may receive only a live read-only query.
    Untracked,
    /// A durable terminal mutation receipt proves application.
    Applied,
    /// A durable terminal failure receipt proves non-application.
    NotApplied,
    /// Durable state preserves ambiguity and permits only another read-only query.
    Query,
}

/// Validated state-derived projection over the immutable event journal.
#[expect(
    clippy::pub_with_shorthand,
    reason = "the recovery module exposes its validated projection only to its parent adapter module"
)]
pub(super) struct JournalProjection {
    /// Number of integrity-checked immutable facts used as the expected stream version.
    events: usize,
    /// Current durable state indexed by opaque idempotency key.
    states: BTreeMap<String, DurableState>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "private checked projection helpers use linear fail-closed replay and total matches"
)]
impl JournalProjection {
    /// Replays integrity-checked facts into validated current state.
    fn replay(
        repository_id: &RepositoryId,
        events: Vec<RepositoryJournalEvent>,
    ) -> Result<Self, LinuxRepositoryRecoveryError> {
        let mut projection = Self {
            events: 0,
            states: BTreeMap::new(),
        };
        for event in events {
            projection.apply(repository_id, event)?;
            projection.events = projection
                .events
                .checked_add(1)
                .ok_or(LinuxRepositoryRecoveryError::StateCorrupt)?;
        }
        Ok(projection)
    }

    /// Validates and applies one immutable fact.
    fn apply(
        &mut self,
        repository_id: &RepositoryId,
        event: RepositoryJournalEvent,
    ) -> Result<(), LinuxRepositoryRecoveryError> {
        if event.schema_version() != JOURNAL_SCHEMA_VERSION
            || event.identity().repository_id() != repository_id
        {
            return Err(LinuxRepositoryRecoveryError::StateStale);
        }
        let key = event
            .identity()
            .provenance()
            .idempotency_key()
            .as_str()
            .to_owned();
        match event {
            RepositoryJournalEvent::Prepared { identity, .. } => {
                if self
                    .states
                    .insert(key, DurableState::Prepared(identity))
                    .is_some()
                {
                    return Err(LinuxRepositoryRecoveryError::StateCorrupt);
                }
            }
            RepositoryJournalEvent::Applied { receipt, .. } => {
                let identity = receipt.identity().clone();
                self.advance(key, &identity, DurableState::Applied(receipt))?;
            }
            RepositoryJournalEvent::Failed { failure, .. } => {
                let identity = failure.identity().clone();
                self.advance(key, &identity, DurableState::Failed(failure))?;
            }
            RepositoryJournalEvent::Unknown { reconciliation, .. } => {
                let identity = reconciliation.identity().clone();
                self.advance(key, &identity, DurableState::Unknown(reconciliation))?;
            }
            RepositoryJournalEvent::Reconciled { outcome, .. } => {
                let identity = reconciliation_identity(&outcome).clone();
                self.advance(key, &identity, DurableState::Reconciled(outcome))?;
            }
        }
        Ok(())
    }

    /// Advances one identity only through permitted durable transitions.
    fn advance(
        &mut self,
        key: String,
        identity: &RepositoryMutationIdentity,
        next: DurableState,
    ) -> Result<(), LinuxRepositoryRecoveryError> {
        let current = self
            .states
            .get(&key)
            .ok_or(LinuxRepositoryRecoveryError::StateCorrupt)?;
        if current.identity() != identity
            || !matches!(
                (current, &next),
                (
                    DurableState::Prepared(_),
                    DurableState::Applied(_)
                        | DurableState::Failed(_)
                        | DurableState::Unknown(_)
                        | DurableState::Reconciled(_),
                ) | (
                    DurableState::Unknown(_) | DurableState::Reconciled(_),
                    DurableState::Reconciled(_),
                )
            )
        {
            return Err(LinuxRepositoryRecoveryError::StateCorrupt);
        }
        let _previous: Option<DurableState> = self.states.insert(key, next);
        Ok(())
    }

    /// Resolves restart dispatch from durable state without automatic replay.
    pub(super) fn dispatch_replay(
        &self,
        identity: &RepositoryMutationIdentity,
    ) -> Result<JournalDispatchReplay, LinuxRepositoryRecoveryError> {
        let key = identity.provenance().idempotency_key().as_str();
        let Some(state) = self.states.get(key) else {
            return Ok(JournalDispatchReplay::New);
        };
        if state.identity() != identity {
            return Err(LinuxRepositoryRecoveryError::StateStale);
        }
        Ok(match state {
            DurableState::Applied(receipt) => JournalDispatchReplay::Applied(receipt.clone()),
            DurableState::Failed(failure) => JournalDispatchReplay::Failed(failure.clone()),
            DurableState::Unknown(reconciliation) => {
                JournalDispatchReplay::Unknown(reconciliation.clone())
            }
            DurableState::Prepared(prepared_identity) => JournalDispatchReplay::Unknown(
                RepositoryReconciliation::from_durable_identity(prepared_identity.clone()),
            ),
            DurableState::Reconciled(_) => JournalDispatchReplay::Unknown(
                RepositoryReconciliation::from_durable_identity(state.identity().clone()),
            ),
        })
    }

    /// Resolves read-only reconciliation authority from durable state.
    pub(super) fn reconciliation_replay(
        &self,
        identity: &RepositoryMutationIdentity,
    ) -> Result<JournalReconciliationReplay, LinuxRepositoryRecoveryError> {
        let key = identity.provenance().idempotency_key().as_str();
        let Some(state) = self.states.get(key) else {
            return Ok(JournalReconciliationReplay::Untracked);
        };
        if state.identity() != identity {
            return Err(LinuxRepositoryRecoveryError::StateStale);
        }
        Ok(match state {
            DurableState::Applied(_) => JournalReconciliationReplay::Applied,
            DurableState::Failed(_) => JournalReconciliationReplay::NotApplied,
            DurableState::Reconciled(RepositoryReconciliationOutcome::Applied(_)) => {
                JournalReconciliationReplay::Applied
            }
            DurableState::Reconciled(RepositoryReconciliationOutcome::NotApplied(_)) => {
                JournalReconciliationReplay::NotApplied
            }
            DurableState::Prepared(_)
            | DurableState::Unknown(_)
            | DurableState::Reconciled(RepositoryReconciliationOutcome::StillUnknown(_)) => {
                JournalReconciliationReplay::Query
            }
        })
    }

    /// Consumes the projection into safe pending recovery handles.
    pub(super) fn scan(self) -> LinuxRepositoryRecoveryScan {
        let pending = self
            .states
            .into_values()
            .filter_map(|state| match state {
                DurableState::Prepared(identity) => {
                    Some(RepositoryReconciliation::from_durable_identity(identity))
                }
                DurableState::Unknown(reconciliation) => Some(reconciliation),
                DurableState::Reconciled(RepositoryReconciliationOutcome::StillUnknown(
                    receipt,
                )) => Some(RepositoryReconciliation::from_durable_identity(
                    receipt.identity().clone(),
                )),
                DurableState::Applied(_)
                | DurableState::Failed(_)
                | DurableState::Reconciled(
                    RepositoryReconciliationOutcome::Applied(_)
                    | RepositoryReconciliationOutcome::NotApplied(_),
                ) => None,
            })
            .collect();
        LinuxRepositoryRecoveryScan { pending }
    }
}

/// Borrowed fact input retained only until its safe receipt is serialized.
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::pub_with_shorthand,
    reason = "private fact inputs are parent-visible and ordered by mutation lifecycle"
)]
pub(super) enum JournalFact<'fact> {
    /// Durable pre-dispatch identity.
    Prepared(&'fact RepositoryMutationIdentity),
    /// Terminal or ambiguous dispatch result.
    Dispatch(&'fact Result<RepositoryDispatchOutcome, RepositoryMutationFailure>),
    /// Read-only reconciliation result.
    Reconciled(&'fact RepositoryReconciliationOutcome),
}

#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "private borrowed facts map totally into owned journal events"
)]
impl JournalFact<'_> {
    /// Converts one borrowed safe fact into its owned event representation.
    fn into_event(self, stream: StreamId) -> RepositoryJournalEvent {
        match self {
            Self::Prepared(identity) => RepositoryJournalEvent::Prepared {
                schema_version: JOURNAL_SCHEMA_VERSION,
                stream,
                identity: identity.clone(),
            },
            Self::Dispatch(Ok(RepositoryDispatchOutcome::Applied(receipt))) => {
                RepositoryJournalEvent::Applied {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    stream,
                    receipt: receipt.clone(),
                }
            }
            Self::Dispatch(Ok(RepositoryDispatchOutcome::OutcomeUnknown(reconciliation))) => {
                RepositoryJournalEvent::Unknown {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    stream,
                    reconciliation: reconciliation.clone(),
                }
            }
            Self::Dispatch(Err(failure)) => RepositoryJournalEvent::Failed {
                schema_version: JOURNAL_SCHEMA_VERSION,
                stream,
                failure: failure.clone(),
            },
            Self::Reconciled(outcome) => RepositoryJournalEvent::Reconciled {
                schema_version: JOURNAL_SCHEMA_VERSION,
                stream,
                outcome: outcome.clone(),
            },
        }
    }
}

/// Opens, integrity-checks, and replays the private journal.
#[expect(
    clippy::implicit_return,
    clippy::map_err_ignore,
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    reason = "journal boundary deliberately collapses backend details into closed recovery states"
)]
pub(super) fn load(
    root: &Path,
    repository_id: &RepositoryId,
) -> Result<JournalProjection, LinuxRepositoryRecoveryError> {
    let owned_root = root.to_path_buf();
    let owned_repository_id = repository_id.clone();
    run_on_journal_runtime(move || async move {
        let store = checked_store(&owned_root)?;
        let stream = journal_stream(&owned_repository_id)?;
        let events = collect_events(
            store
                .read_stream::<RepositoryJournalEvent>(stream)
                .await
                .map_err(|_| LinuxRepositoryRecoveryError::StateUnavailable)?,
        )
        .await
        .map_err(|_| LinuxRepositoryRecoveryError::StateCorrupt)?;
        JournalProjection::replay(&owned_repository_id, events)
    })
}

/// Persists one immutable journal fact with full fsync and directory fsync.
#[expect(
    clippy::implicit_return,
    clippy::map_err_ignore,
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    reason = "append follows load in lifecycle order and maps backend details to closed recovery states"
)]
pub(super) fn append(
    root: &Path,
    repository_id: &RepositoryId,
    projection: &JournalProjection,
    fact: JournalFact<'_>,
) -> Result<(), LinuxRepositoryRecoveryError> {
    let owned_root = root.to_path_buf();
    let owned_repository_id = repository_id.clone();
    let stream = journal_stream(&owned_repository_id)?;
    let event = fact.into_event(stream.clone());
    let version = StreamVersion::new(projection.events);
    run_on_journal_runtime(move || async move {
        let store = checked_store(&owned_root)?;
        let writes = StreamWrites::new()
            .register_stream(stream, version)
            .and_then(|writes| writes.append(event))
            .map_err(|_| LinuxRepositoryRecoveryError::StateCorrupt)?;
        let _append_receipt = store
            .append_events(writes)
            .await
            .map_err(|_| LinuxRepositoryRecoveryError::StateUnavailable)?;
        Ok(())
    })
}

/// Opens one short-lived fully-fsynced store and rejects every unclean status.
#[expect(
    clippy::implicit_return,
    clippy::map_err_ignore,
    clippy::question_mark_used,
    reason = "backend errors are intentionally collapsed into the closed recovery contract"
)]
fn checked_store(root: &Path) -> Result<FileEventStore, LinuxRepositoryRecoveryError> {
    let store = FileEventStore::open_with_config(FsConfig::new(root).with_fsync(FsyncPolicy::Full))
        .map_err(|_| LinuxRepositoryRecoveryError::StateUnavailable)?;
    let status = store
        .status()
        .map_err(|_| LinuxRepositoryRecoveryError::StateCorrupt)?;
    if !status.is_clean() {
        return Err(LinuxRepositoryRecoveryError::StateCorrupt);
    }
    Ok(store)
}

/// Returns the one fixed stream that makes repository mismatches observable during replay.
#[expect(
    clippy::implicit_return,
    clippy::map_err_ignore,
    reason = "the fixed validated stream maps its impossible parse failure to stale state"
)]
fn journal_stream(_repository_id: &RepositoryId) -> Result<StreamId, LinuxRepositoryRecoveryError> {
    StreamId::try_new("repository-recovery").map_err(|_| LinuxRepositoryRecoveryError::StateStale)
}

/// Projects the full mutation identity from any reconciliation terminal or ambiguity receipt.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    reason = "the closed reconciliation outcome is projected by one total match"
)]
fn reconciliation_identity(
    outcome: &RepositoryReconciliationOutcome,
) -> &RepositoryMutationIdentity {
    match outcome {
        RepositoryReconciliationOutcome::Applied(receipt)
        | RepositoryReconciliationOutcome::NotApplied(receipt)
        | RepositoryReconciliationOutcome::StillUnknown(receipt) => receipt.identity(),
    }
}

/// Runs the async store on an isolated current-thread runtime from the synchronous adapter port.
#[expect(
    clippy::implicit_return,
    clippy::map_err_ignore,
    clippy::question_mark_used,
    reason = "runtime construction and join failures collapse into unavailable recovery state"
)]
fn run_on_journal_runtime<Output, Operation, OperationFuture>(
    operation: Operation,
) -> Result<Output, LinuxRepositoryRecoveryError>
where
    Output: Send,
    Operation: FnOnce() -> OperationFuture + Send,
    OperationFuture: Future<Output = Result<Output, LinuxRepositoryRecoveryError>> + Send,
{
    thread::scope(|scope| {
        scope
            .spawn(move || {
                let runtime = RuntimeBuilder::new_current_thread()
                    .build()
                    .map_err(|_| LinuxRepositoryRecoveryError::StateUnavailable)?;
                runtime.block_on(operation())
            })
            .join()
            .map_err(|_| LinuxRepositoryRecoveryError::StateUnavailable)?
    })
}
