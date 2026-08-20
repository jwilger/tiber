//! Signed append and publication boundary for native Tiber facts.
//!
//! The reader module owns authority selection and validation. This module adds
//! only the complementary one-shot publication operation: stage named facts in
//! a disposable `eventcore-fs` store, sign one Git candidate, and publish it
//! to the fixed Tiber authority with an exact-base lease.

use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::{fmt, slice, str};
use std::{
    fs,
    path::{Path, PathBuf},
};

use eventcore::model::StreamIdentity as _;
use eventcore_fs::{FileEventStore, FsConfig, FsyncPolicy};
use eventcore_types::{Event, EventStore as _, EventStoreError, StreamId, StreamWrites};
use tempfile::TempDir;
use tiber_process_service::{ProcessPublication, ProcessStream};
use tiber_repository_service::RepositoryMutationPublication;
use tiber_session_service::{
    InferenceObservationPublication, InferenceRequestPublication, SessionFact,
    SessionStartPublication, SessionSuccessorPublication,
};
use tiber_tasks_service::{
    AcceptanceAddPublication, AcceptanceCheckPublication, DependencyLinkPublication,
    SubtaskIdCorrectionPublication, SubtaskOccurrenceCheckPublication, TaskAbandonmentPublication,
    TaskActivationPublication, TaskCompletionPublication, TaskCreationPublication,
    TaskDetailsPublication, TaskPriorityPublication, TaskReopeningPublication,
    TaskValidationPublication,
};
use tiber_workflow_core::TiberEffect;
use tiber_workflow_service::{
    WorkflowAdvancePublication, WorkflowEffectRequestPublication, WorkflowFact,
    WorkflowInitializationPublication, WorkflowObservationPublication,
};

use crate::{
    EVENT_STORE_DIRECTORY, EVENTS_DIRECTORY, GitOperation, GitStoreError, ResolvedAuthority,
    Retryability, TIBER_REF, TiberRevision, caller_worktree_top_level, git_path_is_self_resolving,
    inspect_event_history, materialize_publishable_revision, require_git_success,
    require_safe_event_store_layout, resolve_authority_revision, run_git,
    stream_version_from_catalog, verify_history_integrity, verify_signed_revision,
};

/// The exact signed revision confirmed by one native Tiber publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedRevision {
    /// The exact signed authority revision confirmed after publication.
    revision: TiberRevision,
}

impl PublishedRevision {
    /// Returns the exact newly authoritative signed revision.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed confirmed revision is clearest as the compact final accessor"
    )]
    pub const fn revision(&self) -> &TiberRevision {
        &self.revision
    }
}

/// Stable failures from staging or publishing one native Tiber transaction.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "publication failures are grouped by durable recovery semantics rather than alphabetical variant names"
)]
pub enum TiberPublicationError {
    /// The authoritative revision advanced before this decision acquired its publication stage.
    #[error("tiber_store_publication_authority_changed")]
    AuthorityChanged,
    /// A remote or local compare-and-swap rejected the candidate without overwriting authority.
    #[error("tiber_store_publication_conflict")]
    Conflict,
    /// A caller attempted to publish an empty event transaction.
    #[error("tiber_store_publication_empty")]
    Empty,
    /// A fact targeted a stream omitted from the command's consistency boundary.
    #[error("tiber_store_publication_undeclared_stream")]
    UndeclaredStream,
    /// Session and workflow tokens described different effects.
    #[error("tiber_store_publication_workflow_effect_mismatch")]
    WorkflowEffectMismatch,
    /// Publication may have reached the authority but could not be confirmed conclusively.
    #[error("tiber_store_publication_ambiguous")]
    Ambiguous,
    /// The signed authority, Git process boundary, or snapshot stage failed.
    #[error(transparent)]
    Store(#[from] GitStoreError),
    /// `eventcore-fs` rejected the declared append transaction.
    #[error(transparent)]
    EventStore(#[from] EventStoreError),
}

impl TiberPublicationError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed borrowed error-code mapping is clearest as one compact match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AuthorityChanged => "tiber_store_publication_authority_changed",
            Self::Conflict => "tiber_store_publication_conflict",
            Self::Empty => "tiber_store_publication_empty",
            Self::UndeclaredStream => "tiber_store_publication_undeclared_stream",
            Self::WorkflowEffectMismatch => "tiber_store_publication_workflow_effect_mismatch",
            Self::Ambiguous => "tiber_store_publication_ambiguous",
            Self::Store(error) => error.code(),
            Self::EventStore(_) => "tiber_store_publication_event_store_failed",
        }
    }

    /// Returns whether retrying the unchanged command may plausibly succeed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed publication recovery mapping remains explicit and compact"
    )]
    pub const fn retryability(&self) -> Retryability {
        match self {
            Self::Empty | Self::UndeclaredStream | Self::WorkflowEffectMismatch => {
                Retryability::Permanent
            }
            Self::Store(error) => error.retryability(),
            Self::EventStore(error) => retryability_for_event_store(error),
            Self::AuthorityChanged | Self::Conflict | Self::Ambiguous => Retryability::Retryable,
        }
    }
}

