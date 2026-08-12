//! Durable lifecycle state for structured Development System operations.
//!
//! Harness hooks never call this module.  A service invokes a transition only
//! after it has validated a capability assignment; this core consequently has
//! no tool-name, patch, shell, Git-verb, or redirect classifier.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use eventcore::{
    execute, mapping,
    model::{ModelCommandLogic, ModelState as _, Modeled, ModeledCommand, ModeledEvents},
    CommandError, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput, ModelState,
    RetryPolicy, StreamId, StreamIdentity,
};
use eventcore_sqlite::rusqlite::Connection;
use eventcore_types::EventStore;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tiber_git::git_event_store::{GitEventStore, GitEventStoreAuthority};

const STATE_DIRECTORY: &str = "development-system";
const STATE_FILE: &str = "workflow-events.sqlite";
const LEGACY_LIFECYCLE_STREAM: &str = "development-discipline:lifecycle";
const MAX_LEGACY_IMPORT_FACTS: usize = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ChangeKind {
    Production,
    Exempt,
}

impl ChangeKind {
    pub fn parse(value: &str) -> Result<Self, WorkflowError> {
        match value {
            "production" => Ok(Self::Production),
            "exempt" => Ok(Self::Exempt),
            _ => Err(WorkflowError::UnexpectedEvidence),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Phase {
    AwaitingRed,
    AwaitingImplementationAuthorization,
    Implementing,
    AwaitingVerification,
    Verifying,
    Reviewing,
    AwaitingDelivery,
    Delivering,
    Delivered,
    Abandoned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Workflow {
    phase: Phase,
    change_kind: ChangeKind,
    epoch: u64,
    red_cycles: u64,
    green_cycles: u64,
    verification_invalidated: bool,
    clean_review_observed: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InitialRepositoryState {
    pub(crate) index_tree: String,
    pub(crate) dirty_paths: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, StreamIdentity)]
pub(crate) struct WorkflowAuthorityStream(pub(crate) StreamId);

#[derive(ModelInput)]
struct StartWorkflowRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    change_kind: ChangeKind,
    #[model(origin)]
    initial_repository: InitialRepositoryState,
}

#[derive(ModelCommand)]
struct StartWorkflow {
    #[stream]
    stream: WorkflowAuthorityStream,
    change_kind: ChangeKind,
    initial_repository: InitialRepositoryState,
}

mapping! {
    StartWorkflowRequestToStream:
        StartWorkflowRequest.stream => StartWorkflow.stream
        using clone;
}

mapping! {
    StartWorkflowRequestToInitialRepository:
        StartWorkflowRequest.initial_repository => StartWorkflow.initial_repository
        using clone;
}

mapping! {
    StartWorkflowRequestToChangeKind:
        StartWorkflowRequest.change_kind => StartWorkflow.change_kind
        using clone;
}

mapping! {
    StartWorkflowStreamToEvent:
        StartWorkflow.stream => WorkflowAuthorityEvent.stream
        using workflow_authority_event_stream;
}

fn start_workflow_fact(
    stream: &WorkflowAuthorityStream,
    change_kind: &ChangeKind,
    initial_repository: &InitialRepositoryState,
    _: &bool,
) -> WorkflowAuthorityFact {
    let _ = stream;
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::WorkflowStarted {
        change_kind: *change_kind,
        initial_repository: initial_repository.clone(),
    })
}

mapping! {
    StartWorkflowToFact:
        (StartWorkflow.stream, StartWorkflow.change_kind, StartWorkflow.initial_repository, StartWorkflowState.active_workflow_exists) => WorkflowAuthorityEvent.fact
        using start_workflow_fact;
}

#[derive(ModelState)]
struct StartWorkflowState {
    #[model(default)]
    active_workflow_exists: bool,
}

#[derive(ModelInput)]
struct AcceptRedEvidenceRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt_id: String,
    #[model(origin)]
    checkpoint: crate::semantic::Checkpoint,
}

#[derive(ModelCommand)]
struct AcceptRedEvidence {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt_id: String,
    checkpoint: crate::semantic::Checkpoint,
}

mapping! {
    AcceptRedEvidenceRequestToStream:
        AcceptRedEvidenceRequest.stream => AcceptRedEvidence.stream
        using clone;
}
mapping! { AcceptRedEvidenceRequestToCheckpoint: AcceptRedEvidenceRequest.checkpoint => AcceptRedEvidence.checkpoint using clone; }

mapping! {
    AcceptRedEvidenceRequestToReceiptId:
        AcceptRedEvidenceRequest.receipt_id => AcceptRedEvidence.receipt_id
        using clone;
}
fn red_checkpoint_fact(
    checkpoint: &crate::semantic::Checkpoint,
    _: &u64,
    _: &Option<String>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::CheckpointCaptured {
        checkpoint: checkpoint.clone(),
    }
}
mapping! { AcceptRedEvidenceToCheckpointFact:
    (AcceptRedEvidence.checkpoint, AcceptRedEvidenceState.state_epoch, AcceptRedEvidenceState.last_checkpoint_id) => WorkflowAuthorityEvent.fact
    using red_checkpoint_fact;
}

mapping! {
    AcceptRedEvidenceStreamToEvent:
        AcceptRedEvidence.stream => WorkflowAuthorityEvent.stream
        using workflow_authority_event_stream;
}

#[expect(
    clippy::ptr_arg,
    reason = "EventCore mapping functions receive references to their declared String fields."
)]
fn accepted_red_evidence_fact(
    receipt_id: &String,
    _: &Option<crate::semantic::CommandReceipt>,
    _: &bool,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::RedEvidenceAccepted {
        receipt_id: receipt_id.clone(),
    }
}

mapping! {
    AcceptRedEvidenceToFact:
        (AcceptRedEvidence.receipt_id, AcceptRedEvidenceState.receipt, AcceptRedEvidenceState.receipt_accepted) => WorkflowAuthorityEvent.fact
        using accepted_red_evidence_fact;
}

fn red_phase_advanced_fact(_: &String, _: &Option<Phase>) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::RedEvidenceAccepted)
}

mapping! {
    AcceptRedEvidenceToLifecycleFact:
        (AcceptRedEvidence.receipt_id, AcceptRedEvidenceState.phase) => WorkflowAuthorityEvent.fact
        using red_phase_advanced_fact;
}

#[derive(ModelState)]
struct AcceptRedEvidenceState {
    #[model(default)]
    phase: Option<Phase>,
    #[model(default)]
    receipt: Option<crate::semantic::CommandReceipt>,
    #[model(default)]
    receipt_accepted: bool,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    last_checkpoint_id: Option<String>,
}

pub(crate) fn phase_after_lifecycle_fact(fact: &LifecycleFact) -> Phase {
    match fact {
        LifecycleFact::WorkflowStarted { change_kind, .. } => match change_kind {
            ChangeKind::Production => Phase::AwaitingRed,
            ChangeKind::Exempt => Phase::AwaitingVerification,
        },
        LifecycleFact::RedEvidenceAccepted => Phase::AwaitingImplementationAuthorization,
        LifecycleFact::ImplementationAuthorized => Phase::Implementing,
        LifecycleFact::GreenEvidenceAccepted => Phase::AwaitingVerification,
        LifecycleFact::VerificationStarted => Phase::Verifying,
        LifecycleFact::VerificationAccepted | LifecycleFact::ReviewStarted => Phase::Reviewing,
        LifecycleFact::CleanReviewAccepted | LifecycleFact::CleanReviewEvidenceAccepted { .. } => {
            Phase::AwaitingDelivery
        }
        LifecycleFact::DeliveryAuthorized => Phase::Delivering,
        LifecycleFact::DeliveryCompleted => Phase::Delivered,
        LifecycleFact::ReturnedToRed | LifecycleFact::CheckpointAbortApplied => Phase::AwaitingRed,
        LifecycleFact::WorkflowAbandoned => Phase::Abandoned,
    }
}

fn fold_phase(previous: Option<Phase>, fact: &WorkflowAuthorityFact) -> Option<Phase> {
    match fact {
        WorkflowAuthorityFact::Lifecycle(fact) => Some(phase_after_lifecycle_fact(fact)),
        WorkflowAuthorityFact::LegacyLifecycleHistoryImported { facts, .. } => facts
            .iter()
            .fold(previous, |_, fact| Some(phase_after_lifecycle_fact(fact))),
        WorkflowAuthorityFact::LegacySemanticHistoryImported { .. } => previous,
        WorkflowAuthorityFact::AssignmentIssued { .. }
        | WorkflowAuthorityFact::CommandReceiptRecorded { .. }
        | WorkflowAuthorityFact::CheckpointCaptured { .. }
        | WorkflowAuthorityFact::FileWriteAuthorized { .. }
        | WorkflowAuthorityFact::FileWritten { .. }
        | WorkflowAuthorityFact::FileDeleteAuthorized { .. }
        | WorkflowAuthorityFact::FileDeleted { .. }
        | WorkflowAuthorityFact::FileMoveAuthorized { .. }
        | WorkflowAuthorityFact::FileMoved { .. }
        | WorkflowAuthorityFact::CheckpointAbortAuthorized { .. }
        | WorkflowAuthorityFact::CheckpointAbortCompleted { .. }
        | WorkflowAuthorityFact::SignedCommitAuthorized { .. }
        | WorkflowAuthorityFact::SignedCommitCreated { .. }
        | WorkflowAuthorityFact::SignedTagAuthorized { .. }
        | WorkflowAuthorityFact::SignedTagCreated { .. }
        | WorkflowAuthorityFact::RemoteRefFetchAuthorized { .. }
        | WorkflowAuthorityFact::RemoteRefFetched { .. }
        | WorkflowAuthorityFact::RemoteRefPushAuthorized { .. }
        | WorkflowAuthorityFact::RemoteRefPushed { .. }
        | WorkflowAuthorityFact::PullRequestOpenAuthorized { .. }
        | WorkflowAuthorityFact::PullRequestOpened { .. }
        | WorkflowAuthorityFact::PullRequestUpdateAuthorized { .. }
        | WorkflowAuthorityFact::PullRequestUpdated { .. }
        | WorkflowAuthorityFact::PullRequestMergeAuthorized { .. }
        | WorkflowAuthorityFact::PullRequestMerged { .. }
        | WorkflowAuthorityFact::RedEvidenceAccepted { .. }
        | WorkflowAuthorityFact::GreenEvidenceAccepted { .. }
        | WorkflowAuthorityFact::VerificationEvidenceAccepted { .. } => previous,
    }
}

fn checkpoint_shape_is_valid(checkpoint: &crate::semantic::Checkpoint) -> bool {
    !checkpoint.id.is_empty()
        && matches!(checkpoint.index_tree.len(), 40 | 64)
        && checkpoint
            .index_tree
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && !checkpoint.command_policy_digest.is_empty()
}

