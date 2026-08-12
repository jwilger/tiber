//! Tiber's authoritative domain-event vocabulary.
//!
//! The model has four semantic stream families:
//! `tiber:repository` records format initialization, `tiber:board` owns
//! lifecycle membership and strict backlog order, `tiber:task:<id>` owns one
//! task's details and history, and `tiber:ci-recovery` owns the repository-wide
//! CI incident. Commands that affect more than one family emit one atomic
//! multi-stream append. Task and CI events are intentionally in the same enum
//! because they share one EventCore store and the single `tiber` Git branch.
//!
//! Every mutating behavior has a named event below. Projections fold these
//! events into typed task, board, and recovery state; no Markdown snapshot is
//! authoritative. Adding a mutator therefore requires adding or deliberately
//! reusing a semantic event and extending the corresponding fold.

use crate::task::{ChecklistItem, Claim, Note, Subtask, Task, ValidationRepair};
use eventcore::ModelEvent;
use eventcore_types::{Event, StreamId};
use serde::{Deserialize, Serialize};

/// Concrete CI-recovery state carried by historical snapshot events. New
/// writes are being migrated to per-transition facts, but this type keeps the
/// immutable v1 Git event documents typed and replayable without an opaque
/// `serde_json::Value` domain payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoverySnapshot {
    pub schema_version: u32,
    pub incident_id: String,
    pub state: CiRecoveryPhase,
    pub epoch: u64,
    pub trigger: CiRecoveryTrigger,
    #[serde(default)]
    pub triggers: Vec<CiRecoveryTrigger>,
    pub owner: CiRecoveryParticipant,
    pub lease_expires_at: u64,
    #[serde(default)]
    pub participants: Vec<CiRecoveryParticipant>,
    #[serde(default)]
    pub assignments: Vec<CiRecoveryAssignment>,
    #[serde(default)]
    pub failure_record: Option<CiRecoveryFailureRecord>,
    #[serde(default)]
    pub diagnosis: Option<CiRecoveryDiagnosis>,
    #[serde(default)]
    pub next_action: Option<CiRecoveryAction>,
    #[serde(default)]
    pub replacement: Option<CiRecoveryReplacement>,
    #[serde(default)]
    pub release_proof: Option<CiRecoveryReleaseProof>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryTrigger {
    pub run_id: String,
    pub run_url: String,
    pub failed_sha: String,
    pub workflow: String,
    pub git_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryParticipant {
    pub host: String,
    pub session: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CiRecoveryPhase {
    Diagnosing,
    ActionSelected,
    WaitingCi,
    Resolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CiRecoveryClassification {
    Caused,
    Unrelated,
    Transient,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CiRecoveryActionKind {
    Repair,
    Rerun,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CiRecoveryReplacementStatus {
    Queued,
    Running,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryFailureRecord {
    pub job: String,
    pub step: String,
    pub log_evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryDiagnosis {
    pub cause: String,
    pub classification: CiRecoveryClassification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryAction {
    pub kind: CiRecoveryActionKind,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryReplacement {
    pub run_id: String,
    pub run_url: String,
    pub sha: String,
    pub status: CiRecoveryReplacementStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryReleaseProof {
    pub replacement_run_id: String,
    pub replacement_run_url: String,
    pub sha: String,
    pub terminal_status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryAssignment {
    pub id: String,
    pub owner_epoch: u64,
    pub assignee: CiRecoveryParticipant,
    pub capabilities: Vec<String>,
    pub scope: String,
    pub report: Option<CiRecoveryReport>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRecoveryReport {
    pub summary: String,
    pub evidence: String,
}

/// Historical full-state payload retained only for replaying immutable v1
/// events. New CI transition facts use their own payload types below.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryEvent {
    pub stream_id: StreamId,
    pub state: Box<CiRecoverySnapshot>,
}

/// The immutable facts that open a repository-wide CI incident.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryClaimedEvent {
    pub stream_id: StreamId,
    pub schema_version: u32,
    pub incident_id: String,
    pub trigger: CiRecoveryTrigger,
    pub owner: CiRecoveryParticipant,
    pub lease_expires_at: u64,
}

/// Facts contributed by another participant to an existing CI incident.
///
/// A join can introduce a retry trigger, a participant, or both. The fields
/// are optional because the claim command accepts either independently but rejects
/// an empty fact at the checked model boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryJoinedEvent {
    pub stream_id: StreamId,
    pub trigger: Option<CiRecoveryTrigger>,
    pub participant: Option<CiRecoveryParticipant>,
}

/// A deliberate handoff of CI-recovery ownership to another participant.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryTransferredEvent {
    pub stream_id: StreamId,
    pub owner: CiRecoveryParticipant,
    pub epoch: u64,
    pub lease_expires_at: u64,
    pub participant: Option<CiRecoveryParticipant>,
}

/// A lease-expiry takeover of CI-recovery ownership.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryTakenOverEvent {
    pub stream_id: StreamId,
    pub owner: CiRecoveryParticipant,
    pub epoch: u64,
    pub lease_expires_at: u64,
    pub participant: Option<CiRecoveryParticipant>,
}

/// An owner-issued assignment. Reports are deliberately separate facts so an
/// assignment's identity and authorization cannot be rewritten by a snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryAssignedEvent {
    pub stream_id: StreamId,
    pub assignment: CiRecoveryAssignment,
}

/// An assignee's immutable report for a current-epoch assignment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryReportedEvent {
    pub stream_id: StreamId,
    pub assignment_id: String,
    pub assignee: CiRecoveryParticipant,
    pub report: CiRecoveryReport,
}

/// A lease renewal by the current owner.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryHeartbeatRecordedEvent {
    pub stream_id: StreamId,
    pub epoch: u64,
    pub owner: CiRecoveryParticipant,
    pub lease_expires_at: u64,
}

/// A causal diagnosis resets subsequent recovery choices.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryDiagnosedEvent {
    pub stream_id: StreamId,
    pub epoch: u64,
    pub owner: CiRecoveryParticipant,
    pub failure_record: CiRecoveryFailureRecord,
    pub diagnosis: CiRecoveryDiagnosis,
}

/// A next action selected from the current diagnosis.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryActionChosenEvent {
    pub stream_id: StreamId,
    pub epoch: u64,
    pub owner: CiRecoveryParticipant,
    pub action: CiRecoveryAction,
}

/// The replacement CI run produced for the selected action.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryReplacementRecordedEvent {
    pub stream_id: StreamId,
    pub epoch: u64,
    pub owner: CiRecoveryParticipant,
    pub replacement: CiRecoveryReplacement,
}

/// Terminal proof that a non-failed replacement succeeded.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CiRecoveryResolvedEvent {
    pub stream_id: StreamId,
    pub participant: CiRecoveryParticipant,
    pub proof: CiRecoveryReleaseProof,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RepositoryInitializedEvent {
    pub stream_id: StreamId,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskCreatedEvent {
    pub stream_id: StreamId,
    pub task: Box<Task>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskTransitionedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub status: String,
    pub claim: Option<Claim>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskOrderEvent {
    pub stream_id: StreamId,
    pub order: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskLinksChangedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskSubtaskAddedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub subtask: Subtask,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskSubtaskCheckedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub subtask_id: String,
    pub checked: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskDetailsUpdatedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub title: String,
    pub tags: Vec<String>,
    pub summary: String,
    pub context: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskClaimChangedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub claim: Option<Claim>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskPullRequestChangedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub url: Option<String>,
    pub status: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskAcceptanceAddedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub item: ChecklistItem,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskAcceptanceCheckedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub index: usize,
    pub checked: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskAcceptanceRemovedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub index: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskNoteAddedEvent {
    pub stream_id: StreamId,
    pub stem: String,
    pub note: Note,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskValidationRepairedEvent {
    pub stream_id: StreamId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_changes: Vec<TaskLinksChangedEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_change: Option<TaskOrderEvent>,
    pub repairs: Vec<ValidationRepair>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TasksClosedFromCommitTrailersEvent {
    pub stream_id: StreamId,
    pub stems: Vec<String>,
    pub order: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskStemEvent {
    pub stream_id: StreamId,
    pub stem: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskStatePublishedEvent {
    pub stream_id: StreamId,
}

#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TiberEvent {
    RepositoryInitialized(RepositoryInitializedEvent),
    TaskCreated(TaskCreatedEvent),
    TaskTransitioned(TaskTransitionedEvent),
    TaskPriorityChanged(TaskOrderEvent),
    TaskLinksChanged(TaskLinksChangedEvent),
    TaskSubtaskAdded(TaskSubtaskAddedEvent),
    TaskSubtaskChecked(TaskSubtaskCheckedEvent),
    TaskDetailsUpdated(TaskDetailsUpdatedEvent),
    /// Historical claim-only fact. Fresh lifecycle commands publish
    /// `TaskTransitioned`, which carries the current claim.
    #[serde(rename = "task_claim_changed")]
    LegacyTaskClaimChanged(TaskClaimChangedEvent),
    TaskPullRequestChanged(TaskPullRequestChangedEvent),
    TaskAcceptanceAdded(TaskAcceptanceAddedEvent),
    TaskAcceptanceChecked(TaskAcceptanceCheckedEvent),
    TaskAcceptanceRemoved(TaskAcceptanceRemovedEvent),
    TaskNoteAdded(TaskNoteAddedEvent),
    TaskValidationRepaired(TaskValidationRepairedEvent),
    TasksClosedFromCommitTrailers(TasksClosedFromCommitTrailersEvent),
    /// Historical singular trailer-closure fact retained solely for replay.
    #[serde(rename = "task_closed_from_trailer")]
    LegacyTaskClosedFromTrailer(TaskStemEvent),
    /// Historical deletion fact retained solely for replay compatibility.
    #[serde(rename = "task_removed")]
    LegacyTaskRemoved(TaskStemEvent),
    BoardReordered(TaskOrderEvent),
    /// Historical projection-notification event. New command paths never emit
    /// this; retained only to replay existing event-store history.
    #[serde(rename = "task_state_published")]
    LegacyTaskStatePublished(TaskStatePublishedEvent),
    CiRecoveryClaimed(CiRecoveryClaimedEvent),
    CiRecoveryJoined(CiRecoveryJoinedEvent),
    CiRecoveryTransferred(CiRecoveryTransferredEvent),
    CiRecoveryTakenOver(CiRecoveryTakenOverEvent),
    CiRecoveryAssigned(CiRecoveryAssignedEvent),
    CiRecoveryReported(CiRecoveryReportedEvent),
    CiRecoveryHeartbeatRecorded(CiRecoveryHeartbeatRecordedEvent),
    CiRecoveryDiagnosed(CiRecoveryDiagnosedEvent),
    CiRecoveryActionChosen(CiRecoveryActionChosenEvent),
    CiRecoveryReplacementRecorded(CiRecoveryReplacementRecordedEvent),
    CiRecoveryResolved(CiRecoveryResolvedEvent),
    /// Historical full-snapshot event retained solely so existing immutable
    /// Git-backed streams can be replayed. New CI-recovery commands must not
    /// emit it; named transition facts are the authority going forward.
    #[serde(rename = "recovery_state_published")]
    LegacyRecoveryStatePublished(CiRecoveryEvent),
}

#[expect(
    non_snake_case,
    reason = "EventCore 2.0.0 mapping! requires getters named after ModelEvent variants"
)]
impl TiberEvent {
    // EventCore 2.0.0's checked `mapping!` expansion uses the same generated
    // getter convention for struct fields and enum payload variants, while its
    // `ModelEvent` derive currently emits only variant constructors. Keep the
    // missing typed accessors next to the event vocabulary until the derive
    // supplies them upstream; a mismatched accessor is a programming defect in
    // a statically registered mapping, never a recoverable domain condition.
    #[doc(hidden)]
    pub fn __eventcore_model_get_RepositoryInitialized(&self) -> &RepositoryInitializedEvent {
        let Self::RepositoryInitialized(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskCreated(&self) -> &TaskCreatedEvent {
        let Self::TaskCreated(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskTransitioned(&self) -> &TaskTransitionedEvent {
        let Self::TaskTransitioned(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskPriorityChanged(&self) -> &TaskOrderEvent {
        let Self::TaskPriorityChanged(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskLinksChanged(&self) -> &TaskLinksChangedEvent {
        let Self::TaskLinksChanged(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskSubtaskAdded(&self) -> &TaskSubtaskAddedEvent {
        let Self::TaskSubtaskAdded(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskSubtaskChecked(&self) -> &TaskSubtaskCheckedEvent {
        let Self::TaskSubtaskChecked(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskDetailsUpdated(&self) -> &TaskDetailsUpdatedEvent {
        let Self::TaskDetailsUpdated(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_LegacyTaskClaimChanged(&self) -> &TaskClaimChangedEvent {
        let Self::LegacyTaskClaimChanged(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskPullRequestChanged(&self) -> &TaskPullRequestChangedEvent {
        let Self::TaskPullRequestChanged(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskAcceptanceAdded(&self) -> &TaskAcceptanceAddedEvent {
        let Self::TaskAcceptanceAdded(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskAcceptanceChecked(&self) -> &TaskAcceptanceCheckedEvent {
        let Self::TaskAcceptanceChecked(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskAcceptanceRemoved(&self) -> &TaskAcceptanceRemovedEvent {
        let Self::TaskAcceptanceRemoved(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskNoteAdded(&self) -> &TaskNoteAddedEvent {
        let Self::TaskNoteAdded(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TaskValidationRepaired(&self) -> &TaskValidationRepairedEvent {
        let Self::TaskValidationRepaired(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_TasksClosedFromCommitTrailers(
        &self,
    ) -> &TasksClosedFromCommitTrailersEvent {
        let Self::TasksClosedFromCommitTrailers(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_LegacyTaskClosedFromTrailer(&self) -> &TaskStemEvent {
        let Self::LegacyTaskClosedFromTrailer(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_LegacyTaskRemoved(&self) -> &TaskStemEvent {
        let Self::LegacyTaskRemoved(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_BoardReordered(&self) -> &TaskOrderEvent {
        let Self::BoardReordered(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_LegacyTaskStatePublished(&self) -> &TaskStatePublishedEvent {
        let Self::LegacyTaskStatePublished(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryClaimed(&self) -> &CiRecoveryClaimedEvent {
        let Self::CiRecoveryClaimed(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryJoined(&self) -> &CiRecoveryJoinedEvent {
        let Self::CiRecoveryJoined(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryTransferred(&self) -> &CiRecoveryTransferredEvent {
        let Self::CiRecoveryTransferred(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryTakenOver(&self) -> &CiRecoveryTakenOverEvent {
        let Self::CiRecoveryTakenOver(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryAssigned(&self) -> &CiRecoveryAssignedEvent {
        let Self::CiRecoveryAssigned(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryReported(&self) -> &CiRecoveryReportedEvent {
        let Self::CiRecoveryReported(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryHeartbeatRecorded(
        &self,
    ) -> &CiRecoveryHeartbeatRecordedEvent {
        let Self::CiRecoveryHeartbeatRecorded(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryDiagnosed(&self) -> &CiRecoveryDiagnosedEvent {
        let Self::CiRecoveryDiagnosed(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryActionChosen(&self) -> &CiRecoveryActionChosenEvent {
        let Self::CiRecoveryActionChosen(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryReplacementRecorded(
        &self,
    ) -> &CiRecoveryReplacementRecordedEvent {
        let Self::CiRecoveryReplacementRecorded(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_CiRecoveryResolved(&self) -> &CiRecoveryResolvedEvent {
        let Self::CiRecoveryResolved(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }
    #[doc(hidden)]
    pub fn __eventcore_model_get_LegacyRecoveryStatePublished(&self) -> &CiRecoveryEvent {
        let Self::LegacyRecoveryStatePublished(value) = self else {
            unreachable!("modeled event variant mismatch")
        };
        value
    }

    pub fn stream_id_value(&self) -> &StreamId {
        match self {
            Self::RepositoryInitialized(RepositoryInitializedEvent { stream_id })
            | Self::TaskCreated(TaskCreatedEvent { stream_id, .. })
            | Self::TaskTransitioned(TaskTransitionedEvent { stream_id, .. })
            | Self::TaskPriorityChanged(TaskOrderEvent { stream_id, .. })
            | Self::TaskLinksChanged(TaskLinksChangedEvent { stream_id, .. })
            | Self::TaskSubtaskAdded(TaskSubtaskAddedEvent { stream_id, .. })
            | Self::TaskSubtaskChecked(TaskSubtaskCheckedEvent { stream_id, .. })
            | Self::TaskDetailsUpdated(TaskDetailsUpdatedEvent { stream_id, .. })
            | Self::LegacyTaskClaimChanged(TaskClaimChangedEvent { stream_id, .. })
            | Self::TaskPullRequestChanged(TaskPullRequestChangedEvent { stream_id, .. })
            | Self::TaskAcceptanceAdded(TaskAcceptanceAddedEvent { stream_id, .. })
            | Self::TaskAcceptanceChecked(TaskAcceptanceCheckedEvent { stream_id, .. })
            | Self::TaskAcceptanceRemoved(TaskAcceptanceRemovedEvent { stream_id, .. })
            | Self::TaskNoteAdded(TaskNoteAddedEvent { stream_id, .. })
            | Self::TaskValidationRepaired(TaskValidationRepairedEvent { stream_id, .. })
            | Self::TasksClosedFromCommitTrailers(TasksClosedFromCommitTrailersEvent {
                stream_id,
                ..
            })
            | Self::LegacyTaskClosedFromTrailer(TaskStemEvent { stream_id, .. })
            | Self::LegacyTaskRemoved(TaskStemEvent { stream_id, .. })
            | Self::BoardReordered(TaskOrderEvent { stream_id, .. })
            | Self::LegacyTaskStatePublished(TaskStatePublishedEvent { stream_id })
            | Self::CiRecoveryClaimed(CiRecoveryClaimedEvent { stream_id, .. })
            | Self::CiRecoveryJoined(CiRecoveryJoinedEvent { stream_id, .. })
            | Self::CiRecoveryTransferred(CiRecoveryTransferredEvent { stream_id, .. })
            | Self::CiRecoveryTakenOver(CiRecoveryTakenOverEvent { stream_id, .. })
            | Self::CiRecoveryAssigned(CiRecoveryAssignedEvent { stream_id, .. })
            | Self::CiRecoveryReported(CiRecoveryReportedEvent { stream_id, .. })
            | Self::CiRecoveryHeartbeatRecorded(CiRecoveryHeartbeatRecordedEvent {
                stream_id,
                ..
            })
            | Self::CiRecoveryDiagnosed(CiRecoveryDiagnosedEvent { stream_id, .. })
            | Self::CiRecoveryActionChosen(CiRecoveryActionChosenEvent { stream_id, .. })
            | Self::CiRecoveryReplacementRecorded(CiRecoveryReplacementRecordedEvent {
                stream_id,
                ..
            })
            | Self::CiRecoveryResolved(CiRecoveryResolvedEvent { stream_id, .. })
            | Self::LegacyRecoveryStatePublished(CiRecoveryEvent { stream_id, .. }) => stream_id,
        }
    }
}

impl Event for TiberEvent {
    fn stream_id(&self) -> &StreamId {
        self.stream_id_value()
    }
    fn event_type_name() -> &'static str {
        "tiber.domain_event"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn historical_task_facts_replay_through_explicit_legacy_variants() {
        let claim = serde_json::from_value::<TiberEvent>(json!({
            "event": "task_claim_changed",
            "stream_id": "tiber:board",
            "stem": "task-a",
            "claim": null
        }))
        .expect("historical claim fact");
        let close = serde_json::from_value::<TiberEvent>(json!({
            "event": "task_closed_from_trailer",
            "stream_id": "tiber:board",
            "stem": "task-a"
        }))
        .expect("historical trailer fact");
        let removed = serde_json::from_value::<TiberEvent>(json!({
            "event": "task_removed",
            "stream_id": "tiber:board",
            "stem": "task-a"
        }))
        .expect("historical removal fact");

        assert!(matches!(claim, TiberEvent::LegacyTaskClaimChanged(_)));
        assert!(matches!(close, TiberEvent::LegacyTaskClosedFromTrailer(_)));
        assert!(matches!(removed, TiberEvent::LegacyTaskRemoved(_)));
    }

    #[test]
    fn historical_validation_fact_defaults_new_repair_payload_fields() {
        let event = serde_json::from_value::<TiberEvent>(json!({
            "event": "task_validation_repaired",
            "stream_id": "tiber:board",
            "repairs": []
        }))
        .expect("historical validation fact");

        let TiberEvent::TaskValidationRepaired(event) = event else {
            panic!("expected validation fact")
        };
        assert!(event.link_changes.is_empty());
        assert!(event.order_change.is_none());
    }
}