/// A one-shot signed append stage for the fixed Tiber authority.
///
/// The publisher is deliberately not an `EventStore` implementation. The
/// service supplies a closed modeled publication token. The publisher stages
/// exactly that token's fact and fixed stream-version fence once, then
/// confirms one signed authority revision.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the stage owns resources in publication-lifecycle order rather than alphabetically"
)]
pub struct TiberEventPublisher {
    /// Owns the disposable repository, work tree, index, and staged files.
    _temporary_directory: TempDir,
    /// The caller repository whose `origin` configuration names the authority.
    caller_repository: PathBuf,
    /// Exact revision from which this single publication decision was staged.
    base_revision: TiberRevision,
    /// Repository containing the selected base object and candidate commit.
    authority_repository: PathBuf,
    /// Caller execution root required by relative caller-local SSH commands.
    caller_execution_root: Option<PathBuf>,
    /// Disposable work tree containing only the committed `eventstore` subtree.
    snapshot: PathBuf,
    /// Disposable Git index representing the root Tiber authority tree.
    index: PathBuf,
    /// The append-capable `EventCore` stage.
    store: FileEventStore,
    /// Immutable source catalog used to derive exact expected stream versions.
    event_history: crate::EventHistoryCatalog,
    /// Remote URL when the fixed remote authority was selected.
    origin_url: Option<String>,
}