impl ModelCommandLogic for AcceptRedEvidence {
    type Event = WorkflowAuthorityEvent;
    type State = AcceptRedEvidenceState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        state.state_epoch = match &event.fact {
            WorkflowAuthorityFact::Lifecycle(LifecycleFact::WorkflowStarted { .. }) => 1,
            WorkflowAuthorityFact::Lifecycle(_) => state.state_epoch.saturating_add(1),
            _ => state.state_epoch,
        };
        match &event.fact {
            WorkflowAuthorityFact::CommandReceiptRecorded { receipt } => {
                if receipt.id == self.receipt_id {
                    state.receipt = Some(receipt.clone());
                }
            }
            WorkflowAuthorityFact::RedEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::GreenEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::VerificationEvidenceAccepted { receipt_id } => {
                if receipt_id == &self.receipt_id {
                    state.receipt_accepted = true;
                }
            }
            WorkflowAuthorityFact::CheckpointCaptured { checkpoint } => {
                state.last_checkpoint_id = Some(checkpoint.id.clone());
            }
            WorkflowAuthorityFact::LegacyLifecycleHistoryImported { .. }
            | WorkflowAuthorityFact::LegacySemanticHistoryImported { .. }
            | WorkflowAuthorityFact::Lifecycle(_)
            | WorkflowAuthorityFact::AssignmentIssued { .. }
            | WorkflowAuthorityFact::FileWriteAuthorized { .. }
            | WorkflowAuthorityFact::FileWritten { .. }
            | WorkflowAuthorityFact::FileDeleteAuthorized { .. }
            | WorkflowAuthorityFact::FileDeleted { .. }
            | WorkflowAuthorityFact::FileMoveAuthorized { .. }
            | WorkflowAuthorityFact::FileMoved { .. } => {}
            WorkflowAuthorityFact::CheckpointAbortAuthorized { .. }
            | WorkflowAuthorityFact::CheckpointAbortCompleted { .. }
            | WorkflowAuthorityFact::SignedCommitAuthorized { .. }
            | WorkflowAuthorityFact::SignedCommitCreated { .. } => {}
            WorkflowAuthorityFact::SignedTagAuthorized { .. }
            | WorkflowAuthorityFact::SignedTagCreated { .. }
            | WorkflowAuthorityFact::RemoteRefFetchAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefFetched { .. } => {}
            WorkflowAuthorityFact::RemoteRefPushAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefPushed { .. }
            | WorkflowAuthorityFact::PullRequestOpenAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestOpened { .. }
            | WorkflowAuthorityFact::PullRequestUpdateAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestUpdated { .. }
            | WorkflowAuthorityFact::PullRequestMergeAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestMerged { .. } => {}
        }
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::AwaitingRed) {
            return Err(CommandError::ValidationError(
                "development_workflow.red_evidence_required".to_string(),
            ));
        }
        let receipt = state.as_ref().receipt.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.command_receipt_unknown".to_string())
        })?;
        if receipt.succeeded || state.as_ref().receipt_accepted {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_outcome_invalid".to_string(),
            ));
        }
        if !checkpoint_shape_is_valid(&self.checkpoint)
            || self.checkpoint.state_epoch != state.as_ref().state_epoch.saturating_add(1)
            || self.checkpoint.predecessor != state.as_ref().last_checkpoint_id
            || self.checkpoint.evidence_ids != [self.receipt_id.clone()].into_iter().collect()
        {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_invalid".to_string(),
            ));
        }
        let mut facts = ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptRedEvidenceStreamToEvent::apply(self))
                .fact(AcceptRedEvidenceToFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        facts.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptRedEvidenceStreamToEvent::apply(self))
                .fact(AcceptRedEvidenceToLifecycleFact::apply((
                    self,
                    state.as_ref(),
                )))
                .build(),
        );
        facts.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptRedEvidenceStreamToEvent::apply(self))
                .fact(AcceptRedEvidenceToCheckpointFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        Ok(facts)
    }
}

#[derive(ModelInput)]
struct AuthorizeImplementationRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
}
#[derive(ModelCommand)]
struct AuthorizeImplementation {
    #[stream]
    stream: WorkflowAuthorityStream,
}
mapping! { AuthorizeImplementationRequestToStream: AuthorizeImplementationRequest.stream => AuthorizeImplementation.stream using clone; }
mapping! { AuthorizeImplementationStreamToEvent: AuthorizeImplementation.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn implementation_authorized_fact(
    _: &WorkflowAuthorityStream,
    _: &Option<Phase>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::ImplementationAuthorized)
}
mapping! { AuthorizeImplementationToFact:
    (AuthorizeImplementation.stream, AuthorizeImplementationState.phase) => WorkflowAuthorityEvent.fact
    using implementation_authorized_fact;
}
#[derive(ModelState)]
struct AuthorizeImplementationState {
    #[model(default)]
    phase: Option<Phase>,
}
impl ModelCommandLogic for AuthorizeImplementation {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeImplementationState;
    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }
    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::AwaitingImplementationAuthorization) {
            return Err(CommandError::ValidationError(
                "development_workflow.red_evidence_required".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeImplementationStreamToEvent::apply(self))
                .fact(AuthorizeImplementationToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AcceptGreenEvidenceRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt_id: String,
    #[model(origin)]
    checkpoint: crate::semantic::Checkpoint,
}

#[derive(ModelCommand)]
struct AcceptGreenEvidence {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt_id: String,
    checkpoint: crate::semantic::Checkpoint,
}

mapping! {
    AcceptGreenEvidenceRequestToStream:
        AcceptGreenEvidenceRequest.stream => AcceptGreenEvidence.stream
        using clone;
}
mapping! { AcceptGreenEvidenceRequestToCheckpoint: AcceptGreenEvidenceRequest.checkpoint => AcceptGreenEvidence.checkpoint using clone; }

mapping! {
    AcceptGreenEvidenceRequestToReceiptId:
        AcceptGreenEvidenceRequest.receipt_id => AcceptGreenEvidence.receipt_id
        using clone;
}
fn green_checkpoint_fact(
    checkpoint: &crate::semantic::Checkpoint,
    _: &u64,
    _: &Option<String>,
    _: &std::collections::BTreeSet<String>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::CheckpointCaptured {
        checkpoint: checkpoint.clone(),
    }
}
mapping! { AcceptGreenEvidenceToCheckpointFact:
    (AcceptGreenEvidence.checkpoint, AcceptGreenEvidenceState.state_epoch, AcceptGreenEvidenceState.last_checkpoint_id, AcceptGreenEvidenceState.accepted_receipt_ids) => WorkflowAuthorityEvent.fact
    using green_checkpoint_fact;
}

mapping! {
    AcceptGreenEvidenceStreamToEvent:
        AcceptGreenEvidence.stream => WorkflowAuthorityEvent.stream
        using lifecycle_event_stream;
}

#[expect(
    clippy::ptr_arg,
    reason = "EventCore mapping functions receive references to their declared String fields."
)]
fn accepted_green_evidence_fact(
    receipt_id: &String,
    _: &Option<crate::semantic::CommandReceipt>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::GreenEvidenceAccepted {
        receipt_id: receipt_id.clone(),
    }
}

mapping! {
    AcceptGreenEvidenceToFact:
        (AcceptGreenEvidence.receipt_id, AcceptGreenEvidenceState.receipt) => WorkflowAuthorityEvent.fact
        using accepted_green_evidence_fact;
}

fn green_phase_advanced_fact(_: &String, _: &Option<Phase>) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::GreenEvidenceAccepted)
}

mapping! {
    AcceptGreenEvidenceToLifecycleFact:
        (AcceptGreenEvidence.receipt_id, AcceptGreenEvidenceState.phase) => WorkflowAuthorityEvent.fact
        using green_phase_advanced_fact;
}

/// This command decides only whether implementation is presently active. It
/// deliberately does not retain counters, review state, or the workflow
/// projection: those facts cannot affect accepting GREEN evidence.
#[derive(ModelState)]
struct AcceptGreenEvidenceState {
    #[model(default)]
    phase: Option<Phase>,
    #[model(default)]
    receipt: Option<crate::semantic::CommandReceipt>,
    #[model(default)]
    accepted_receipt_ids: std::collections::BTreeSet<String>,
    #[model(default)]
    state_epoch: u64,
    #[model(default)]
    last_checkpoint_id: Option<String>,
}

impl ModelCommandLogic for AcceptGreenEvidence {
    type Event = WorkflowAuthorityEvent;
    type State = AcceptGreenEvidenceState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        state.state_epoch = match &event.fact {
            WorkflowAuthorityFact::Lifecycle(LifecycleFact::WorkflowStarted { .. }) => 1,
            WorkflowAuthorityFact::Lifecycle(_) => state.state_epoch.saturating_add(1),
            _ => state.state_epoch,
        };
        match &event.fact {
            WorkflowAuthorityFact::CommandReceiptRecorded { receipt } => {
                if receipt.id == self.receipt_id {
                    state.receipt = Some(receipt.clone());
                }
            }
            WorkflowAuthorityFact::RedEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::GreenEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::VerificationEvidenceAccepted { receipt_id } => {
                state.accepted_receipt_ids.insert(receipt_id.clone());
            }
            WorkflowAuthorityFact::CheckpointCaptured { checkpoint } => {
                state.last_checkpoint_id = Some(checkpoint.id.clone());
            }
            WorkflowAuthorityFact::LegacyLifecycleHistoryImported { .. }
            | WorkflowAuthorityFact::LegacySemanticHistoryImported { .. }
            | WorkflowAuthorityFact::Lifecycle(_)
            | WorkflowAuthorityFact::AssignmentIssued { .. }
            | WorkflowAuthorityFact::FileWriteAuthorized { .. }
            | WorkflowAuthorityFact::FileWritten { .. }
            | WorkflowAuthorityFact::FileDeleteAuthorized { .. }
            | WorkflowAuthorityFact::FileDeleted { .. }
            | WorkflowAuthorityFact::FileMoveAuthorized { .. }
            | WorkflowAuthorityFact::FileMoved { .. } => {}
            WorkflowAuthorityFact::CheckpointAbortAuthorized { .. }
            | WorkflowAuthorityFact::CheckpointAbortCompleted { .. }
            | WorkflowAuthorityFact::SignedCommitAuthorized { .. }
            | WorkflowAuthorityFact::SignedCommitCreated { .. } => {}
            WorkflowAuthorityFact::SignedTagAuthorized { .. }
            | WorkflowAuthorityFact::SignedTagCreated { .. }
            | WorkflowAuthorityFact::RemoteRefFetchAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefFetched { .. } => {}
            WorkflowAuthorityFact::RemoteRefPushAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefPushed { .. }
            | WorkflowAuthorityFact::PullRequestOpenAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestOpened { .. }
            | WorkflowAuthorityFact::PullRequestUpdateAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestUpdated { .. }
            | WorkflowAuthorityFact::PullRequestMergeAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestMerged { .. } => {}
        }
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::Implementing) {
            return Err(CommandError::ValidationError(
                "development_workflow.implementation_required".to_string(),
            ));
        }
        let receipt = state.as_ref().receipt.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.command_receipt_unknown".to_string())
        })?;
        if !receipt.succeeded
            || state
                .as_ref()
                .accepted_receipt_ids
                .contains(&self.receipt_id)
        {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_outcome_invalid".to_string(),
            ));
        }
        let mut expected_evidence = state.as_ref().accepted_receipt_ids.clone();
        expected_evidence.insert(self.receipt_id.clone());
        if !checkpoint_shape_is_valid(&self.checkpoint)
            || self.checkpoint.state_epoch != state.as_ref().state_epoch.saturating_add(1)
            || self.checkpoint.predecessor != state.as_ref().last_checkpoint_id
            || self.checkpoint.evidence_ids != expected_evidence
        {
            return Err(CommandError::ValidationError(
                "development_system.checkpoint_invalid".to_string(),
            ));
        }
        let mut facts = ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptGreenEvidenceStreamToEvent::apply(self))
                .fact(AcceptGreenEvidenceToFact::apply((self, state.as_ref())))
                .build(),
        );
        facts.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptGreenEvidenceStreamToEvent::apply(self))
                .fact(AcceptGreenEvidenceToLifecycleFact::apply((
                    self,
                    state.as_ref(),
                )))
                .build(),
        );
        facts.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptGreenEvidenceStreamToEvent::apply(self))
                .fact(AcceptGreenEvidenceToCheckpointFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        Ok(facts)
    }
}