impl fmt::Debug for TiberEventPublisher {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the debug surface intentionally exposes only the durable base revision"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TiberEventPublisher")
            .field("base_revision", &self.base_revision)
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the publisher API follows stage opening, inspection, then one-shot append/publication flow"
)]
impl TiberEventPublisher {
    /// Publishes one closed process decision under its exact effect-owned stream fence.
    ///
    /// An idempotent modeled retry contains no events. It confirms and returns
    /// the pinned base revision without creating a Git candidate.
    ///
    /// # Errors
    ///
    /// Returns a typed failure before staging when the caller's exact stream
    /// fence differs from the closed publication, or when signed exact-base
    /// publication cannot be confirmed.
    #[inline]
    pub async fn publish_process(
        &mut self,
        expected_stream: &ProcessStream,
        publication: ProcessPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, [publication_stream]) = publication.into_events_and_consistency_streams();
        if &publication_stream != expected_stream {
            return Err(TiberPublicationError::UndeclaredStream);
        }
        let consistency_stream = publication_stream.as_stream_id().clone();
        if events.is_empty() {
            return self.confirm_base();
        }
        self.append(slice::from_ref(&consistency_stream), events)
            .await
    }

    /// Publishes one command-specific repository-mutation decision.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when `EventCore` staging, signing, or exact-base
    /// publication cannot be confirmed.
    #[inline]
    pub async fn publish_repository_mutation(
        &mut self,
        publication: RepositoryMutationPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, publication_streams) = publication.into_event_and_consistency_streams();
        let consistency_streams = publication_streams.map(|stream| stream.as_stream_id().clone());
        self.append(&consistency_streams, vec![event]).await
    }

    /// Atomically publishes one approved repository mutation and its prepared boundary.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when the closed publications target different
    /// streams or exact-base signed publication cannot be confirmed.
    #[inline]
    pub async fn publish_approved_and_prepared_repository_mutation(
        &mut self,
        [approval, prepared]: [RepositoryMutationPublication; 2],
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (approval_event, approval_streams) = approval.into_event_and_consistency_streams();
        let (prepared_event, prepared_streams) = prepared.into_event_and_consistency_streams();
        if approval_streams != prepared_streams {
            return Err(TiberPublicationError::UndeclaredStream);
        }
        let consistency_streams = approval_streams.map(|stream| stream.as_stream_id().clone());
        self.append(&consistency_streams, vec![approval_event, prepared_event])
            .await
    }

    /// Atomically publishes a prompt and its workflow-owned effect request.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when the closed publications disagree, `EventCore`
    /// staging fails, signing fails, or the candidate cannot be confirmed.
    #[inline]
    pub async fn publish_inference_request_with_workflow(
        &mut self,
        session: InferenceRequestPublication,
        initialization: WorkflowInitializationPublication,
        request: WorkflowEffectRequestPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (session_event, session_streams) = session.into_event_and_consistency_streams();
        let initialization_predecessor = initialization.predecessor_effect_id().cloned();
        let (initialized, workflow_streams) = initialization.into_event_and_consistency_streams();
        let (requested, request_streams) = request.into_event_and_consistency_streams();
        if workflow_streams.last() != request_streams.first() {
            return Err(TiberPublicationError::UndeclaredStream);
        }
        let SessionFact::InferenceRequested {
            effect: session_effect,
            predecessor_effect_id: session_predecessor,
            ..
        } = session_event.fact().clone()
        else {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        };
        let WorkflowFact::WorkflowInitialized { state } = initialized.fact().clone() else {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        };
        let WorkflowFact::EffectRequested {
            effect: TiberEffect::Infer(workflow_effect),
            ..
        } = requested.fact().clone()
        else {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        };
        if session_predecessor != initialization_predecessor
            || session_effect != workflow_effect
            || &session_effect != state.initial_effect()
        {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        }
        let session_writes = self.build_writes(&session_streams, vec![session_event])?;
        let _session = self.store.append_events(session_writes).await?;
        let workflow_writes = self.build_writes(&workflow_streams, vec![initialized, requested])?;
        let _workflow = self.store.append_events(workflow_writes).await?;
        let candidate = self.create_signed_candidate()?;
        self.publish_candidate(&candidate)
    }

    /// Atomically publishes assistant transcript and its workflow observation.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when the closed publications disagree, `EventCore`
    /// staging fails, signing fails, or the candidate cannot be confirmed.
    #[inline]
    #[expect(
        clippy::pattern_type_mismatch,
        clippy::wildcard_enum_match_arm,
        reason = "the publication boundary borrows the closed session resolution and rejects unrelated or future session facts"
    )]
    pub async fn publish_inference_observation_with_workflow(
        &mut self,
        session: InferenceObservationPublication,
        observation: WorkflowObservationPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (session_event, session_streams) = session.into_event_and_consistency_streams();
        let (workflow_event, workflow_streams) = observation.into_event_and_consistency_streams();
        let effect_id = match session_event.fact() {
            SessionFact::InferenceObserved { effect_id, .. } => effect_id,
            SessionFact::InferenceInterrupted {
                observation: interruption,
            } => interruption.effect_id(),
            _ => return Err(TiberPublicationError::WorkflowEffectMismatch),
        };
        let WorkflowFact::EffectObserved {
            observation: workflow_observation,
        } = workflow_event.fact().clone()
        else {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        };
        if effect_id != workflow_observation.effect_id() {
            return Err(TiberPublicationError::WorkflowEffectMismatch);
        }
        let session_writes = self.build_writes(&session_streams, vec![session_event])?;
        let _session = self.store.append_events(session_writes).await?;
        let workflow_writes = self.build_writes(&workflow_streams, vec![workflow_event])?;
        let _workflow = self.store.append_events(workflow_writes).await?;
        let candidate = self.create_signed_candidate()?;
        self.publish_candidate(&candidate)
    }

    /// Publishes the deterministic terminal workflow advance.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when `EventCore` staging fails, signing fails, or
    /// the candidate cannot be confirmed.
    #[inline]
    pub async fn publish_workflow_advance(
        &mut self,
        advance: WorkflowAdvancePublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, streams) = advance.into_event_and_consistency_streams();
        self.append(&streams, vec![event]).await
    }

    /// Publishes one modeled task-bound session start.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when signed authority changes,
    /// signing fails, or publication cannot be confirmed.
    #[inline]
    pub async fn publish_session_start(
        &mut self,
        publication: SessionStartPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes one modeled transfer to the successor task-bound session.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when `EventCore` staging fails, signing fails, or
    /// the candidate cannot be confirmed.
    #[inline]
    pub async fn publish_session_successor(
        &mut self,
        publication: SessionSuccessorPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes one modeled backlog creation and resulting strict board order.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when the task authority changes,
    /// signing fails, or the remote result cannot be safely reconciled.
    #[inline]
    pub async fn publish_task_creation(
        &mut self,
        publication: TaskCreationPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, consistency_streams) = publication.into_events_and_consistency_streams();
        return self.append(&consistency_streams, events).await;
    }

    /// Publishes one modeled replacement of a task's editable details.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when task authority changes,
    /// signing fails, or the remote result cannot be safely reconciled.
    #[inline]
    pub async fn publish_task_details(
        &mut self,
        publication: TaskDetailsPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, streams) = publication.into_event_and_consistency_streams();
        return self.append(&streams, vec![event]).await;
    }

    /// Publishes one modeled unchecked acceptance addition.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when task authority changes,
    /// signing fails, or the remote result cannot be safely reconciled.
    #[inline]
    pub async fn publish_acceptance_add(
        &mut self,
        publication: AcceptanceAddPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, streams) = publication.into_event_and_consistency_streams();
        return self.append(&streams, vec![event]).await;
    }

    /// Publishes one modeled reciprocal blocked-by dependency.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when task authority changes,
    /// signing fails, or the remote result cannot be safely reconciled.
    #[inline]
    pub async fn publish_dependency_link(
        &mut self,
        publication: DependencyLinkPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, streams) = publication.into_events_and_consistency_streams();
        return self.append(&streams, events).await;
    }

    /// Publishes one modeled strict board-priority movement.
    ///
    /// # Errors
    ///
    /// Returns the typed publication failure when board authority changes,
    /// signing fails, or the remote result cannot be safely reconciled.
    #[inline]
    pub async fn publish_task_priority(
        &mut self,
        publication: TaskPriorityPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, streams) = publication.into_event_and_consistency_streams();
        return self.append(&streams, vec![event]).await;
    }

    /// Publishes one modeled task abandonment and resulting board order.
    /// Publishes one modeled task abandonment batch.
    ///
    /// # Errors
    ///
    /// Returns a typed publication error when the modeled batch cannot be
    /// appended, committed, pushed, or reconciled against task authority.
    #[inline]
    pub async fn publish_task_abandonment(
        &mut self,
        publication: TaskAbandonmentPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, streams) = publication.into_events_and_consistency_streams();
        return self.append(&streams, events).await;
    }

    /// Publishes one checked deterministic task-board repair.
    ///
    /// # Errors
    ///
    /// Returns a typed publication error when the checked repair cannot be
    /// appended, committed, pushed, or reconciled against task authority.
    #[inline]
    pub async fn publish_task_validation(
        &mut self,
        publication: TaskValidationPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, streams) = publication.into_event_and_consistency_streams();
        self.append(&streams, vec![event]).await
    }

    /// Publishes one modeled abandoned-to-backlog reopening and board order.
    ///
    /// # Errors
    ///
    /// Returns a typed publication error when the modeled transition/order
    /// batch cannot be appended, committed, pushed, or reconciled against task
    /// authority.
    #[inline]
    pub async fn publish_task_reopening(
        &mut self,
        publication: TaskReopeningPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, streams) = publication.into_events_and_consistency_streams();
        self.append(&streams, events).await
    }

    /// Opens a publishable snapshot only if it still equals the already-read authority revision.
    ///
    /// # Errors
    ///
    /// Returns [`TiberPublicationError::AuthorityChanged`] when the authority
    /// advanced between a command's canonical read and its publication stage.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the staged authority boundary sequences fixed snapshot effects while preserving typed failures"
    )]
    #[inline]
    pub fn open_at(
        repository: &Path,
        expected_revision: &TiberRevision,
    ) -> Result<Self, TiberPublicationError> {
        let temporary_directory =
            TempDir::new().map_err(|_source| GitStoreError::Materialization)?;
        let (authority, origin_url) =
            resolve_publication_authority(repository, temporary_directory.path())?;
        if &authority.revision != expected_revision {
            return Err(TiberPublicationError::AuthorityChanged);
        }
        verify_signed_revision(&authority.repository, &authority.revision)?;

        let snapshot = temporary_directory.path().join("publication-snapshot");
        fs::create_dir_all(&snapshot).map_err(|_source| GitStoreError::Materialization)?;
        let index = temporary_directory.path().join("publication.index");
        let materialization_index = temporary_directory
            .path()
            .join("publication-materialization.index");
        materialize_publishable_revision(
            &authority.repository,
            &authority.revision,
            &snapshot,
            &index,
            &materialization_index,
        )?;
        require_safe_event_store_layout(&snapshot)?;
        let event_store_root = snapshot.join(EVENT_STORE_DIRECTORY);
        let store = FileEventStore::open_with_config(
            FsConfig::new(&event_store_root).with_fsync(FsyncPolicy::None),
        )
        .map_err(|_source| GitStoreError::EventHistory)?;
        let event_history = inspect_event_history(&event_store_root.join(EVENTS_DIRECTORY))?;
        verify_history_integrity(&store, &event_history)?;
        if origin_url.is_some() {
            copy_local_publication_identity(repository, &authority.repository)?;
        }

        Ok(Self {
            base_revision: authority.revision,
            authority_repository: authority.repository,
            caller_execution_root: authority.caller_execution_root,
            caller_repository: repository.to_path_buf(),
            event_history,
            index,
            origin_url,
            snapshot,
            store,
            _temporary_directory: temporary_directory,
        })
    }

    /// Returns the exact authority revision on which this one-shot stage is based.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed base revision is clearest as the compact final accessor"
    )]
    pub const fn base_revision(&self) -> &TiberRevision {
        &self.base_revision
    }

    /// Confirms that an empty modeled retry still names the current authority.
    fn confirm_base(&self) -> Result<PublishedRevision, TiberPublicationError> {
        let current = if let Some(origin_url) = self.origin_url.as_ref() {
            remote_authority_head(&self.caller_repository, origin_url)?
        } else {
            let output = run_git(
                &self.caller_repository,
                GitOperation::ResolveTiberRef,
                &["rev-parse", TIBER_REF],
                None,
                None,
            )?;
            require_git_success(&output, GitOperation::ResolveTiberRef)?;
            parse_publication_object_id(&output.stdout)?
        };
        if current != self.base_revision {
            return Err(TiberPublicationError::AuthorityChanged);
        }
        Ok(PublishedRevision { revision: current })
    }

    /// Publishes the only modeled fact accepted by the first native task-write slice.
    ///
    /// The opaque service token can represent only a checked acceptance fact
    /// with the exact board and addressed-task streams that fenced its pure
    /// command decision.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict without overwriting authority when a concurrent
    /// writer advances the fixed branch. Returns [`TiberPublicationError::Ambiguous`]
    /// if a transport interruption prevents conclusive publication confirmation;
    /// callers must reload the authority before retrying their idempotent intent.
    #[expect(
        clippy::implicit_return,
        reason = "the closed acceptance-check boundary retains its exact event, optimistic staging, signing, and publication steps"
    )]
    #[inline]
    pub async fn publish_acceptance_check(
        &mut self,
        publication: AcceptanceCheckPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes the only modeled fact accepted by the strict-next task-activation boundary.
    ///
    /// The opaque service token can represent only one unclaimed `InProgress`
    /// transition with the exact board and addressed-task streams that fenced
    /// its pure command decision. It cannot encode a general lifecycle
    /// transition or a generic mutable task batch.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict without overwriting authority when a concurrent
    /// writer advances the fixed branch. Returns [`TiberPublicationError::Ambiguous`]
    /// if a transport interruption prevents conclusive publication confirmation;
    /// callers must reload the authority before retrying their idempotent intent.
    #[expect(
        clippy::implicit_return,
        reason = "the closed task-activation boundary retains its exact event, optimistic staging, signing, and publication steps"
    )]
    #[inline]
    pub async fn publish_task_activation(
        &mut self,
        publication: TaskActivationPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes the only modeled fact accepted by the duplicate-subtask repair boundary.
    ///
    /// The opaque service token can represent only a preconditioned correction
    /// for one exact subtask occurrence and the board plus addressed-task
    /// streams whose versions fenced that pure decision.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict without overwriting authority when a concurrent
    /// writer advances the fixed branch. Returns [`TiberPublicationError::Ambiguous`]
    /// if a transport interruption prevents conclusive publication confirmation;
    /// callers must reload the authority before retrying their idempotent intent.
    #[expect(
        clippy::implicit_return,
        reason = "the closed correction boundary retains its exact event, optimistic staging, signing, and publication steps"
    )]
    #[inline]
    pub async fn publish_subtask_id_correction(
        &mut self,
        publication: SubtaskIdCorrectionPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes the only modeled fact accepted by the exact-subtask-check boundary.
    ///
    /// The opaque service token can represent only a checked occurrence with
    /// its complete unchecked preimage and the board plus addressed-task
    /// streams whose versions fenced its pure decision. It cannot become an
    /// identifier-based or generic subtask mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict without overwriting authority when a concurrent
    /// writer advances the fixed branch. Returns [`TiberPublicationError::Ambiguous`]
    /// if a transport interruption prevents conclusive publication confirmation;
    /// callers must reload the authority before retrying their idempotent intent.
    #[expect(
        clippy::implicit_return,
        reason = "the closed occurrence-check boundary retains its exact event, optimistic staging, signing, and publication steps"
    )]
    #[inline]
    pub async fn publish_subtask_occurrence_check(
        &mut self,
        publication: SubtaskOccurrenceCheckPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (event, consistency_streams) = publication.into_event_and_consistency_streams();
        self.append(&consistency_streams, vec![event]).await
    }

    /// Publishes the only modeled one-or-two-fact task-completion batch.
    ///
    /// The opaque service token permits a terminal `Done` transition with its
    /// strict board-order repair, or the order repair alone for a stale board
    /// entry left after an already-completed task. It cannot encode another
    /// lifecycle status or a generic mutable task batch.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict without overwriting authority when a concurrent
    /// writer advances the fixed branch. Returns [`TiberPublicationError::Ambiguous`]
    /// if a transport interruption prevents conclusive publication confirmation;
    /// callers must reload the authority before retrying their idempotent intent.
    #[expect(
        clippy::implicit_return,
        reason = "the closed completion boundary retains its exact modeled batch, optimistic staging, signing, and publication steps"
    )]
    #[inline]
    pub async fn publish_task_completion(
        &mut self,
        publication: TaskCompletionPublication,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let (events, consistency_streams) = publication.into_events_and_consistency_streams();
        self.append(&consistency_streams, events).await
    }

    /// Appends an already-closed internal event batch to the disposable stage.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the private staging helper retains explicit batch validation before the effectful append"
    )]
    async fn append<E: Event>(
        &mut self,
        consistency_streams: &[StreamId],
        events: Vec<E>,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        if events.is_empty() {
            return Err(TiberPublicationError::Empty);
        }
        let writes = self.build_writes(consistency_streams, events)?;
        let _appended = self.store.append_events(writes).await?;
        let candidate = self.create_signed_candidate()?;
        self.publish_candidate(&candidate)
    }

    /// Builds one atomic `EventCore` batch from the command's exact declared streams.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the named consistency boundary is kept as one direct builder before the effectful append"
    )]
    fn build_writes<E: Event>(
        &self,
        consistency_streams: &[StreamId],
        events: Vec<E>,
    ) -> Result<StreamWrites, TiberPublicationError> {
        let declared = consistency_streams
            .iter()
            .fold(BTreeMap::new(), |mut streams, stream| {
                let identifier: &str = stream.as_ref();
                let _previous = streams.insert(identifier.to_owned(), stream.clone());
                streams
            });
        if declared.is_empty() {
            return Err(TiberPublicationError::UndeclaredStream);
        }
        if events.iter().any(|event| {
            let identifier: &str = event.stream_id().as_ref();
            !declared.contains_key(identifier)
        }) {
            return Err(TiberPublicationError::UndeclaredStream);
        }
        let mut writes = StreamWrites::new();
        for stream in declared.into_values() {
            let version = stream_version_from_catalog(&self.event_history, &stream);
            writes = writes.register_stream(stream, version)?;
        }
        for event in events {
            writes = writes.append(event)?;
        }
        Ok(writes)
    }

    /// Stages only immutable event files in the original root-tree index and signs the candidate.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "candidate construction preserves the existing root index while adding only newly staged EventCore transaction files"
    )]
    fn create_signed_candidate(&self) -> Result<TiberRevision, TiberPublicationError> {
        let staged = run_git(
            &self.authority_repository,
            GitOperation::AppendTiberEvents,
            &["add", "-A", "eventstore/events"],
            Some(&self.snapshot),
            Some(&self.index),
        )?;
        require_git_success(&staged, GitOperation::AppendTiberEvents)?;
        let tree_output = run_git(
            &self.authority_repository,
            GitOperation::AppendTiberEvents,
            &["write-tree"],
            None,
            Some(&self.index),
        )?;
        require_git_success(&tree_output, GitOperation::AppendTiberEvents)?;
        let tree = parse_publication_object_id(&tree_output.stdout)?;
        let committed = run_git(
            &self.authority_repository,
            GitOperation::SignTiberCandidate,
            &[
                "commit-tree",
                "-S",
                tree.as_str(),
                "-p",
                self.base_revision.as_str(),
            ],
            None,
            None,
        )?;
        require_git_success(&committed, GitOperation::SignTiberCandidate)?;
        let candidate = parse_publication_object_id(&committed.stdout)?;
        verify_signed_revision(&self.authority_repository, &candidate)?;
        Ok(candidate)
    }

    /// Publishes one candidate with exact-base compare-and-swap semantics and confirms the fixed ref.
    #[expect(
        clippy::implicit_return,
        reason = "the fixed remote/local publication split preserves exact-base compare-and-swap semantics"
    )]
    fn publish_candidate(
        &self,
        candidate: &TiberRevision,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        if let Some(origin_url) = self.origin_url.as_ref() {
            return self.publish_remote_candidate(candidate, origin_url);
        }
        self.publish_local_candidate(candidate)
    }

    /// Publishes through the remote fixed authority with an exact base lease.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the remote confirmation branch keeps caller-relative SSH execution and exact advertised-ref verification explicit"
    )]
    fn publish_remote_candidate(
        &self,
        candidate: &TiberRevision,
        origin_url: &str,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let authority_git_dir = self
            .authority_repository
            .to_str()
            .map(|path| format!("--git-dir={path}"))
            .ok_or(GitStoreError::RefreshOriginTiberRef)?;
        let execution_root = self
            .caller_execution_root
            .as_deref()
            .ok_or(GitStoreError::RefreshOriginTiberRef)?;
        let lease = format!(
            "--force-with-lease={}:{}",
            crate::ORIGIN_TIBER_REF,
            self.base_revision.as_str()
        );
        let refspec = format!("{}:{}", candidate.as_str(), crate::ORIGIN_TIBER_REF);
        let pushed = run_git(
            execution_root,
            GitOperation::PublishTiberEvents,
            &[
                authority_git_dir.as_str(),
                "push",
                lease.as_str(),
                "origin",
                refspec.as_str(),
            ],
            None,
            None,
        )?;
        if pushed.status.success() {
            return self.confirm_remote_candidate(candidate, origin_url);
        }
        self.classify_failed_remote_publish(candidate, origin_url)
    }

    /// Confirms a successful lease-protected push reached the exact fixed remote ref.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the conclusive authority probe intentionally maps only an exact candidate head to publication success"
    )]
    fn confirm_remote_candidate(
        &self,
        candidate: &TiberRevision,
        origin_url: &str,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let head = remote_authority_head(&self.caller_repository, origin_url)?;
        publication_result_from_remote_head(None, candidate, &head)
    }

    /// Separates a visible concurrent advance from an otherwise ambiguous failed push.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the failure classifier avoids treating a suppressed Git exit as a safe retry without checking fixed authority state"
    )]
    fn classify_failed_remote_publish(
        &self,
        candidate: &TiberRevision,
        origin_url: &str,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let Some(head) =
            remote_authority_head_after_failed_publish(&self.caller_repository, origin_url)?
        else {
            return Err(TiberPublicationError::Conflict);
        };
        publication_result_from_remote_head(Some(&self.base_revision), candidate, &head)
    }

    /// Applies a local compare-and-swap only when no remote authority exists.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the local fixed-ref update supplies explicit compare-and-swap arguments instead of a forced update"
    )]
    fn publish_local_candidate(
        &self,
        candidate: &TiberRevision,
    ) -> Result<PublishedRevision, TiberPublicationError> {
        let updated = run_git(
            &self.caller_repository,
            GitOperation::PublishTiberEvents,
            &[
                "update-ref",
                crate::TIBER_REF,
                candidate.as_str(),
                self.base_revision.as_str(),
            ],
            None,
            None,
        )?;
        if updated.status.success() {
            return Ok(PublishedRevision {
                revision: candidate.clone(),
            });
        }
        Err(TiberPublicationError::Conflict)
    }
}

/// Classifies the exact fixed remote head after either publication outcome.
///
/// A successful push has no prior-head comparison available, while a failed
/// push can identify a visible concurrent advance from its exact base revision.
/// Both paths share the only safe outcome: return success only when the
/// advertised authority names the signed candidate.
#[expect(
    clippy::implicit_return,
    reason = "the exact remote-head mapping is a pure, deterministic publication decision"
)]
fn publication_result_from_remote_head(
    base_revision: Option<&TiberRevision>,
    candidate: &TiberRevision,
    head: &TiberRevision,
) -> Result<PublishedRevision, TiberPublicationError> {
    if head == candidate {
        return Ok(PublishedRevision {
            revision: candidate.clone(),
        });
    }
    if base_revision.is_some_and(|base| head != base) {
        return Err(TiberPublicationError::Conflict);
    }
    Err(TiberPublicationError::Ambiguous)
}

/// Classifies a direct `EventCore` append failure for unchanged-command recovery.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "only concurrency and backing-store availability can plausibly change on an unchanged native publication retry"
)]
const fn retryability_for_event_store(error: &EventStoreError) -> Retryability {
    match error {
        EventStoreError::StoreFailure { .. } | EventStoreError::VersionConflict { .. } => {
            Retryability::Retryable
        }
        EventStoreError::ConflictingExpectedVersions { .. }
        | EventStoreError::UndeclaredStream { .. }
        | EventStoreError::SerializationFailed { .. }
        | EventStoreError::DeserializationFailed { .. } => Retryability::Permanent,
    }
}