#[derive(ModelInput)]
struct BeginVerificationRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
}

#[derive(ModelCommand)]
struct BeginVerification {
    #[stream]
    stream: WorkflowAuthorityStream,
}

mapping! { BeginVerificationRequestToStream: BeginVerificationRequest.stream => BeginVerification.stream using clone; }
mapping! { BeginVerificationStreamToEvent: BeginVerification.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn verification_started_fact(
    _: &WorkflowAuthorityStream,
    _: &Option<Phase>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::VerificationStarted)
}
mapping! { BeginVerificationToFact:
    (BeginVerification.stream, BeginVerificationState.phase) => WorkflowAuthorityEvent.fact
    using verification_started_fact;
}

#[derive(ModelState)]
struct BeginVerificationState {
    #[model(default)]
    phase: Option<Phase>,
}

impl ModelCommandLogic for BeginVerification {
    type Event = WorkflowAuthorityEvent;
    type State = BeginVerificationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::AwaitingVerification) {
            return Err(CommandError::ValidationError(
                "development_workflow.verification_required".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(BeginVerificationStreamToEvent::apply(self))
                .fact(BeginVerificationToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AcceptVerificationRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    receipt_id: String,
}

#[derive(ModelCommand)]
struct AcceptVerification {
    #[stream]
    stream: WorkflowAuthorityStream,
    receipt_id: String,
}

mapping! { AcceptVerificationRequestToStream: AcceptVerificationRequest.stream => AcceptVerification.stream using clone; }
mapping! { AcceptVerificationRequestToReceiptId: AcceptVerificationRequest.receipt_id => AcceptVerification.receipt_id using clone; }
mapping! { AcceptVerificationStreamToEvent: AcceptVerification.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
#[expect(
    clippy::ptr_arg,
    reason = "EventCore mappings borrow declared String fields."
)]
fn verification_evidence_accepted_fact(
    receipt_id: &String,
    _: &Option<crate::semantic::CommandReceipt>,
    _: &bool,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::VerificationEvidenceAccepted {
        receipt_id: receipt_id.clone(),
    }
}
mapping! { AcceptVerificationToFact:
    (AcceptVerification.receipt_id, AcceptVerificationState.receipt, AcceptVerificationState.receipt_accepted) => WorkflowAuthorityEvent.fact
    using verification_evidence_accepted_fact;
}
fn verification_phase_advanced_fact(
    _: &String,
    _: &Option<Phase>,
    _: &bool,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::VerificationAccepted)
}
mapping! { AcceptVerificationToLifecycleFact:
    (AcceptVerification.receipt_id, AcceptVerificationState.phase, AcceptVerificationState.verification_invalidated) => WorkflowAuthorityEvent.fact
    using verification_phase_advanced_fact;
}

/// Verification acceptance needs no workflow projection. Only the live phase
/// and whether a mutation invalidated its evidence influence this decision.
#[derive(ModelState)]
struct AcceptVerificationState {
    #[model(default)]
    phase: Option<Phase>,
    #[model(default)]
    verification_invalidated: bool,
    #[model(default)]
    receipt: Option<crate::semantic::CommandReceipt>,
    #[model(default)]
    receipt_accepted: bool,
}

impl ModelCommandLogic for AcceptVerification {
    type Event = WorkflowAuthorityEvent;
    type State = AcceptVerificationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        if let WorkflowAuthorityFact::Lifecycle(fact) = &event.fact {
            match fact {
                LifecycleFact::WorkflowStarted { .. } => state.verification_invalidated = false,
                LifecycleFact::RedEvidenceAccepted
                | LifecycleFact::ReturnedToRed
                | LifecycleFact::CheckpointAbortApplied => state.verification_invalidated = true,
                LifecycleFact::GreenEvidenceAccepted => state.verification_invalidated = false,
                _ => {}
            }
        }
        match &event.fact {
            WorkflowAuthorityFact::CommandReceiptRecorded { receipt } => {
                if receipt.id == self.receipt_id {
                    state.receipt = Some(receipt.clone());
                }
            }
            WorkflowAuthorityFact::RedEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::GreenEvidenceAccepted { receipt_id }
            | WorkflowAuthorityFact::VerificationEvidenceAccepted { receipt_id } => {
                if receipt_id == &self.receipt_id {
                    state.receipt_accepted = true;
                }
            }
            WorkflowAuthorityFact::LegacyLifecycleHistoryImported { .. }
            | WorkflowAuthorityFact::LegacySemanticHistoryImported { .. }
            | WorkflowAuthorityFact::Lifecycle(_)
            | WorkflowAuthorityFact::AssignmentIssued { .. }
            | WorkflowAuthorityFact::CheckpointCaptured { .. }
            | WorkflowAuthorityFact::FileWriteAuthorized { .. }
            | WorkflowAuthorityFact::FileWritten { .. }
            | WorkflowAuthorityFact::FileDeleteAuthorized { .. }
            | WorkflowAuthorityFact::FileDeleted { .. }
            | WorkflowAuthorityFact::FileMoveAuthorized { .. }
            | WorkflowAuthorityFact::FileMoved { .. } => {}
            WorkflowAuthorityFact::CheckpointAbortAuthorized { .. }
            | WorkflowAuthorityFact::CheckpointAbortCompleted { .. }
            | WorkflowAuthorityFact::SignedCommitAuthorized { .. }
            | WorkflowAuthorityFact::SignedCommitCreated { .. } => {}
            WorkflowAuthorityFact::SignedTagAuthorized { .. }
            | WorkflowAuthorityFact::SignedTagCreated { .. }
            | WorkflowAuthorityFact::RemoteRefFetchAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefFetched { .. } => {}
            WorkflowAuthorityFact::RemoteRefPushAuthorized { .. }
            | WorkflowAuthorityFact::RemoteRefPushed { .. }
            | WorkflowAuthorityFact::PullRequestOpenAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestOpened { .. }
            | WorkflowAuthorityFact::PullRequestUpdateAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestUpdated { .. }
            | WorkflowAuthorityFact::PullRequestMergeAuthorized { .. }
            | WorkflowAuthorityFact::PullRequestMerged { .. } => {}
        }
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::Verifying) || state.as_ref().verification_invalidated
        {
            return Err(CommandError::ValidationError(
                "development_workflow.verification_required".to_string(),
            ));
        }
        let receipt = state.as_ref().receipt.as_ref().ok_or_else(|| {
            CommandError::ValidationError("development_system.command_receipt_unknown".to_string())
        })?;
        if !receipt.succeeded || state.as_ref().receipt_accepted {
            return Err(CommandError::ValidationError(
                "development_system.command_receipt_outcome_invalid".to_string(),
            ));
        }
        let mut facts = ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptVerificationStreamToEvent::apply(self))
                .fact(AcceptVerificationToFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        facts.push(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptVerificationStreamToEvent::apply(self))
                .fact(AcceptVerificationToLifecycleFact::apply((
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        );
        Ok(facts)
    }
}

#[derive(ModelInput)]
struct AcceptCleanReviewRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    evidence_id: String,
}

#[derive(ModelCommand)]
struct AcceptCleanReview {
    #[stream]
    stream: WorkflowAuthorityStream,
    evidence_id: String,
}

mapping! { AcceptCleanReviewRequestToStream: AcceptCleanReviewRequest.stream => AcceptCleanReview.stream using clone; }
mapping! { AcceptCleanReviewRequestToEvidenceId: AcceptCleanReviewRequest.evidence_id => AcceptCleanReview.evidence_id using clone; }
mapping! { AcceptCleanReviewStreamToEvent: AcceptCleanReview.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn clean_review_accepted_fact(evidence_id: &str, _: &Option<Phase>) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::CleanReviewEvidenceAccepted {
        evidence_id: evidence_id.to_string(),
    })
}
mapping! { AcceptCleanReviewToFact:
    (AcceptCleanReview.evidence_id, AcceptCleanReviewState.phase) => WorkflowAuthorityEvent.fact
    using clean_review_accepted_fact;
}

#[derive(ModelState)]
struct AcceptCleanReviewState {
    #[model(default)]
    phase: Option<Phase>,
}