/// Copies the three repository-local commit identity settings required by `commit-tree -S`.
///
/// Remote publication creates its candidate in a disposable bare repository.
/// Global Git configuration remains inherited, but a caller's local identity
/// must cross that boundary so a normal signed commit can be created without
/// reading credentials or inventing an author identity.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the small fixed identity-key transfer preserves ordinary caller-local commit signing semantics in the disposable authority repository"
)]
fn copy_local_publication_identity(
    repository: &Path,
    authority_repository: &Path,
) -> Result<(), GitStoreError> {
    let mut caller_worktree_root = None;
    for key in ["user.name", "user.email", "user.signingkey"] {
        let caller_uses_ssh_signing = key.eq_ignore_ascii_case("user.signingkey")
            && caller_uses_ssh_signing_format(repository)?;
        let configuration = run_git(
            repository,
            GitOperation::PublishTiberEvents,
            &["config", "--null", "--local", "--get-all", key],
            None,
            None,
        )?;
        if configuration.status.code() == Some(crate::GIT_CONFIG_NOT_FOUND_EXIT) {
            continue;
        }
        require_git_success(&configuration, GitOperation::PublishTiberEvents)?;
        for raw_value in configuration
            .stdout
            .split(|byte| *byte == 0)
            .filter(|value| !value.is_empty())
        {
            let value =
                str::from_utf8(raw_value).map_err(|_source| GitStoreError::Materialization)?;
            let authority_value = publication_identity_value_for_authority(
                repository,
                key,
                value,
                caller_uses_ssh_signing,
                &mut caller_worktree_root,
            )?;
            let copied = run_git(
                authority_repository,
                GitOperation::PublishTiberEvents,
                &["config", "--local", "--add", key, authority_value.as_str()],
                None,
                None,
            )?;
            require_git_success(&copied, GitOperation::PublishTiberEvents)?;
        }
    }
    Ok(())
}