impl ModelCommandLogic for AcceptCleanReview {
    type Event = WorkflowAuthorityEvent;
    type State = AcceptCleanReviewState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::Reviewing) {
            return Err(CommandError::ValidationError(
                "development_workflow.review_required".to_string(),
            ));
        }
        if self.evidence_id.trim().is_empty() {
            return Err(CommandError::ValidationError(
                "development_workflow.clean_review_evidence_required".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AcceptCleanReviewStreamToEvent::apply(self))
                .fact(AcceptCleanReviewToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AuthorizeDeliveryRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    remote_hold: bool,
}

/// The hold is a typed observation of Tiber's sole CI-recovery authority at
/// decision time, not a free-form action flag or a second workflow authority.
#[derive(ModelCommand)]
struct AuthorizeDelivery {
    #[stream]
    stream: WorkflowAuthorityStream,
    remote_hold: bool,
}

mapping! { AuthorizeDeliveryRequestToStream: AuthorizeDeliveryRequest.stream => AuthorizeDelivery.stream using clone; }
mapping! { AuthorizeDeliveryRequestToRemoteHold: AuthorizeDeliveryRequest.remote_hold => AuthorizeDelivery.remote_hold using clone; }
mapping! { AuthorizeDeliveryStreamToEvent: AuthorizeDelivery.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn delivery_authorized_fact(
    _: &WorkflowAuthorityStream,
    _: &bool,
    _: &Option<Phase>,
    _: &bool,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::DeliveryAuthorized)
}
mapping! { AuthorizeDeliveryToFact:
    (AuthorizeDelivery.stream, AuthorizeDelivery.remote_hold, AuthorizeDeliveryState.phase, AuthorizeDeliveryState.clean_review_observed) => WorkflowAuthorityEvent.fact
    using delivery_authorized_fact;
}

#[derive(ModelState)]
struct AuthorizeDeliveryState {
    #[model(default)]
    phase: Option<Phase>,
    #[model(default)]
    clean_review_observed: bool,
}

impl ModelCommandLogic for AuthorizeDelivery {
    type Event = WorkflowAuthorityEvent;
    type State = AuthorizeDeliveryState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        if let WorkflowAuthorityFact::Lifecycle(fact) = &event.fact {
            match fact {
                LifecycleFact::WorkflowStarted { .. }
                | LifecycleFact::RedEvidenceAccepted
                | LifecycleFact::ReturnedToRed
                | LifecycleFact::CheckpointAbortApplied => state.clean_review_observed = false,
                LifecycleFact::CleanReviewAccepted
                | LifecycleFact::CleanReviewEvidenceAccepted { .. } => {
                    state.clean_review_observed = true
                }
                _ => {}
            }
        }
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if self.remote_hold {
            return Err(CommandError::ValidationError(
                "tiber.ci_recovery_hold_active".to_string(),
            ));
        }
        if state.as_ref().phase != Some(Phase::AwaitingDelivery)
            || !state.as_ref().clean_review_observed
        {
            return Err(CommandError::ValidationError(
                "development_workflow.review_required".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AuthorizeDeliveryStreamToEvent::apply(self))
                .fact(AuthorizeDeliveryToFact::apply((
                    self,
                    self,
                    state.as_ref(),
                    state.as_ref(),
                )))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct CompleteDeliveryRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
}

#[derive(ModelCommand)]
struct CompleteDelivery {
    #[stream]
    stream: WorkflowAuthorityStream,
}

mapping! { CompleteDeliveryRequestToStream: CompleteDeliveryRequest.stream => CompleteDelivery.stream using clone; }
mapping! { CompleteDeliveryStreamToEvent: CompleteDelivery.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn delivery_completed_fact(
    _: &WorkflowAuthorityStream,
    _: &Option<Phase>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::DeliveryCompleted)
}
mapping! { CompleteDeliveryToFact:
    (CompleteDelivery.stream, CompleteDeliveryState.phase) => WorkflowAuthorityEvent.fact
    using delivery_completed_fact;
}

#[derive(ModelState)]
struct CompleteDeliveryState {
    #[model(default)]
    phase: Option<Phase>,
}

impl ModelCommandLogic for CompleteDelivery {
    type Event = WorkflowAuthorityEvent;
    type State = CompleteDeliveryState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().phase != Some(Phase::Delivering) {
            return Err(CommandError::ValidationError(
                "development_workflow.delivery_authorization_required".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(CompleteDeliveryStreamToEvent::apply(self))
                .fact(CompleteDeliveryToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct AbandonWorkflowRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
}

#[derive(ModelCommand)]
struct AbandonWorkflow {
    #[stream]
    stream: WorkflowAuthorityStream,
}

mapping! { AbandonWorkflowRequestToStream: AbandonWorkflowRequest.stream => AbandonWorkflow.stream using clone; }
mapping! { AbandonWorkflowStreamToEvent: AbandonWorkflow.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn workflow_abandoned_fact(
    _: &WorkflowAuthorityStream,
    _: &Option<Phase>,
) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::WorkflowAbandoned)
}
mapping! { AbandonWorkflowToFact:
    (AbandonWorkflow.stream, AbandonWorkflowState.phase) => WorkflowAuthorityEvent.fact
    using workflow_abandoned_fact;
}

#[derive(ModelState)]
struct AbandonWorkflowState {
    #[model(default)]
    phase: Option<Phase>,
}

impl ModelCommandLogic for AbandonWorkflow {
    type Event = WorkflowAuthorityEvent;
    type State = AbandonWorkflowState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if matches!(
            state.as_ref().phase,
            None | Some(Phase::Delivered | Phase::Abandoned)
        ) {
            return Err(CommandError::ValidationError(
                "development_workflow.unexpected_evidence".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(AbandonWorkflowStreamToEvent::apply(self))
                .fact(AbandonWorkflowToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

#[derive(ModelInput)]
struct ReturnToRedRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
}

#[derive(ModelCommand)]
struct ReturnToRed {
    #[stream]
    stream: WorkflowAuthorityStream,
}

mapping! { ReturnToRedRequestToStream: ReturnToRedRequest.stream => ReturnToRed.stream using clone; }
mapping! { ReturnToRedStreamToEvent: ReturnToRed.stream => WorkflowAuthorityEvent.stream using lifecycle_event_stream; }
fn returned_to_red_fact(_: &WorkflowAuthorityStream, _: &Option<Phase>) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::Lifecycle(LifecycleFact::ReturnedToRed)
}
mapping! { ReturnToRedToFact:
    (ReturnToRed.stream, ReturnToRedState.phase) => WorkflowAuthorityEvent.fact
    using returned_to_red_fact;
}

#[derive(ModelState)]
struct ReturnToRedState {
    #[model(default)]
    phase: Option<Phase>,
}

impl ModelCommandLogic for ReturnToRed {
    type Event = WorkflowAuthorityEvent;
    type State = ReturnToRedState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        state.phase = fold_phase(state.phase, &event.fact);
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if !matches!(
            state.as_ref().phase,
            Some(
                Phase::AwaitingVerification
                    | Phase::Verifying
                    | Phase::Reviewing
                    | Phase::AwaitingDelivery
            )
        ) {
            return Err(CommandError::ValidationError(
                "development_workflow.unexpected_evidence".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ReturnToRedStreamToEvent::apply(self))
                .fact(ReturnToRedToFact::apply((self, state.as_ref())))
                .build(),
        ))
    }
}

impl ModelCommandLogic for StartWorkflow {
    type Event = WorkflowAuthorityEvent;
    type State = StartWorkflowState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut state = state.into_inner();
        if let WorkflowAuthorityFact::Lifecycle(fact) = &event.fact {
            match fact {
                LifecycleFact::WorkflowStarted { .. } => state.active_workflow_exists = true,
                LifecycleFact::WorkflowAbandoned | LifecycleFact::DeliveryCompleted => {
                    state.active_workflow_exists = false
                }
                _ => {}
            }
        }
        Modeled::from_built(state)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if state.as_ref().active_workflow_exists {
            return Err(CommandError::ValidationError(
                "development_workflow.active_lifecycle_exists".to_string(),
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(StartWorkflowStreamToEvent::apply(self))
                .fact(StartWorkflowToFact::apply((
                    self,
                    self,
                    self,
                    state.as_ref(),
                )))
                .build(),
        ))
    }
}

/// Closed lifecycle domain intents. Harness tool names are parsed at the
/// boundary; EventCore commands never carry arbitrary transition strings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum WorkflowIntent {
    AcceptRedEvidence,
    AuthorizeImplementation,
    AcceptGreenEvidence,
    BeginVerification,
    AcceptVerification,
    AcceptCleanReview,
    AuthorizeDelivery,
    CompleteDelivery,
    ReturnToRed,
    AbortToCheckpoint,
    Abandon,
}

/// The single live Development Discipline authority stream.  Lifecycle and
/// capability facts share optimistic concurrency, so a command folds the
/// exact epoch and assignments it needs instead of observing another stream
/// before it executes.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum WorkflowAuthorityFact {
    /// The sole compatibility boundary for retired lifecycle histories. Fresh
    /// workflow commands cannot construct this variant; the named importer
    /// emits it after decoding the bounded historical vocabulary.
    LegacyLifecycleHistoryImported {
        source_id: String,
        facts: Vec<LifecycleFact>,
    },
    /// The equivalent typed boundary for the retired capability stream.
    /// Current capability commands emit its individual domain facts instead.
    LegacySemanticHistoryImported {
        source_id: String,
        facts: Vec<crate::semantic::LegacySemanticFact>,
    },
    Lifecycle(LifecycleFact),
    /// The failed named command whose observation established the next RED
    /// cycle. This is distinct from the phase fact so evidence remains
    /// inspectable without making historical lifecycle payloads incompatible.
    RedEvidenceAccepted {
        receipt_id: String,
    },
    /// The successful named command whose observation completed the current
    /// implementation cycle.
    GreenEvidenceAccepted {
        receipt_id: String,
    },
    VerificationEvidenceAccepted {
        receipt_id: String,
    },
    AssignmentIssued {
        assignment: crate::semantic::Assignment,
    },
    CommandReceiptRecorded {
        receipt: crate::semantic::CommandReceipt,
    },
    CheckpointCaptured {
        checkpoint: crate::semantic::Checkpoint,
    },
    FileWriteAuthorized {
        operation: crate::semantic::WorkspaceFileWrite,
    },
    FileWritten {
        operation: crate::semantic::WorkspaceFileWrite,
    },
    FileDeleteAuthorized {
        operation: crate::semantic::WorkspaceFileDeletion,
    },
    FileDeleted {
        operation: crate::semantic::WorkspaceFileDeletion,
    },
    FileMoveAuthorized {
        operation: crate::semantic::WorkspaceFileMove,
    },
    FileMoved {
        operation: crate::semantic::WorkspaceFileMove,
    },
    CheckpointAbortAuthorized {
        operation: crate::semantic::CheckpointAbortOperation,
    },
    CheckpointAbortCompleted {
        receipt: crate::semantic::CheckpointAbortReceipt,
    },
    SignedCommitAuthorized {
        operation: crate::semantic::SignedCommitOperation,
    },
    SignedCommitCreated {
        receipt: crate::semantic::SignedCommitReceipt,
    },
    SignedTagAuthorized {
        operation: crate::semantic::SignedTagOperation,
    },
    SignedTagCreated {
        receipt: crate::semantic::SignedTagReceipt,
    },
    RemoteRefFetchAuthorized {
        operation: crate::semantic::FetchRefOperation,
    },
    RemoteRefFetched {
        receipt: crate::semantic::FetchRefReceipt,
    },
    RemoteRefPushAuthorized {
        operation: crate::semantic::PushRefOperation,
    },
    RemoteRefPushed {
        receipt: crate::semantic::PushRefReceipt,
    },
    PullRequestOpenAuthorized {
        operation: crate::semantic::OpenPullRequestOperation,
    },
    PullRequestOpened {
        receipt: crate::semantic::OpenPullRequestReceipt,
    },
    PullRequestUpdateAuthorized {
        operation: crate::semantic::UpdatePullRequestOperation,
    },
    PullRequestUpdated {
        receipt: crate::semantic::UpdatePullRequestReceipt,
    },
    PullRequestMergeAuthorized {
        operation: crate::semantic::MergePullRequestOperation,
    },
    PullRequestMerged {
        receipt: crate::semantic::MergePullRequestReceipt,
    },
}

/// Typed compatibility input. It is deliberately limited to historical
/// lifecycle facts decoded by this module, rather than raw JSON or caller
/// supplied authority events.
#[derive(Clone, Debug)]
struct LegacyLifecycleImport {
    source_id: String,
    facts: Vec<LifecycleFact>,
}

#[derive(ModelInput)]
struct ImportLegacyLifecycleRequest {
    #[model(origin)]
    stream: WorkflowAuthorityStream,
    #[model(origin)]
    import: LegacyLifecycleImport,
}

#[derive(ModelCommand)]
struct ImportLegacyLifecycle {
    #[stream]
    stream: WorkflowAuthorityStream,
    import: LegacyLifecycleImport,
}

mapping! { ImportLegacyLifecycleRequestToStream: ImportLegacyLifecycleRequest.stream => ImportLegacyLifecycle.stream using clone; }
mapping! { ImportLegacyLifecycleRequestToImport: ImportLegacyLifecycleRequest.import => ImportLegacyLifecycle.import using clone; }
mapping! { ImportLegacyLifecycleStreamToEvent: ImportLegacyLifecycle.stream => WorkflowAuthorityEvent.stream using workflow_authority_event_stream; }

#[derive(ModelState)]
struct ImportLegacyLifecycleState {
    #[model(default)]
    imported_sources: BTreeSet<String>,
}

fn fold_imported_source(
    fact: &WorkflowAuthorityFact,
    previous: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut sources = previous.clone();
    if let WorkflowAuthorityFact::LegacyLifecycleHistoryImported { source_id, .. } = fact {
        sources.insert(source_id.clone());
    }
    sources
}

mapping! { ImportLegacyLifecycleEventToSources:
    (WorkflowAuthorityEvent.fact, previous(ImportLegacyLifecycleState.imported_sources)) => ImportLegacyLifecycleState.imported_sources
    using fold_imported_source;
}

fn legacy_lifecycle_history_imported_fact(import: &LegacyLifecycleImport) -> WorkflowAuthorityFact {
    WorkflowAuthorityFact::LegacyLifecycleHistoryImported {
        source_id: import.source_id.clone(),
        facts: import.facts.clone(),
    }
}
mapping! { ImportLegacyLifecycleToFact:
    ImportLegacyLifecycle.import => WorkflowAuthorityEvent.fact
    using legacy_lifecycle_history_imported_fact;
}

impl ModelCommandLogic for ImportLegacyLifecycle {
    type Event = WorkflowAuthorityEvent;
    type State = ImportLegacyLifecycleState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        ImportLegacyLifecycleState::model_builder()
            .imported_sources(ImportLegacyLifecycleEventToSources::apply((
                event,
                state.as_ref(),
            )))
            .build()
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        if self.import.source_id.is_empty() || self.import.facts.is_empty() {
            return Err(CommandError::ValidationError(
                "development_workflow.legacy_import_invalid".to_string(),
            ));
        }
        if self.import.facts.len() > MAX_LEGACY_IMPORT_FACTS {
            return Err(CommandError::ValidationError(
                "development_workflow.legacy_import_too_large".to_string(),
            ));
        }
        if state
            .as_ref()
            .imported_sources
            .contains(&self.import.source_id)
        {
            return Ok(ModeledEvents::none(
                "legacy lifecycle source already imported",
            ));
        }
        Ok(ModeledEvents::one(
            WorkflowAuthorityEvent::model_builder()
                .stream(ImportLegacyLifecycleStreamToEvent::apply(self))
                .fact(ImportLegacyLifecycleToFact::apply(self))
                .build(),
        ))
    }
}

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
pub(crate) struct WorkflowAuthorityEvent {
    pub(crate) stream: StreamId,
    pub(crate) fact: WorkflowAuthorityFact,
}

/// Read-only compatibility shape for rows written before lifecycle facts were
/// introduced. No command constructs this type; it is a one-way replay
/// boundary until existing local stores naturally advance with typed facts.
#[derive(Clone, Debug, Deserialize)]
struct LegacyLifecycleSnapshotEvent {
    workflow: Workflow,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PersistedLifecycleEvent {
    Current(LegacyLifecycleEvent),
    Legacy(LegacyLifecycleSnapshotEvent),
}

/// A persisted fact, never a post-transition workflow snapshot. The fact
/// names the result of an accepted intent, while the projection is rebuilt by
/// folding these facts in order.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum LifecycleFact {
    WorkflowStarted {
        change_kind: ChangeKind,
        #[serde(default)]
        initial_repository: InitialRepositoryState,
    },
    RedEvidenceAccepted,
    ImplementationAuthorized,
    GreenEvidenceAccepted,
    VerificationStarted,
    VerificationAccepted,
    ReviewStarted,
    /// Historical bare review acceptance. Fresh commands emit the evidence-bound fact below.
    CleanReviewAccepted,
    CleanReviewEvidenceAccepted {
        evidence_id: String,
    },
    DeliveryAuthorized,
    DeliveryCompleted,
    ReturnedToRed,
    CheckpointAbortApplied,
    WorkflowAbandoned,
}

/// Historical decoder shape. Fresh commands never construct this event type.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegacyLifecycleEvent {
    stream: StreamId,
    fact: LifecycleFact,
}

impl Event for LegacyLifecycleEvent {
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }

    fn event_type_name() -> &'static str {
        "DevelopmentDisciplineLifecycleEvent"
    }
}

impl Event for WorkflowAuthorityEvent {
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }

    fn event_type_name() -> &'static str {
        "DevelopmentDisciplineWorkflowAuthorityEvent"
    }
}

pub(crate) fn workflow_authority_event_stream(stream: &WorkflowAuthorityStream) -> StreamId {
    stream.0.clone()
}

fn lifecycle_event_stream(stream: &WorkflowAuthorityStream) -> StreamId {
    workflow_authority_event_stream(stream)
}

#[derive(ModelState)]
struct LifecycleProjection {
    #[model(default)]
    workflow: Option<Workflow>,
}

#[derive(ModelOutput)]
struct WorkflowEventRoutingOutput {
    stream: StreamId,
}

mapping! {
    WorkflowAuthorityEventStreamToRoutingOutput:
        WorkflowAuthorityEvent.stream => WorkflowEventRoutingOutput.stream
        using clone;
}

#[derive(ModelOutput)]
struct LifecycleStatusOutput {
    workflow: Option<Workflow>,
}

mapping! {
    LifecycleProjectionToStatusOutput:
        LifecycleProjection.workflow => LifecycleStatusOutput.workflow
        using clone;
}

fn observe_event_routing(event: &WorkflowAuthorityEvent) -> Modeled<WorkflowEventRoutingOutput> {
    WorkflowEventRoutingOutput::model_builder()
        .stream(WorkflowAuthorityEventStreamToRoutingOutput::apply(event))
        .build()
}

fn lifecycle_status_output(projection: &LifecycleProjection) -> Modeled<LifecycleStatusOutput> {
    LifecycleStatusOutput::model_builder()
        .workflow(LifecycleProjectionToStatusOutput::apply(projection))
        .build()
}

fn observe_initial_repository(project_root: &Path) -> Result<InitialRepositoryState, String> {
    let tree = Command::new("git")
        .args(["write-tree"])
        .current_dir(project_root)
        .output()
        .map_err(|error| format!("development_workflow.git_unavailable source={error}"))?;
    if !tree.status.success() {
        return Err("development_workflow.initial_index_unavailable=true".to_string());
    }
    let index_tree = String::from_utf8(tree.stdout)
        .map_err(|_| "development_workflow.initial_index_invalid=true".to_string())?
        .trim()
        .to_string();
    if !matches!(index_tree.len(), 40 | 64)
        || !index_tree.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("development_workflow.initial_index_invalid=true".to_string());
    }
    let status = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(project_root)
        .output()
        .map_err(|error| format!("development_workflow.git_unavailable source={error}"))?;
    if !status.status.success() {
        return Err("development_workflow.initial_status_unavailable=true".to_string());
    }
    let mut dirty_paths = std::collections::BTreeSet::new();
    let mut fields = status.stdout.split(|byte| *byte == 0).peekable();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        if field.len() < 4 || field[2] != b' ' {
            return Err("development_workflow.initial_status_invalid=true".to_string());
        }
        let path = std::str::from_utf8(&field[3..])
            .map_err(|_| "development_workflow.initial_status_invalid=true".to_string())?;
        dirty_paths.insert(path.to_string());
        if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
            let original = fields
                .next()
                .filter(|field| !field.is_empty())
                .ok_or_else(|| "development_workflow.initial_status_invalid=true".to_string())?;
            dirty_paths.insert(
                std::str::from_utf8(original)
                    .map_err(|_| "development_workflow.initial_status_invalid=true".to_string())?
                    .to_string(),
            );
        }
    }
    Ok(InitialRepositoryState {
        index_tree,
        dirty_paths,
    })
}

pub fn start_at(project_root: &Path, change_kind: ChangeKind) -> Result<Workflow, String> {
    let initial_repository = observe_initial_repository(project_root)?;
    let request = StartWorkflowRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .change_kind(change_kind)
        .initial_repository(initial_repository)
        .build();
    let command = StartWorkflow::model_builder()
        .stream(StartWorkflowRequestToStream::apply(request.as_ref()))
        .change_kind(StartWorkflowRequestToChangeKind::apply(request.as_ref()))
        .initial_repository(StartWorkflowRequestToInitialRepository::apply(
            request.as_ref(),
        ))
        .build();
    execute_start_workflow(project_root, command)?;
    status_at(project_root)
}

fn transition_for_intent_at(
    project_root: &Path,
    intent: WorkflowIntent,
) -> Result<Workflow, String> {
    if intent == WorkflowIntent::AuthorizeImplementation {
        let request = AuthorizeImplementationRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .build();
        let command = AuthorizeImplementation::model_builder()
            .stream(AuthorizeImplementationRequestToStream::apply(
                request.as_ref(),
            ))
            .build();
        execute_authorize_implementation(project_root, command)?;
        return status_at(project_root);
    }
    if intent == WorkflowIntent::BeginVerification {
        let request = BeginVerificationRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .build();
        let command = BeginVerification::model_builder()
            .stream(BeginVerificationRequestToStream::apply(request.as_ref()))
            .build();
        execute_begin_verification(project_root, command)?;
        return status_at(project_root);
    }
    if intent == WorkflowIntent::AuthorizeDelivery {
        let request = AuthorizeDeliveryRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .remote_hold(tiber_ci_recovery_hold(project_root)?)
            .build();
        let command = AuthorizeDelivery::model_builder()
            .stream(AuthorizeDeliveryRequestToStream::apply(request.as_ref()))
            .remote_hold(AuthorizeDeliveryRequestToRemoteHold::apply(
                request.as_ref(),
            ))
            .build();
        execute_authorize_delivery(project_root, command)?;
        return status_at(project_root);
    }
    if intent == WorkflowIntent::CompleteDelivery {
        let request = CompleteDeliveryRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .build();
        let command = CompleteDelivery::model_builder()
            .stream(CompleteDeliveryRequestToStream::apply(request.as_ref()))
            .build();
        execute_complete_delivery(project_root, command)?;
        return status_at(project_root);
    }
    if intent == WorkflowIntent::Abandon {
        let request = AbandonWorkflowRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .build();
        let command = AbandonWorkflow::model_builder()
            .stream(AbandonWorkflowRequestToStream::apply(request.as_ref()))
            .build();
        execute_abandon_workflow(project_root, command)?;
        return status_at(project_root);
    }
    if intent == WorkflowIntent::ReturnToRed {
        let request = ReturnToRedRequest::model_builder()
            .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
                project_root,
            )?))
            .build();
        let command = ReturnToRed::model_builder()
            .stream(ReturnToRedRequestToStream::apply(request.as_ref()))
            .build();
        execute_return_to_red(project_root, command)?;
        return status_at(project_root);
    }
    Err("development_workflow.named_lifecycle_command_missing".to_string())
}