/// Reports whether the caller's effective Git signing format interprets `user.signingkey` as SSH.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the fixed effective-config lookup preserves Git's actual active signing-format selection"
)]
fn caller_uses_ssh_signing_format(repository: &Path) -> Result<bool, GitStoreError> {
    let configured = run_git(
        repository,
        GitOperation::SignTiberCandidate,
        &["config", "--get", "gpg.format"],
        None,
        None,
    )?;
    if configured.status.code() == Some(crate::GIT_CONFIG_NOT_FOUND_EXIT) {
        return Ok(false);
    }
    require_git_success(&configured, GitOperation::SignTiberCandidate)?;
    let format =
        str::from_utf8(&configured.stdout).map_err(|_source| GitStoreError::Materialization)?;
    Ok(format.trim_end().eq_ignore_ascii_case("ssh"))
}

/// Preserves a caller-relative SSH signing-key pathname across the disposable authority.
///
/// Git resolves `user.signingkey` pathnames from the repository that invokes
/// `commit-tree`. A remote publication signs in a disposable bare repository,
/// so an active SSH configuration rebases a relative value only when it names
/// a regular file in the caller worktree. This retains bare GPG key IDs, SSH
/// key literals, and command names under their normal Git interpretation.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the one-key pathname transfer preserves normal caller signing semantics without reinterpreting key IDs or commands"
)]
fn publication_identity_value_for_authority(
    repository: &Path,
    key: &str,
    value: &str,
    caller_uses_ssh_signing: bool,
    caller_worktree_root: &mut Option<PathBuf>,
) -> Result<String, GitStoreError> {
    if !key.eq_ignore_ascii_case("user.signingkey") || !caller_uses_ssh_signing {
        return Ok(value.to_owned());
    }
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() || git_path_is_self_resolving(value) {
        return Ok(value.to_owned());
    }
    if caller_worktree_root.is_none() {
        let worktree_root =
            caller_worktree_top_level(repository, GitOperation::SignTiberCandidate)?;
        *caller_worktree_root = Some(worktree_root);
    }
    let worktree_root = caller_worktree_root
        .as_deref()
        .ok_or(GitStoreError::Materialization)?;
    let rebased_path = worktree_root.join(path);
    if !rebased_path.is_file() {
        return Ok(value.to_owned());
    }
    let Some(rebased) = rebased_path.to_str() else {
        return Err(GitStoreError::Materialization);
    };
    Ok(rebased.to_owned())
}

/// Resolves publication authority exactly as the reader, retaining the remote URL when applicable.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the local-or-fixed-origin split is repeated narrowly here so publication can preserve the reader's immutable authority selection"
)]
fn resolve_publication_authority(
    repository: &Path,
    temporary_directory: &Path,
) -> Result<(ResolvedAuthority, Option<String>), TiberPublicationError> {
    let origin = run_git(
        repository,
        GitOperation::ResolveTiberRef,
        &["remote", "get-url", "origin"],
        None,
        None,
    )?;
    if origin.status.success() {
        let origin_url = crate::parse_origin_url(&origin.stdout)?;
        let disposable_origin_url =
            crate::origin_url_for_disposable_authority(repository, &origin_url)?;
        let authority = crate::resolve_remote_authority_revision(
            repository,
            temporary_directory,
            &disposable_origin_url,
        )?;
        return Ok((authority, Some(disposable_origin_url)));
    }
    if origin.status.code() == Some(crate::GIT_NOT_FOUND_EXIT) {
        let authority = resolve_authority_revision(repository, temporary_directory)?;
        return Ok((authority, None));
    }
    Err(crate::git_nonzero_error(GitOperation::ResolveTiberRef, &origin).into())
}

/// Parses a newline-terminated Git object name created by fixed publication plumbing.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the narrow publication parser maps malformed commit and tree IDs to one stable stage failure"
)]
fn parse_publication_object_id(output: &[u8]) -> Result<TiberRevision, TiberPublicationError> {
    TiberRevision::parse(
        str::from_utf8(output)
            .map_err(|_source| GitStoreError::Materialization)?
            .strip_suffix('\n')
            .ok_or(GitStoreError::Materialization)?,
    )
    .map_err(|_source| GitStoreError::Materialization.into())
}