pub(crate) fn accept_red_evidence_at(
    project_root: &Path,
    receipt_id: &str,
    checkpoint: crate::semantic::Checkpoint,
) -> Result<Workflow, String> {
    let request = AcceptRedEvidenceRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .receipt_id(receipt_id.to_string())
        .checkpoint(checkpoint)
        .build();
    let command = AcceptRedEvidence::model_builder()
        .stream(AcceptRedEvidenceRequestToStream::apply(request.as_ref()))
        .receipt_id(AcceptRedEvidenceRequestToReceiptId::apply(request.as_ref()))
        .checkpoint(AcceptRedEvidenceRequestToCheckpoint::apply(
            request.as_ref(),
        ))
        .build();
    execute_accept_red_evidence(project_root, command)?;
    status_at(project_root)
}

pub fn authorize_implementation_at(project_root: &Path) -> Result<Workflow, String> {
    transition_for_intent_at(project_root, WorkflowIntent::AuthorizeImplementation)
}

pub(crate) fn accept_green_evidence_at(
    project_root: &Path,
    receipt_id: &str,
    checkpoint: crate::semantic::Checkpoint,
) -> Result<Workflow, String> {
    let request = AcceptGreenEvidenceRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .receipt_id(receipt_id.to_string())
        .checkpoint(checkpoint)
        .build();
    let command = AcceptGreenEvidence::model_builder()
        .stream(AcceptGreenEvidenceRequestToStream::apply(request.as_ref()))
        .receipt_id(AcceptGreenEvidenceRequestToReceiptId::apply(
            request.as_ref(),
        ))
        .checkpoint(AcceptGreenEvidenceRequestToCheckpoint::apply(
            request.as_ref(),
        ))
        .build();
    execute_accept_green_evidence(project_root, command)?;
    status_at(project_root)
}

pub fn begin_verification_at(project_root: &Path) -> Result<Workflow, String> {
    transition_for_intent_at(project_root, WorkflowIntent::BeginVerification)
}

pub fn accept_verification_at(project_root: &Path, receipt_id: &str) -> Result<Workflow, String> {
    let request = AcceptVerificationRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .receipt_id(receipt_id.to_string())
        .build();
    let command = AcceptVerification::model_builder()
        .stream(AcceptVerificationRequestToStream::apply(request.as_ref()))
        .receipt_id(AcceptVerificationRequestToReceiptId::apply(
            request.as_ref(),
        ))
        .build();
    execute_accept_verification(project_root, command)?;
    status_at(project_root)
}

pub fn accept_clean_review_at(project_root: &Path, evidence_id: &str) -> Result<Workflow, String> {
    let request = AcceptCleanReviewRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .evidence_id(evidence_id.to_string())
        .build();
    let command = AcceptCleanReview::model_builder()
        .stream(AcceptCleanReviewRequestToStream::apply(request.as_ref()))
        .evidence_id(AcceptCleanReviewRequestToEvidenceId::apply(
            request.as_ref(),
        ))
        .build();
    execute_accept_clean_review(project_root, command)?;
    status_at(project_root)
}

pub fn authorize_delivery_at(project_root: &Path) -> Result<Workflow, String> {
    transition_for_intent_at(project_root, WorkflowIntent::AuthorizeDelivery)
}

pub fn complete_delivery_at(project_root: &Path) -> Result<Workflow, String> {
    transition_for_intent_at(project_root, WorkflowIntent::CompleteDelivery)
}

pub fn abandon_workflow_at(project_root: &Path) -> Result<Workflow, String> {
    transition_for_intent_at(project_root, WorkflowIntent::Abandon)
}

fn tiber_ci_recovery_hold(project_root: &Path) -> Result<bool, String> {
    tiber_git::ci_recovery_hold_at(project_root).map_err(|error| {
        format!("development_workflow.tiber_ci_recovery_read_failed source={error}")
    })
}

pub fn status_at(project_root: &Path) -> Result<Workflow, String> {
    read_lifecycle_projection(project_root)?
        .workflow
        .ok_or_else(|| "development_workflow.not_started".to_string())
}

/// The lifecycle fact fold is the authoritative epoch for every assignment,
/// receipt, and checkpoint decision. Capability code must not maintain a
/// second mutable epoch counter.
pub fn state_epoch_at(project_root: &Path) -> Result<u64, String> {
    Ok(status_at(project_root)?.epoch)
}

impl Workflow {
    #[must_use]
    pub fn start(change_kind: ChangeKind) -> Self {
        Self {
            phase: if change_kind == ChangeKind::Production {
                Phase::AwaitingRed
            } else {
                Phase::AwaitingVerification
            },
            change_kind,
            epoch: 1,
            red_cycles: 0,
            green_cycles: 0,
            verification_invalidated: false,
            clean_review_observed: false,
        }
    }

    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::AwaitingRed => "awaiting_red",
            Phase::AwaitingImplementationAuthorization => "awaiting_implementation_authorization",
            Phase::Implementing => "implementing",
            Phase::AwaitingVerification => "awaiting_verification",
            Phase::Verifying => "verifying",
            Phase::Reviewing => "reviewing",
            Phase::AwaitingDelivery => "awaiting_delivery",
            Phase::Delivering => "delivering",
            Phase::Delivered => "delivered",
            Phase::Abandoned => "abandoned",
        }
    }

    fn transition(&mut self, intent: WorkflowIntent) -> Result<(), WorkflowError> {
        match intent {
            WorkflowIntent::AcceptRedEvidence if self.phase == Phase::AwaitingRed => {
                self.red_cycles += 1;
                self.clean_review_observed = false;
                self.verification_invalidated = true;
                self.phase = Phase::AwaitingImplementationAuthorization;
            }
            WorkflowIntent::AuthorizeImplementation
                if self.phase == Phase::AwaitingImplementationAuthorization =>
            {
                self.phase = Phase::Implementing
            }
            WorkflowIntent::AcceptGreenEvidence if self.phase == Phase::Implementing => {
                self.green_cycles += 1;
                self.verification_invalidated = false;
                self.phase = Phase::AwaitingVerification;
            }
            WorkflowIntent::BeginVerification if self.phase == Phase::AwaitingVerification => {
                self.phase = Phase::Verifying
            }
            WorkflowIntent::AcceptVerification
                if self.phase == Phase::Verifying && !self.verification_invalidated =>
            {
                self.phase = Phase::Reviewing
            }
            WorkflowIntent::AcceptCleanReview if self.phase == Phase::Reviewing => {
                self.clean_review_observed = true;
                self.phase = Phase::AwaitingDelivery;
            }
            WorkflowIntent::AuthorizeDelivery
                if self.phase == Phase::AwaitingDelivery && self.clean_review_observed =>
            {
                self.phase = Phase::Delivering
            }
            WorkflowIntent::CompleteDelivery if self.phase == Phase::Delivering => {
                self.phase = Phase::Delivered
            }
            // A changed artifact starts a new RED cycle. This makes repeated
            // RED/GREEN cycles explicit and invalidates later evidence.
            WorkflowIntent::ReturnToRed
                if matches!(
                    self.phase,
                    Phase::AwaitingVerification
                        | Phase::Verifying
                        | Phase::Reviewing
                        | Phase::AwaitingDelivery
                ) =>
            {
                self.verification_invalidated = true;
                self.clean_review_observed = false;
                self.phase = Phase::AwaitingRed;
            }
            WorkflowIntent::AbortToCheckpoint if !self.is_terminal() => {
                self.verification_invalidated = true;
                self.clean_review_observed = false;
                self.phase = Phase::AwaitingRed;
            }
            WorkflowIntent::Abandon if !self.is_terminal() => self.phase = Phase::Abandoned,
            WorkflowIntent::AuthorizeImplementation => {
                return Err(WorkflowError::RedEvidenceRequired);
            }
            WorkflowIntent::AcceptGreenEvidence => {
                return Err(WorkflowError::ImplementationRequired);
            }
            WorkflowIntent::AcceptVerification => return Err(WorkflowError::VerificationRequired),
            WorkflowIntent::AcceptCleanReview => return Err(WorkflowError::ReviewRequired),
            WorkflowIntent::CompleteDelivery => {
                return Err(WorkflowError::DeliveryAuthorizationRequired);
            }
            _ => return Err(WorkflowError::UnexpectedEvidence),
        }
        self.epoch += 1;
        Ok(())
    }

    fn apply_fact(&mut self, fact: &LifecycleFact) {
        let intent = match fact {
            LifecycleFact::WorkflowStarted { .. } => unreachable!("start resets projection"),
            LifecycleFact::RedEvidenceAccepted => WorkflowIntent::AcceptRedEvidence,
            LifecycleFact::ImplementationAuthorized => WorkflowIntent::AuthorizeImplementation,
            LifecycleFact::GreenEvidenceAccepted => WorkflowIntent::AcceptGreenEvidence,
            LifecycleFact::VerificationStarted => WorkflowIntent::BeginVerification,
            LifecycleFact::VerificationAccepted => WorkflowIntent::AcceptVerification,
            // Historical stores can contain this retired fact. It is a
            // replay-only compatibility case, never emitted by a fresh
            // command, and means the review phase was entered.
            LifecycleFact::ReviewStarted => {
                self.phase = Phase::Reviewing;
                self.epoch += 1;
                return;
            }
            LifecycleFact::CleanReviewAccepted
            | LifecycleFact::CleanReviewEvidenceAccepted { .. } => {
                WorkflowIntent::AcceptCleanReview
            }
            LifecycleFact::DeliveryAuthorized => WorkflowIntent::AuthorizeDelivery,
            LifecycleFact::DeliveryCompleted => WorkflowIntent::CompleteDelivery,
            LifecycleFact::ReturnedToRed => WorkflowIntent::ReturnToRed,
            LifecycleFact::CheckpointAbortApplied => WorkflowIntent::AbortToCheckpoint,
            LifecycleFact::WorkflowAbandoned => WorkflowIntent::Abandon,
        };
        self.transition(intent)
            .expect("persisted lifecycle fact has already been decided");
    }

    fn is_terminal(&self) -> bool {
        matches!(self.phase, Phase::Delivered | Phase::Abandoned)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowError {
    RedEvidenceRequired,
    ImplementationRequired,
    VerificationRequired,
    ReviewRequired,
    DeliveryAuthorizationRequired,
    UnexpectedEvidence,
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RedEvidenceRequired => "development_workflow.red_evidence_required",
            Self::ImplementationRequired => "development_workflow.implementation_required",
            Self::VerificationRequired => "development_workflow.verification_required",
            Self::ReviewRequired => "development_workflow.review_required",
            Self::DeliveryAuthorizationRequired => {
                "development_workflow.delivery_authorization_required"
            }
            Self::UnexpectedEvidence => "development_workflow.unexpected_evidence",
        })
    }
}

fn workflow_state_path(project_root: &Path) -> Result<PathBuf, String> {
    Ok(common_git_directory(project_root)?
        .join(STATE_DIRECTORY)
        .join(STATE_FILE))
}

pub(crate) fn workflow_authority_stream_id(project_root: &Path) -> Result<StreamId, String> {
    StreamId::try_new(format!(
        "development-discipline:workflow-authority:{}",
        crate::semantic::content_digest(
            common_git_directory(project_root)?
                .to_string_lossy()
                .as_bytes(),
        )
    ))
    .map_err(|error| format!("development_workflow.stream_invalid source={error}"))
}

pub(crate) fn legacy_semantic_stream_id(project_root: &Path) -> Result<StreamId, String> {
    StreamId::try_new(format!(
        "development-discipline:workflow:{}",
        crate::semantic::content_digest(
            common_git_directory(project_root)?
                .to_string_lossy()
                .as_bytes(),
        )
    ))
    .map_err(|error| format!("development_workflow.stream_invalid source={error}"))
}

pub(crate) fn lifecycle_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: std::sync::LazyLock<Result<tokio::runtime::Runtime, String>> =
        std::sync::LazyLock::new(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|error| {
                    format!("development_workflow.event_runtime_failed source={error}")
                })
        });

    RUNTIME.as_ref().map_err(Clone::clone)
}

/// The fixed Git EventStore authority is the sole live lifecycle authority.
/// `workflow-events.sqlite` is read only below for compatibility with old
/// local installations; no current command opens it for mutation.
fn open_event_store(project_root: &Path) -> Result<GitEventStore, String> {
    GitEventStore::open_for_authority(project_root, GitEventStoreAuthority::DevelopmentWorkflow)
        .map_err(|error| format!("development_workflow.event_store_open_failed source={error}"))
}

/// Opens the fixed Git authority used by the non-test final-review command
/// executor. Test builds use an in-memory compatibility path instead.
#[cfg(not(test))]
pub(crate) fn open_development_workflow_event_store(
    project_root: &Path,
) -> Result<GitEventStore, String> {
    open_event_store(project_root)
}

/// Opens the fixed Git authority selected by the final-review adapter. The
/// plugin advisory authority is deliberately local-only and separate from the
/// remotely published native workflow authority.
#[cfg(not(test))]
pub(crate) fn open_final_review_event_store(
    project_root: &Path,
    authority: GitEventStoreAuthority,
) -> Result<GitEventStore, String> {
    GitEventStore::open_for_authority(project_root, authority)
        .map_err(|error| format!("final_review.event_store_open_failed source={error}"))
}

fn legacy_lifecycle_stream_id() -> Result<StreamId, String> {
    StreamId::try_new(LEGACY_LIFECYCLE_STREAM)
        .map_err(|error| format!("development_workflow.stream_invalid source={error}"))
}

/// Decode only the retired lifecycle vocabulary. This is the explicit
/// compatibility boundary between the former EventCore stream and the shared
/// workflow authority stream; ordinary commands never read or emit this type.
fn legacy_lifecycle_import_at(
    project_root: &Path,
) -> Result<Option<LegacyLifecycleImport>, String> {
    let store = open_event_store(project_root)?;
    let runtime = lifecycle_runtime()?;
    let legacy_stream = legacy_lifecycle_stream_id()?;
    let git_facts = runtime.block_on(async move {
        let mut events = store
            .read_stream::<LegacyLifecycleEvent>(legacy_stream)
            .await
            .map_err(|error| {
                format!("development_workflow.legacy_event_store_read_failed source={error}")
            })?;
        let mut facts = Vec::new();
        while let Some(event) = events.next().await {
            let event = event.map_err(|error| {
                format!("development_workflow.legacy_event_store_read_failed source={error}")
            })?;
            facts.push(event.fact);
            if facts.len() > MAX_LEGACY_IMPORT_FACTS {
                return Err("development_workflow.legacy_import_too_large".to_string());
            }
        }
        Ok::<_, String>(facts)
    })?;
    if !git_facts.is_empty() {
        let digest =
            crate::semantic::content_digest(&serde_json::to_vec(&git_facts).map_err(|error| {
                format!("development_workflow.legacy_import_encode_failed source={error}")
            })?);
        return Ok(Some(LegacyLifecycleImport {
            source_id: format!("git-lifecycle-v1:{digest}"),
            facts: git_facts,
        }));
    }

    let path = workflow_state_path(project_root)?;
    if !path.exists() {
        return Ok(None);
    }
    let connection = Connection::open(path).map_err(|error| {
        format!("development_workflow.legacy_event_store_read_failed source={error}")
    })?;
    let mut statement = connection
        .prepare(
            "SELECT event_data FROM eventcore_events WHERE stream_id = ?1 ORDER BY stream_version",
        )
        .map_err(|error| {
            format!("development_workflow.legacy_event_store_read_failed source={error}")
        })?;
    let rows = statement
        .query_map([LEGACY_LIFECYCLE_STREAM], |row| row.get::<_, String>(0))
        .map_err(|error| {
            format!("development_workflow.legacy_event_store_read_failed source={error}")
        })?;
    let mut facts = Vec::new();
    for row in rows {
        let encoded = row.map_err(|error| {
            format!("development_workflow.legacy_event_store_read_failed source={error}")
        })?;
        let event: PersistedLifecycleEvent = serde_json::from_str(&encoded)
            .map_err(|error| format!("development_workflow.event_decode_failed source={error}"))?;
        match event {
            PersistedLifecycleEvent::Current(event) => facts.push(event.fact),
            // A whole-state snapshot is historical data only. It remains
            // read-only and is intentionally not converted into a fresh fact.
            PersistedLifecycleEvent::Legacy(_) => {
                return Err(
                    "development_workflow.legacy_snapshot_manual_recovery_required".to_string(),
                );
            }
        }
        if facts.len() > MAX_LEGACY_IMPORT_FACTS {
            return Err("development_workflow.legacy_import_too_large".to_string());
        }
    }
    if facts.is_empty() {
        return Ok(None);
    }
    let digest = crate::semantic::content_digest(&serde_json::to_vec(&facts).map_err(|error| {
        format!("development_workflow.legacy_import_encode_failed source={error}")
    })?);
    Ok(Some(LegacyLifecycleImport {
        source_id: format!("sqlite-lifecycle-v1:{digest}"),
        facts,
    }))
}

/// Imports a retired lifecycle stream through a checked EventCore command
/// immediately before a mutation. Reads intentionally call neither this
/// function nor `execute`, so inspection remains non-mutating.
pub(crate) fn ensure_legacy_lifecycle_imported_at(project_root: &Path) -> Result<(), String> {
    let Some(import) = legacy_lifecycle_import_at(project_root)? else {
        return Ok(());
    };
    let request = ImportLegacyLifecycleRequest::model_builder()
        .stream(WorkflowAuthorityStream(workflow_authority_stream_id(
            project_root,
        )?))
        .import(import)
        .build();
    let command = ImportLegacyLifecycle::model_builder()
        .stream(ImportLegacyLifecycleRequestToStream::apply(
            request.as_ref(),
        ))
        .import(ImportLegacyLifecycleRequestToImport::apply(
            request.as_ref(),
        ))
        .build();
    let store = open_event_store(project_root)?;
    lifecycle_runtime()?
        .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
        .map(|_| ())
        .map_err(|error| format!("development_workflow.legacy_import_failed source={error}"))
}

macro_rules! execute_lifecycle_command {
    ($function:ident, $command:ty) => {
        fn $function(project_root: &Path, command: ModeledCommand<$command>) -> Result<(), String> {
            ensure_legacy_lifecycle_imported_at(project_root)?;
            let store = open_event_store(project_root)?;
            lifecycle_runtime()?
                .block_on(execute(store, command, RetryPolicy::new().max_retries(4)))
                .map(|_| ())
                .map_err(|error| {
                    format!("development_workflow.event_command_failed source={error}")
                })
        }
    };
}

execute_lifecycle_command!(execute_start_workflow, StartWorkflow);
execute_lifecycle_command!(execute_accept_red_evidence, AcceptRedEvidence);
execute_lifecycle_command!(execute_authorize_implementation, AuthorizeImplementation);
execute_lifecycle_command!(execute_accept_green_evidence, AcceptGreenEvidence);
execute_lifecycle_command!(execute_begin_verification, BeginVerification);
execute_lifecycle_command!(execute_accept_verification, AcceptVerification);
execute_lifecycle_command!(execute_accept_clean_review, AcceptCleanReview);
execute_lifecycle_command!(execute_authorize_delivery, AuthorizeDelivery);
execute_lifecycle_command!(execute_complete_delivery, CompleteDelivery);
execute_lifecycle_command!(execute_abandon_workflow, AbandonWorkflow);
execute_lifecycle_command!(execute_return_to_red, ReturnToRed);