/// Reads exactly the advertised fixed remote authority head after publication.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the successful-push confirmation keeps absent-ref failure distinct from the rejected-push conflict classifier"
)]
fn remote_authority_head(
    caller_repository: &Path,
    origin_url: &str,
) -> Result<TiberRevision, TiberPublicationError> {
    let output = run_git(
        caller_repository,
        GitOperation::PublishTiberEvents,
        &[
            "ls-remote",
            "--exit-code",
            origin_url,
            crate::ORIGIN_TIBER_REF,
        ],
        None,
        None,
    )?;
    require_git_success(&output, GitOperation::PublishTiberEvents)?;
    crate::parse_ls_remote_revision(&output.stdout).map_err(Into::into)
}

/// Reads the fixed remote authority after a rejected publication, preserving ref absence.
///
/// A failed exact-revision lease followed by a missing fixed ref is a conclusive
/// conflict: the authority changed from the command's base revision and the
/// rejected publication did not recreate it.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the rejected-push probe alone maps fixed-ref absence to conflict before applying the exact one-row authority parser"
)]
fn remote_authority_head_after_failed_publish(
    caller_repository: &Path,
    origin_url: &str,
) -> Result<Option<TiberRevision>, TiberPublicationError> {
    let output = run_git(
        caller_repository,
        GitOperation::PublishTiberEvents,
        &[
            "ls-remote",
            "--exit-code",
            origin_url,
            crate::ORIGIN_TIBER_REF,
        ],
        None,
        None,
    )?;
    if output.status.code() == Some(crate::GIT_NOT_FOUND_EXIT) {
        return Ok(None);
    }
    require_git_success(&output, GitOperation::PublishTiberEvents)?;
    crate::parse_ls_remote_revision(&output.stdout)
        .map(Some)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{TiberPublicationError, publication_result_from_remote_head};
    use crate::{Retryability, TiberRevision};

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the pure fixed-OID classifier fixture uses direct construction to prove its stable recovery contract"
    )]
    fn revision(value: &str) -> TiberRevision {
        TiberRevision::parse(value).expect("fixture object ID should parse")
    }

    #[expect(
        clippy::expect_used,
        reason = "the pure classifier regression requires the typed ambiguous result for its stable recovery assertions"
    )]
    #[test]
    fn reports_a_retryable_ambiguous_error_when_successful_push_confirmation_sees_another_head() {
        let candidate = revision("1111111111111111111111111111111111111111");
        let different_head = revision("2222222222222222222222222222222222222222");

        let error = publication_result_from_remote_head(None, &candidate, &different_head)
            .expect_err("a successful push without candidate confirmation must remain ambiguous");

        assert!(matches!(error, TiberPublicationError::Ambiguous));
        assert_eq!(error.code(), "tiber_store_publication_ambiguous");
        assert_eq!(error.retryability(), Retryability::Retryable);
    }

    #[expect(
        clippy::expect_used,
        reason = "the pure classifier regression requires the typed ambiguous result for its stable recovery assertions"
    )]
    #[test]
    fn reports_a_retryable_ambiguous_error_when_failed_push_leaves_the_base_head_visible() {
        let base = revision("3333333333333333333333333333333333333333");
        let candidate = revision("4444444444444444444444444444444444444444");

        let error = publication_result_from_remote_head(Some(&base), &candidate, &base)
            .expect_err("a failed push with only the base still visible must remain ambiguous");

        assert!(matches!(error, TiberPublicationError::Ambiguous));
        assert_eq!(error.code(), "tiber_store_publication_ambiguous");
        assert_eq!(error.retryability(), Retryability::Retryable);
    }
}