fn fold_lifecycle_fact(
    projection: &mut LifecycleProjection,
    fact: &LifecycleFact,
) -> Result<(), String> {
    match fact {
        LifecycleFact::WorkflowStarted { change_kind, .. } => {
            projection.workflow = Some(Workflow::start(*change_kind));
        }
        fact => projection
            .workflow
            .as_mut()
            .ok_or_else(|| "development_workflow.fact_before_start".to_string())?
            .apply_fact(fact),
    }
    Ok(())
}

fn read_lifecycle_projection(project_root: &Path) -> Result<LifecycleProjection, String> {
    let store = open_event_store(project_root)?;
    let runtime = lifecycle_runtime()?;
    let stream = workflow_authority_stream_id(project_root)?;
    let (projection, events_found) = runtime.block_on(async move {
        let mut events = store
            .read_stream::<WorkflowAuthorityEvent>(stream)
            .await
            .map_err(|error| {
                format!("development_workflow.event_store_read_failed source={error}")
            })?;
        let mut projection = LifecycleProjection::initial().into_inner();
        let mut events_found = false;
        while let Some(event) = events.next().await {
            let event = event.map_err(|error| {
                format!("development_workflow.event_store_read_failed source={error}")
            })?;
            let routing = observe_event_routing(&event);
            debug_assert_eq!(&routing.as_ref().stream, event.stream_id());
            events_found = true;
            match &event.fact {
                WorkflowAuthorityFact::Lifecycle(fact) => {
                    fold_lifecycle_fact(&mut projection, fact)?;
                }
                WorkflowAuthorityFact::LegacyLifecycleHistoryImported { facts, .. } => {
                    for fact in facts {
                        fold_lifecycle_fact(&mut projection, fact)?;
                    }
                }
                _ => {}
            }
        }
        Ok::<_, String>((projection, events_found))
    })?;
    if events_found {
        let status = lifecycle_status_output(&projection).into_inner();
        return Ok(LifecycleProjection {
            workflow: status.workflow,
        });
    }

    // Legacy SQLite rows are a read-only compatibility source. Fresh EventCore
    // commands never append there, and a later explicit migration will import
    // this projection as a bounded legacy fact before enabling mutation.
    let path = workflow_state_path(project_root)?;
    if !path.exists() {
        return Ok(projection);
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("development_workflow.event_store_read_failed source={error}"))?;
    let mut statement = connection
        .prepare(
            "SELECT event_data FROM eventcore_events WHERE stream_id = ?1 ORDER BY stream_version",
        )
        .map_err(|error| format!("development_workflow.event_store_read_failed source={error}"))?;
    let rows = statement
        .query_map([LEGACY_LIFECYCLE_STREAM], |row| row.get::<_, String>(0))
        .map_err(|error| format!("development_workflow.event_store_read_failed source={error}"))?;
    let mut projection = LifecycleProjection::initial().into_inner();
    for row in rows {
        let encoded = row.map_err(|error| {
            format!("development_workflow.event_store_read_failed source={error}")
        })?;
        let event: PersistedLifecycleEvent = serde_json::from_str(&encoded)
            .map_err(|error| format!("development_workflow.event_decode_failed source={error}"))?;
        match event {
            PersistedLifecycleEvent::Legacy(legacy) => projection.workflow = Some(legacy.workflow),
            PersistedLifecycleEvent::Current(event) => {
                fold_lifecycle_fact(&mut projection, &event.fact)?;
            }
        }
    }
    let status = lifecycle_status_output(&projection).into_inner();
    Ok(LifecycleProjection {
        workflow: status.workflow,
    })
}

fn common_git_directory(cwd: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("development_workflow.git_unavailable source={error}"))?;
    if !output.status.success() {
        return Err("development_workflow.git_repository_required=true".to_string());
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|_| "development_workflow.git_path_invalid=true".to_string())?;
    let path = PathBuf::from(path.trim());
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lifecycle_operations_share_one_process_runtime() {
        let first = lifecycle_runtime().expect("first lifecycle runtime");
        let second = lifecycle_runtime().expect("second lifecycle runtime");

        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn modeled_lifecycle_has_complete_provenance_without_assumptions() {
        let report = eventcore::model::check().expect("complete EventCore workflow model");
        assert_eq!(report.status, eventcore::model::CheckStatus::Verified);
        assert!(
            report.warnings.is_empty(),
            "verified workflow model still has unconsumed provenance: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn lifecycle_persistence_contains_a_fact_not_a_workflow_snapshot() {
        let event = WorkflowAuthorityEvent {
            stream: StreamId::try_new("development-discipline:workflow-authority:test")
                .expect("valid workflow authority stream"),
            fact: WorkflowAuthorityFact::Lifecycle(LifecycleFact::RedEvidenceAccepted),
        };
        let encoded = serde_json::to_value(event).expect("encode lifecycle fact");
        assert_eq!(
            encoded["fact"],
            serde_json::json!({ "Lifecycle": "RedEvidenceAccepted" })
        );
        assert!(encoded.get("workflow").is_none());

        let review = WorkflowAuthorityEvent {
            stream: StreamId::try_new("development-discipline:workflow-authority:test")
                .expect("valid workflow authority stream"),
            fact: WorkflowAuthorityFact::Lifecycle(LifecycleFact::CleanReviewEvidenceAccepted {
                evidence_id: "review-state-fingerprint".to_string(),
            }),
        };
        let encoded = serde_json::to_value(review).expect("encode clean-review fact");
        assert_eq!(
            encoded["fact"],
            serde_json::json!({
                "Lifecycle": {
                    "CleanReviewEvidenceAccepted": {
                        "evidence_id": "review-state-fingerprint"
                    }
                }
            })
        );
    }

    #[test]
    fn legacy_lifecycle_snapshot_is_decode_only_compatibility() {
        let workflow = Workflow::start(ChangeKind::Production);
        let encoded = serde_json::json!({ "workflow": workflow });
        assert!(matches!(
            serde_json::from_value::<PersistedLifecycleEvent>(encoded)
                .expect("decode legacy snapshot"),
            PersistedLifecycleEvent::Legacy(_)
        ));
    }

    #[test]
    fn historical_bare_clean_review_fact_remains_replayable() {
        let encoded = serde_json::json!({
            "stream": "development-discipline:workflow-authority:test",
            "fact": { "Lifecycle": "CleanReviewAccepted" }
        });
        let decoded = serde_json::from_value::<WorkflowAuthorityEvent>(encoded)
            .expect("decode historical clean-review fact");

        assert!(matches!(
            decoded.fact,
            WorkflowAuthorityFact::Lifecycle(LifecycleFact::CleanReviewAccepted)
        ));
    }

    #[test]
    fn typed_legacy_lifecycle_import_is_idempotent_and_replays_on_the_shared_stream() {
        let repository = TempDir::new().expect("temporary repository");
        let status = Command::new("git")
            .args([
                "init",
                "--quiet",
                repository.path().to_str().expect("repository path"),
            ])
            .status()
            .expect("git init");
        assert!(status.success());

        let import = LegacyLifecycleImport {
            source_id: "fixture-lifecycle:9e3779b9".to_string(),
            facts: vec![
                LifecycleFact::WorkflowStarted {
                    change_kind: ChangeKind::Production,
                    initial_repository: InitialRepositoryState::default(),
                },
                LifecycleFact::RedEvidenceAccepted,
            ],
        };
        let request = ImportLegacyLifecycleRequest::model_builder()
            .stream(WorkflowAuthorityStream(
                workflow_authority_stream_id(repository.path()).expect("stream"),
            ))
            .import(import)
            .build();
        let command = ImportLegacyLifecycle::model_builder()
            .stream(ImportLegacyLifecycleRequestToStream::apply(
                request.as_ref(),
            ))
            .import(ImportLegacyLifecycleRequestToImport::apply(
                request.as_ref(),
            ))
            .build();
        let store = open_event_store(repository.path()).expect("event store");
        lifecycle_runtime()
            .expect("runtime")
            .block_on(execute(store, command, RetryPolicy::new().max_retries(1)))
            .expect("import legacy lifecycle");

        let status = status_at(repository.path()).expect("replayed status");
        assert_eq!(status.phase_name(), "awaiting_implementation_authorization");

        let request = ImportLegacyLifecycleRequest::model_builder()
            .stream(WorkflowAuthorityStream(
                workflow_authority_stream_id(repository.path()).expect("stream"),
            ))
            .import(LegacyLifecycleImport {
                source_id: "fixture-lifecycle:9e3779b9".to_string(),
                facts: vec![LifecycleFact::WorkflowStarted {
                    change_kind: ChangeKind::Production,
                    initial_repository: InitialRepositoryState::default(),
                }],
            })
            .build();
        let command = ImportLegacyLifecycle::model_builder()
            .stream(ImportLegacyLifecycleRequestToStream::apply(
                request.as_ref(),
            ))
            .import(ImportLegacyLifecycleRequestToImport::apply(
                request.as_ref(),
            ))
            .build();
        let store = open_event_store(repository.path()).expect("event store");
        lifecycle_runtime()
            .expect("runtime")
            .block_on(execute(store, command, RetryPolicy::new().max_retries(1)))
            .expect("idempotent import");
        assert_eq!(
            status_at(repository.path())
                .expect("replayed status")
                .phase_name(),
            "awaiting_implementation_authorization"
        );
    }

    #[test]
    fn status_is_read_only_before_a_workflow_exists() {
        let repository = TempDir::new().expect("temporary repository");
        let status = Command::new("git")
            .args([
                "init",
                "--quiet",
                repository.path().to_str().expect("repository path"),
            ])
            .status()
            .expect("git init");
        assert!(status.success());

        assert!(matches!(
            status_at(repository.path()),
            Err(error) if error == "development_workflow.not_started"
        ));
        assert!(!repository.path().join(".git/development-system").exists());
    }

    #[test]
    fn production_supports_repeated_red_green_cycles_and_invalidation() {
        let mut workflow = Workflow::start(ChangeKind::Production);
        for _ in 0..2 {
            workflow
                .transition(WorkflowIntent::AcceptRedEvidence)
                .expect("red");
            workflow
                .transition(WorkflowIntent::AuthorizeImplementation)
                .expect("implement");
            workflow
                .transition(WorkflowIntent::AcceptGreenEvidence)
                .expect("green");
            workflow
                .transition(WorkflowIntent::BeginVerification)
                .expect("verify");
            workflow
                .transition(WorkflowIntent::AcceptVerification)
                .expect("verified");
            if workflow.green_cycles == 1 {
                workflow
                    .transition(WorkflowIntent::ReturnToRed)
                    .expect("invalidate");
            }
        }
        workflow
            .transition(WorkflowIntent::AcceptCleanReview)
            .expect("review");
        assert_eq!(workflow.phase_name(), "awaiting_delivery");
    }
}
