//! Command-specific durable authority for repository mutations.

#![forbid(unsafe_code)]
#![expect(
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    reason = "EventCore's model derives generate public checked-model helpers without item-local lint hooks"
)]

use core::{error::Error, fmt};

use eventcore::{
    CommandError, CommandLogic, Event, ModelCommand, ModelEvent, ModelInput, ModelOutput,
    ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity},
};
use serde::{Deserialize, Serialize};
use tiber_repository_core::{
    AuthorizedRepositoryMutation, OwnerApprovalId, RepositoryAssignmentContext,
    RepositoryMutationFailure, RepositoryMutationIdentity, RepositoryMutationPolicy,
    RepositoryMutationProposal, RepositoryMutationProposalIdentity, RepositoryMutationProvenance,
    RepositoryMutationReceipt, RepositoryReconciliation, RepositoryReconciliationOutcome,
    authorize_prepared_mutation as authorize_core_prepared_mutation, prepare_mutation_identity,
};

/// Stable service failures exposed at the authority boundary.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "stable service failures are grouped by lifecycle meaning rather than alphabetically"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryMutationServiceError {
    /// The proposal provenance cannot form a valid `EventCore` stream.
    InvalidStream,
    /// The caller-supplied stream is not owned by the proposed workflow effect.
    StreamProposalMismatch,
    /// Checked command logic rejected retained history or the requested transition.
    ModeledCommandFailed,
    /// The supplied proposal no longer matches the durable safe identity.
    StaleProposal,
    /// The supplied active workflow provenance no longer matches the proposal.
    StaleWorkflowProvenance,
    /// Retained mutation history violates lifecycle ordering or identity.
    InvalidHistory,
    /// No durable proposal exists for the requested owner decision.
    ProposalMissing,
    /// An owner decision already terminated the proposal.
    OwnerDecisionAlreadyRecorded,
    /// A dispatch outcome already terminated the prepared mutation.
    TerminalAlreadyRecorded,
    /// A read-only reconciliation result already exists for the prepared mutation.
    ReconciliationAlreadyRecorded,
    /// Durable mutation history could not be read for restart recovery.
    RecoveryReadFailed,
    /// Checked command logic did not emit exactly one durable fact.
    InvalidModeledEmission,
    /// Pure repository policy rejected dispatch authorization.
    AuthorizationRejected,
}

impl RepositoryMutationServiceError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        match *self {
            Self::InvalidHistory => "repository_mutation_history_invalid",
            Self::StreamProposalMismatch => "repository_mutation_stream_proposal_mismatch",
            Self::InvalidStream
            | Self::ModeledCommandFailed
            | Self::StaleProposal
            | Self::StaleWorkflowProvenance
            | Self::ProposalMissing
            | Self::OwnerDecisionAlreadyRecorded
            | Self::TerminalAlreadyRecorded
            | Self::ReconciliationAlreadyRecorded
            | Self::RecoveryReadFailed
            | Self::InvalidModeledEmission
            | Self::AuthorizationRejected => "repository_mutation_rejected",
        }
    }
}

impl fmt::Display for RepositoryMutationServiceError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            Self::InvalidStream => "repository mutation stream is invalid",
            Self::StreamProposalMismatch => "repository mutation stream does not own proposal",
            Self::ModeledCommandFailed => "repository mutation command was rejected",
            Self::StaleProposal => "repository mutation proposal is stale",
            Self::StaleWorkflowProvenance => "repository mutation workflow provenance is stale",
            Self::InvalidHistory => "repository mutation history is invalid",
            Self::ProposalMissing => "repository mutation proposal is missing",
            Self::OwnerDecisionAlreadyRecorded => {
                "repository mutation owner decision already exists"
            }
            Self::TerminalAlreadyRecorded => "repository mutation terminal outcome already exists",
            Self::ReconciliationAlreadyRecorded => {
                "repository reconciliation result already exists"
            }
            Self::RecoveryReadFailed => "repository mutation recovery history could not be read",
            Self::InvalidModeledEmission => {
                "repository mutation command emitted an invalid event set"
            }
            Self::AuthorizationRejected => "repository mutation authorization was rejected",
        })
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the error has no nested source or nightly request-provider metadata"
)]
impl Error for RepositoryMutationServiceError {}

/// One durable stream per workflow-owned repository mutation effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryMutationStream(StreamId);

impl RepositoryMutationStream {
    /// Builds the exact repository-mutation stream owned by one durable effect.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryMutationServiceError::InvalidStream`] when `EventCore`
    /// rejects the effect-derived identifier.
    #[inline]
    pub fn for_effect(
        effect_id: &tiber_workflow_core::EffectId,
    ) -> Result<Self, RepositoryMutationServiceError> {
        StreamId::try_new(format!("tiber:repository-mutation:{}", effect_id.as_str()))
            .map(Self)
            .map_err(|_source| RepositoryMutationServiceError::InvalidStream)
    }

    /// Builds the stream derived from an exact workflow provenance.
    fn for_provenance(
        provenance: &RepositoryMutationProvenance,
    ) -> Result<Self, RepositoryMutationServiceError> {
        Self::for_effect(provenance.effect_id())
    }

    /// Builds the stream owned by an exact proposal's workflow effect.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryMutationServiceError::InvalidStream`] if `EventCore`
    /// rejects the effect-derived identifier.
    #[inline]
    pub fn new(
        proposal: &RepositoryMutationProposalIdentity,
    ) -> Result<Self, RepositoryMutationServiceError> {
        Self::for_provenance(proposal.provenance())
    }
}

impl StreamIdentity for RepositoryMutationStream {
    #[inline]
    fn as_stream_id(&self) -> &StreamId {
        &self.0
    }
}

/// Immutable repository-mutation lifecycle facts.
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "durable facts remain in lifecycle order from proposal through reconciliation"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum RepositoryMutationFact {
    /// A safe, content-free proposal identity was durably admitted.
    Proposed(RepositoryMutationProposalIdentity),
    /// A stale proposal was replaced by a newly read safe identity.
    Reproposed(RepositoryMutationProposalIdentity),
    /// The owner approved the exact durable proposal under its active workflow.
    Approved(RepositoryMutationApprovalFact),
    /// The owner denied the exact durable proposal under its active workflow.
    Denied(RepositoryMutationProposalIdentity),
    /// The owner cancelled the exact durable proposal under its active workflow.
    Cancelled(RepositoryMutationProposalIdentity),
    /// Dispatch authority was persisted immediately before becoming adapter-visible.
    Prepared(RepositoryMutationIdentity),
    /// The adapter definitively applied the prepared mutation.
    Applied(tiber_repository_core::RepositoryMutationReceipt),
    /// The adapter definitively proved the prepared mutation did not apply.
    Failed(RepositoryMutationFailure),
    /// The adapter could not establish whether the prepared mutation applied.
    Unknown(RepositoryReconciliation),
    /// A restart-time read-only reconciliation produced one durable result.
    Reconciled(RepositoryReconciliationOutcome),
}

/// Durable content-free owner approval for one exact proposal identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMutationApprovalFact {
    /// Exact durable owner-approval identity.
    approval: OwnerApprovalId,
    /// Safe proposal identity approved by the owner.
    proposal: RepositoryMutationProposalIdentity,
}

impl RepositoryMutationApprovalFact {
    /// Returns the durable owner-decision identity.
    #[must_use]
    #[inline]
    pub const fn approval(&self) -> &OwnerApprovalId {
        &self.approval
    }

    /// Returns the exact safe proposal identity approved by the owner.
    #[must_use]
    #[inline]
    pub const fn proposal(&self) -> &RepositoryMutationProposalIdentity {
        &self.proposal
    }
}

/// Durable `EventCore` envelope for one repository mutation.
#[derive(Clone, Debug, Deserialize, ModelEvent, Serialize)]
pub struct RepositoryMutationEvent {
    /// Immutable lifecycle fact carried by the durable event.
    fact: RepositoryMutationFact,
    /// Owning mutation stream used for optimistic consistency.
    stream: StreamId,
}

impl RepositoryMutationEvent {
    /// Returns the immutable lifecycle fact.
    #[must_use]
    #[inline]
    pub const fn fact(&self) -> &RepositoryMutationFact {
        &self.fact
    }
}

impl Event for RepositoryMutationEvent {
    #[inline]
    fn event_type_name() -> &'static str {
        "TiberRepositoryMutationEvent"
    }

    #[inline]
    fn stream_id(&self) -> &StreamId {
        &self.stream
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled projection of one durable repository-mutation event.
struct RepositoryMutationEventView {
    /// Owning mutation stream projected from the event.
    /// Owning repository-mutation stream.
    stream: StreamId,
    /// Immutable lifecycle fact projected from the event.
    /// Immutable lifecycle fact projected from the durable event.
    fact: RepositoryMutationFact,
}

mapping! { RepositoryMutationEventStreamToView: RepositoryMutationEvent.stream => RepositoryMutationEventView.stream using clone; }
mapping! { RepositoryMutationEventFactToView: RepositoryMutationEvent.fact => RepositoryMutationEventView.fact using clone; }

impl RepositoryMutationEventView {
    /// Projects one durable event into checked modeled state.
    fn from_event(event: &RepositoryMutationEvent) -> Modeled<Self> {
        Self::model_builder()
            .stream(RepositoryMutationEventStreamToView::apply(event))
            .fact(RepositoryMutationEventFactToView::apply(event))
            .build()
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the propose mutation transition.
struct ProposeMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the propose mutation transition.
struct ProposeMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
}

mapping! { ProposeMutationRequestToStream: ProposeMutationRequest.stream => ProposeMutation.stream using clone; }
mapping! { ProposeMutationRequestToProposal: ProposeMutationRequest.proposal => ProposeMutation.proposal using clone; }

#[derive(ModelState)]
/// Folded durable state used to decide the propose mutation transition.
struct ProposeMutationState {
    #[model(default)]
    /// Whether this proposal is already durably recorded.
    proposed: bool,
}

#[derive(ModelOutput)]
/// Modeled output selected by the propose mutation transition.
struct ProposeMutationDecision {
    /// Whether this proposal is already durably recorded.
    proposed: bool,
}

mapping! { ProposeMutationStateToDecision: ProposeMutationState.proposed => ProposeMutationDecision.proposed using copy; }
mapping! { ProposeMutationStreamToEvent: ProposeMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { ProposeMutationProposalToFact: (ProposeMutation.proposal, ProposeMutationDecision.proposed) => RepositoryMutationEvent.fact using try proposed_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for ProposeMutation {
    type Event = RepositoryMutationEvent;
    type State = ProposeMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let view = RepositoryMutationEventView::from_event(event);
        let proposed = state.as_ref().proposed
            || matches!(&view.as_ref().fact, RepositoryMutationFact::Proposed(_));
        Modeled::from_built(ProposeMutationState { proposed })
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ProposeMutationDecision::model_builder()
            .proposed(ProposeMutationStateToDecision::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(ProposeMutationStreamToEvent::apply(self))
                .fact(ProposeMutationProposalToFact::apply((
                    self,
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the repropose mutation transition.
struct ReproposeMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the repropose mutation transition.
struct ReproposeMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
}

mapping! { ReproposeMutationRequestToStream: ReproposeMutationRequest.stream => ReproposeMutation.stream using clone; }
mapping! { ReproposeMutationRequestToProposal: ReproposeMutationRequest.proposal => ReproposeMutation.proposal using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the repropose mutation transition.
struct ReproposeMutationState {
    #[model(default)]
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    #[model(default)]
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the repropose mutation transition.
struct ReproposeMutationDecision {
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { ReproposeMutationStateToDecisionProposal: ReproposeMutationState.proposal => ReproposeMutationDecision.proposal using clone; }
mapping! { ReproposeMutationStateToDecisionTerminal: ReproposeMutationState.terminal => ReproposeMutationDecision.terminal using copy; }
mapping! { ReproposeMutationStateToDecisionMalformed: ReproposeMutationState.malformed => ReproposeMutationDecision.malformed using copy; }
mapping! { ReproposeMutationStreamToEvent: ReproposeMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { ReproposeMutationToFact: (ReproposeMutation.proposal, ReproposeMutationDecision.proposal, ReproposeMutationDecision.terminal, ReproposeMutationDecision.malformed) => RepositoryMutationEvent.fact using try reproposed_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for ReproposeMutation {
    type Event = RepositoryMutationEvent;
    type State = ReproposeMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(proposal) => {
                if !same_stream || folded.proposal.is_some() || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Reproposed(proposal) => {
                if !same_stream || folded.proposal.is_none() || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Approved(recorded) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(recorded.proposal())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Denied(proposal)
            | RepositoryMutationFact::Cancelled(proposal) => {
                if !same_stream || folded.proposal.as_ref() != Some(proposal) || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Prepared(_)
            | RepositoryMutationFact::Applied(_)
            | RepositoryMutationFact::Failed(_)
            | RepositoryMutationFact::Unknown(_)
            | RepositoryMutationFact::Reconciled(_) => {
                folded.malformed |= !same_stream || folded.proposal.is_none() || folded.terminal;
                folded.terminal = !folded.malformed;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ReproposeMutationDecision::model_builder()
            .proposal(ReproposeMutationStateToDecisionProposal::apply(
                state.as_ref(),
            ))
            .terminal(ReproposeMutationStateToDecisionTerminal::apply(
                state.as_ref(),
            ))
            .malformed(ReproposeMutationStateToDecisionMalformed::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(ReproposeMutationStreamToEvent::apply(self))
                .fact(ReproposeMutationToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the approve mutation transition.
struct ApproveMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    #[model(origin)]
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
    #[model(origin)]
    /// Exact durable owner-approval identity.
    approval: OwnerApprovalId,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the approve mutation transition.
struct ApproveMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
    /// Exact durable owner-approval identity.
    approval: OwnerApprovalId,
}

mapping! { ApproveMutationRequestToStream: ApproveMutationRequest.stream => ApproveMutation.stream using clone; }
mapping! { ApproveMutationRequestToProposal: ApproveMutationRequest.proposal => ApproveMutation.proposal using clone; }
mapping! { ApproveMutationRequestToProvenance: ApproveMutationRequest.active_provenance => ApproveMutation.active_provenance using clone; }
mapping! { ApproveMutationRequestToApproval: ApproveMutationRequest.approval => ApproveMutation.approval using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the approve mutation transition.
struct ApproveMutationState {
    #[model(default)]
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    #[model(default)]
    /// Whether an owner decision has already been recorded.
    decided: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the approve mutation transition.
struct ApproveMutationDecision {
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    /// Whether an owner decision has already been recorded.
    decided: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { ApproveMutationStateToDecisionProposal: ApproveMutationState.proposal => ApproveMutationDecision.proposal using clone; }
mapping! { ApproveMutationStateToDecisionDecided: ApproveMutationState.decided => ApproveMutationDecision.decided using copy; }
mapping! { ApproveMutationStateToDecisionMalformed: ApproveMutationState.malformed => ApproveMutationDecision.malformed using copy; }
mapping! { ApproveMutationStreamToEvent: ApproveMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { ApproveMutationToFact: (ApproveMutation.proposal, ApproveMutation.active_provenance, ApproveMutation.approval, ApproveMutationDecision.proposal, ApproveMutationDecision.decided, ApproveMutationDecision.malformed) => RepositoryMutationEvent.fact using try approved_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for ApproveMutation {
    type Event = RepositoryMutationEvent;
    type State = ApproveMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(proposal) => {
                if !same_stream || folded.proposal.is_some() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Reproposed(proposal) => {
                if !same_stream || folded.proposal.is_none() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Approved(recorded) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(recorded.proposal())
                    || folded.decided
                {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Denied(proposal)
            | RepositoryMutationFact::Cancelled(proposal) => {
                if !same_stream || folded.proposal.as_ref() != Some(proposal) || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Prepared(_)
            | RepositoryMutationFact::Applied(_)
            | RepositoryMutationFact::Failed(_)
            | RepositoryMutationFact::Unknown(_)
            | RepositoryMutationFact::Reconciled(_) => {
                folded.malformed |= !same_stream || folded.proposal.is_none() || folded.decided;
                folded.decided = !folded.malformed;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ApproveMutationDecision::model_builder()
            .proposal(ApproveMutationStateToDecisionProposal::apply(
                state.as_ref(),
            ))
            .decided(ApproveMutationStateToDecisionDecided::apply(state.as_ref()))
            .malformed(ApproveMutationStateToDecisionMalformed::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(ApproveMutationStreamToEvent::apply(self))
                .fact(ApproveMutationToFact::apply((
                    self,
                    self,
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the deny mutation transition.
struct DenyMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    #[model(origin)]
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the deny mutation transition.
struct DenyMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
}

mapping! { DenyMutationRequestToStream: DenyMutationRequest.stream => DenyMutation.stream using clone; }
mapping! { DenyMutationRequestToProposal: DenyMutationRequest.proposal => DenyMutation.proposal using clone; }
mapping! { DenyMutationRequestToProvenance: DenyMutationRequest.active_provenance => DenyMutation.active_provenance using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the deny mutation transition.
struct DenyMutationState {
    #[model(default)]
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    #[model(default)]
    /// Whether an owner decision has already been recorded.
    decided: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the deny mutation transition.
struct DenyMutationDecision {
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    /// Whether an owner decision has already been recorded.
    decided: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { DenyMutationStateToDecisionProposal: DenyMutationState.proposal => DenyMutationDecision.proposal using clone; }
mapping! { DenyMutationStateToDecisionDecided: DenyMutationState.decided => DenyMutationDecision.decided using copy; }
mapping! { DenyMutationStateToDecisionMalformed: DenyMutationState.malformed => DenyMutationDecision.malformed using copy; }
mapping! { DenyMutationStreamToEvent: DenyMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { DenyMutationToFact: (DenyMutation.proposal, DenyMutation.active_provenance, DenyMutationDecision.proposal, DenyMutationDecision.decided, DenyMutationDecision.malformed) => RepositoryMutationEvent.fact using try denied_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for DenyMutation {
    type Event = RepositoryMutationEvent;
    type State = DenyMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(proposal) => {
                if !same_stream || folded.proposal.is_some() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Reproposed(proposal) => {
                if !same_stream || folded.proposal.is_none() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Approved(recorded) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(recorded.proposal())
                    || folded.decided
                {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Denied(proposal)
            | RepositoryMutationFact::Cancelled(proposal) => {
                if !same_stream || folded.proposal.as_ref() != Some(proposal) || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Prepared(_)
            | RepositoryMutationFact::Applied(_)
            | RepositoryMutationFact::Failed(_)
            | RepositoryMutationFact::Unknown(_)
            | RepositoryMutationFact::Reconciled(_) => {
                folded.malformed |= !same_stream || folded.proposal.is_none() || folded.decided;
                folded.decided = !folded.malformed;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = DenyMutationDecision::model_builder()
            .proposal(DenyMutationStateToDecisionProposal::apply(state.as_ref()))
            .decided(DenyMutationStateToDecisionDecided::apply(state.as_ref()))
            .malformed(DenyMutationStateToDecisionMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(DenyMutationStreamToEvent::apply(self))
                .fact(DenyMutationToFact::apply((
                    self,
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the cancel mutation transition.
struct CancelMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    #[model(origin)]
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the cancel mutation transition.
struct CancelMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    /// Active workflow provenance authorizing the owner decision.
    active_provenance: RepositoryMutationProvenance,
}

mapping! { CancelMutationRequestToStream: CancelMutationRequest.stream => CancelMutation.stream using clone; }
mapping! { CancelMutationRequestToProposal: CancelMutationRequest.proposal => CancelMutation.proposal using clone; }
mapping! { CancelMutationRequestToProvenance: CancelMutationRequest.active_provenance => CancelMutation.active_provenance using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the cancel mutation transition.
struct CancelMutationState {
    #[model(default)]
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    #[model(default)]
    /// Whether an owner decision has already been recorded.
    decided: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the cancel mutation transition.
struct CancelMutationDecision {
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    /// Whether an owner decision has already been recorded.
    decided: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { CancelMutationStateToDecisionProposal: CancelMutationState.proposal => CancelMutationDecision.proposal using clone; }
mapping! { CancelMutationStateToDecisionDecided: CancelMutationState.decided => CancelMutationDecision.decided using copy; }
mapping! { CancelMutationStateToDecisionMalformed: CancelMutationState.malformed => CancelMutationDecision.malformed using copy; }
mapping! { CancelMutationStreamToEvent: CancelMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { CancelMutationToFact: (CancelMutation.proposal, CancelMutation.active_provenance, CancelMutationDecision.proposal, CancelMutationDecision.decided, CancelMutationDecision.malformed) => RepositoryMutationEvent.fact using try cancelled_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for CancelMutation {
    type Event = RepositoryMutationEvent;
    type State = CancelMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(proposal) => {
                if !same_stream || folded.proposal.is_some() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Reproposed(proposal) => {
                if !same_stream || folded.proposal.is_none() || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Approved(recorded) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(recorded.proposal())
                    || folded.decided
                {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Denied(proposal)
            | RepositoryMutationFact::Cancelled(proposal) => {
                if !same_stream || folded.proposal.as_ref() != Some(proposal) || folded.decided {
                    folded.malformed = true;
                } else {
                    folded.decided = true;
                }
            }
            RepositoryMutationFact::Prepared(_)
            | RepositoryMutationFact::Applied(_)
            | RepositoryMutationFact::Failed(_)
            | RepositoryMutationFact::Unknown(_)
            | RepositoryMutationFact::Reconciled(_) => {
                folded.malformed |= !same_stream || folded.proposal.is_none() || folded.decided;
                folded.decided = !folded.malformed;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = CancelMutationDecision::model_builder()
            .proposal(CancelMutationStateToDecisionProposal::apply(state.as_ref()))
            .decided(CancelMutationStateToDecisionDecided::apply(state.as_ref()))
            .malformed(CancelMutationStateToDecisionMalformed::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(CancelMutationStreamToEvent::apply(self))
                .fact(CancelMutationToFact::apply((
                    self,
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the prepare mutation transition.
struct PrepareMutationRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    #[model(origin)]
    /// Prepared mutation identity bound to the exact proposal.
    identity: RepositoryMutationIdentity,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the prepare mutation transition.
struct PrepareMutation {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Exact safe proposal identity folded or commanded.
    proposal: RepositoryMutationProposalIdentity,
    /// Prepared mutation identity bound to the exact proposal.
    identity: RepositoryMutationIdentity,
}

mapping! { PrepareMutationRequestToStream: PrepareMutationRequest.stream => PrepareMutation.stream using clone; }
mapping! { PrepareMutationRequestToProposal: PrepareMutationRequest.proposal => PrepareMutation.proposal using clone; }
mapping! { PrepareMutationRequestToIdentity: PrepareMutationRequest.identity => PrepareMutation.identity using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the prepare mutation transition.
struct PrepareMutationState {
    #[model(default)]
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    #[model(default)]
    /// Exact durable owner-approval identity.
    approval: Option<RepositoryMutationApprovalFact>,
    #[model(default)]
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: bool,
    #[model(default)]
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the prepare mutation transition.
struct PrepareMutationDecision {
    /// Exact safe proposal identity folded or commanded.
    proposal: Option<RepositoryMutationProposalIdentity>,
    /// Exact durable owner-approval identity.
    approval: Option<RepositoryMutationApprovalFact>,
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: bool,
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { PrepareMutationStateToDecisionProposal: PrepareMutationState.proposal => PrepareMutationDecision.proposal using clone; }
mapping! { PrepareMutationStateToDecisionApproval: PrepareMutationState.approval => PrepareMutationDecision.approval using clone; }
mapping! { PrepareMutationStateToDecisionPrepared: PrepareMutationState.prepared => PrepareMutationDecision.prepared using copy; }
mapping! { PrepareMutationStateToDecisionTerminal: PrepareMutationState.terminal => PrepareMutationDecision.terminal using copy; }
mapping! { PrepareMutationStateToDecisionMalformed: PrepareMutationState.malformed => PrepareMutationDecision.malformed using copy; }
mapping! { PrepareMutationStreamToEvent: PrepareMutation.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { PrepareMutationToFact: (PrepareMutation.proposal, PrepareMutation.identity, PrepareMutationDecision.proposal, PrepareMutationDecision.approval, PrepareMutationDecision.prepared, PrepareMutationDecision.terminal, PrepareMutationDecision.malformed) => RepositoryMutationEvent.fact using try prepared_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for PrepareMutation {
    type Event = RepositoryMutationEvent;
    type State = PrepareMutationState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(proposal) => {
                if !same_stream || folded.proposal.is_some() || folded.terminal || folded.prepared {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                }
            }
            RepositoryMutationFact::Reproposed(proposal) => {
                if !same_stream || folded.proposal.is_none() || folded.terminal || folded.prepared {
                    folded.malformed = true;
                } else {
                    folded.proposal = Some(proposal.clone());
                    folded.approval = None;
                }
            }
            RepositoryMutationFact::Approved(approval) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(approval.proposal())
                    || folded.approval.is_some()
                    || folded.terminal
                    || folded.prepared
                {
                    folded.malformed = true;
                } else {
                    folded.approval = Some(approval.clone());
                }
            }
            RepositoryMutationFact::Denied(proposal)
            | RepositoryMutationFact::Cancelled(proposal) => {
                if !same_stream
                    || folded.proposal.as_ref() != Some(proposal)
                    || folded.terminal
                    || folded.prepared
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Prepared(identity) => {
                if !same_stream || folded.prepared || folded.terminal || identity != &self.identity
                {
                    folded.malformed = true;
                } else {
                    folded.prepared = true;
                }
            }
            RepositoryMutationFact::Applied(receipt) => {
                if !same_stream
                    || !folded.prepared
                    || folded.terminal
                    || receipt.identity() != &self.identity
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Failed(failure) => {
                if !same_stream
                    || !folded.prepared
                    || folded.terminal
                    || failure.identity() != &self.identity
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if !same_stream
                    || !folded.prepared
                    || folded.terminal
                    || reconciliation.identity() != &self.identity
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                if !same_stream
                    || !folded.prepared
                    || folded.terminal
                    || reconciliation_outcome_identity(outcome) != &self.identity
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = PrepareMutationDecision::model_builder()
            .proposal(PrepareMutationStateToDecisionProposal::apply(
                state.as_ref(),
            ))
            .approval(PrepareMutationStateToDecisionApproval::apply(
                state.as_ref(),
            ))
            .prepared(PrepareMutationStateToDecisionPrepared::apply(
                state.as_ref(),
            ))
            .terminal(PrepareMutationStateToDecisionTerminal::apply(
                state.as_ref(),
            ))
            .malformed(PrepareMutationStateToDecisionMalformed::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(PrepareMutationStreamToEvent::apply(self))
                .fact(PrepareMutationToFact::apply((
                    self,
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the record applied transition.
struct RecordAppliedRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Content-free receipt for the definitively applied mutation.
    receipt: RepositoryMutationReceipt,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the record applied transition.
struct RecordApplied {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Content-free receipt for the definitively applied mutation.
    receipt: RepositoryMutationReceipt,
}

mapping! { RecordAppliedRequestToStream: RecordAppliedRequest.stream => RecordApplied.stream using clone; }
mapping! { RecordAppliedRequestToReceipt: RecordAppliedRequest.receipt => RecordApplied.receipt using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the record applied transition.
struct RecordAppliedState {
    #[model(default)]
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    #[model(default)]
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the record applied transition.
struct RecordAppliedDecision {
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { RecordAppliedStateToDecisionPrepared: RecordAppliedState.prepared => RecordAppliedDecision.prepared using clone; }
mapping! { RecordAppliedStateToDecisionTerminal: RecordAppliedState.terminal => RecordAppliedDecision.terminal using copy; }
mapping! { RecordAppliedStateToDecisionMalformed: RecordAppliedState.malformed => RecordAppliedDecision.malformed using copy; }
mapping! { RecordAppliedStreamToEvent: RecordApplied.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { RecordAppliedToFact: (RecordApplied.receipt, RecordAppliedDecision.prepared, RecordAppliedDecision.terminal, RecordAppliedDecision.malformed) => RepositoryMutationEvent.fact using try applied_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for RecordApplied {
    type Event = RepositoryMutationEvent;
    type State = RecordAppliedState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(_)
            | RepositoryMutationFact::Reproposed(_)
            | RepositoryMutationFact::Approved(_) => {
                folded.malformed |= !same_stream || folded.prepared.is_some() || folded.terminal;
            }
            RepositoryMutationFact::Denied(_) | RepositoryMutationFact::Cancelled(_) => {
                folded.malformed = true;
                folded.terminal = true;
            }
            RepositoryMutationFact::Prepared(identity) => {
                if !same_stream || folded.prepared.is_some() || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.prepared = Some(identity.clone());
                }
            }
            RepositoryMutationFact::Applied(receipt) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(receipt.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Failed(failure) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(failure.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation_outcome_identity(outcome))
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RecordAppliedDecision::model_builder()
            .prepared(RecordAppliedStateToDecisionPrepared::apply(state.as_ref()))
            .terminal(RecordAppliedStateToDecisionTerminal::apply(state.as_ref()))
            .malformed(RecordAppliedStateToDecisionMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(RecordAppliedStreamToEvent::apply(self))
                .fact(RecordAppliedToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the record failed transition.
struct RecordFailedRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Content-free receipt for the definitively rejected mutation.
    failure: RepositoryMutationFailure,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the record failed transition.
struct RecordFailed {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Content-free receipt for the definitively rejected mutation.
    failure: RepositoryMutationFailure,
}

mapping! { RecordFailedRequestToStream: RecordFailedRequest.stream => RecordFailed.stream using clone; }
mapping! { RecordFailedRequestToFailure: RecordFailedRequest.failure => RecordFailed.failure using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the record failed transition.
struct RecordFailedState {
    #[model(default)]
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    #[model(default)]
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the record failed transition.
struct RecordFailedDecision {
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { RecordFailedStateToDecisionPrepared: RecordFailedState.prepared => RecordFailedDecision.prepared using clone; }
mapping! { RecordFailedStateToDecisionTerminal: RecordFailedState.terminal => RecordFailedDecision.terminal using copy; }
mapping! { RecordFailedStateToDecisionMalformed: RecordFailedState.malformed => RecordFailedDecision.malformed using copy; }
mapping! { RecordFailedStreamToEvent: RecordFailed.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { RecordFailedToFact: (RecordFailed.failure, RecordFailedDecision.prepared, RecordFailedDecision.terminal, RecordFailedDecision.malformed) => RepositoryMutationEvent.fact using try failed_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for RecordFailed {
    type Event = RepositoryMutationEvent;
    type State = RecordFailedState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(_)
            | RepositoryMutationFact::Reproposed(_)
            | RepositoryMutationFact::Approved(_) => {
                folded.malformed |= !same_stream || folded.prepared.is_some() || folded.terminal;
            }
            RepositoryMutationFact::Denied(_) | RepositoryMutationFact::Cancelled(_) => {
                folded.malformed = true;
                folded.terminal = true;
            }
            RepositoryMutationFact::Prepared(identity) => {
                if !same_stream || folded.prepared.is_some() || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.prepared = Some(identity.clone());
                }
            }
            RepositoryMutationFact::Applied(receipt) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(receipt.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Failed(failure) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(failure.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation_outcome_identity(outcome))
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RecordFailedDecision::model_builder()
            .prepared(RecordFailedStateToDecisionPrepared::apply(state.as_ref()))
            .terminal(RecordFailedStateToDecisionTerminal::apply(state.as_ref()))
            .malformed(RecordFailedStateToDecisionMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(RecordFailedStreamToEvent::apply(self))
                .fact(RecordFailedToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the record unknown transition.
struct RecordUnknownRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Read-only reconciliation authority for an ambiguous mutation.
    reconciliation: RepositoryReconciliation,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the record unknown transition.
struct RecordUnknown {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Read-only reconciliation authority for an ambiguous mutation.
    reconciliation: RepositoryReconciliation,
}

mapping! { RecordUnknownRequestToStream: RecordUnknownRequest.stream => RecordUnknown.stream using clone; }
mapping! { RecordUnknownRequestToReconciliation: RecordUnknownRequest.reconciliation => RecordUnknown.reconciliation using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the record unknown transition.
struct RecordUnknownState {
    #[model(default)]
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    #[model(default)]
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the record unknown transition.
struct RecordUnknownDecision {
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    /// Whether a terminal lifecycle fact has already been recorded.
    terminal: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { RecordUnknownStateToDecisionPrepared: RecordUnknownState.prepared => RecordUnknownDecision.prepared using clone; }
mapping! { RecordUnknownStateToDecisionTerminal: RecordUnknownState.terminal => RecordUnknownDecision.terminal using copy; }
mapping! { RecordUnknownStateToDecisionMalformed: RecordUnknownState.malformed => RecordUnknownDecision.malformed using copy; }
mapping! { RecordUnknownStreamToEvent: RecordUnknown.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { RecordUnknownToFact: (RecordUnknown.reconciliation, RecordUnknownDecision.prepared, RecordUnknownDecision.terminal, RecordUnknownDecision.malformed) => RepositoryMutationEvent.fact using try unknown_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for RecordUnknown {
    type Event = RepositoryMutationEvent;
    type State = RecordUnknownState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(_)
            | RepositoryMutationFact::Reproposed(_)
            | RepositoryMutationFact::Approved(_) => {
                folded.malformed |= !same_stream || folded.prepared.is_some() || folded.terminal;
            }
            RepositoryMutationFact::Denied(_) | RepositoryMutationFact::Cancelled(_) => {
                folded.malformed = true;
                folded.terminal = true;
            }
            RepositoryMutationFact::Prepared(identity) => {
                if !same_stream || folded.prepared.is_some() || folded.terminal {
                    folded.malformed = true;
                } else {
                    folded.prepared = Some(identity.clone());
                }
            }
            RepositoryMutationFact::Applied(receipt) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(receipt.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Failed(failure) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(failure.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation.identity())
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                if !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation_outcome_identity(outcome))
                    || folded.terminal
                {
                    folded.malformed = true;
                } else {
                    folded.terminal = true;
                }
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RecordUnknownDecision::model_builder()
            .prepared(RecordUnknownStateToDecisionPrepared::apply(state.as_ref()))
            .terminal(RecordUnknownStateToDecisionTerminal::apply(state.as_ref()))
            .malformed(RecordUnknownStateToDecisionMalformed::apply(state.as_ref()))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(RecordUnknownStreamToEvent::apply(self))
                .fact(RecordUnknownToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelInput)]
/// Modeled input binding for the record reconciled transition.
struct RecordReconciledRequest {
    #[model(origin)]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    #[model(origin)]
    /// Read-only reconciliation outcome to record.
    outcome: RepositoryReconciliationOutcome,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelCommand)]
/// Checked `EventCore` command for the record reconciled transition.
struct RecordReconciled {
    #[stream]
    /// Owning repository-mutation stream.
    stream: RepositoryMutationStream,
    /// Read-only reconciliation outcome to record.
    outcome: RepositoryReconciliationOutcome,
}

mapping! { RecordReconciledRequestToStream: RecordReconciledRequest.stream => RecordReconciled.stream using clone; }
mapping! { RecordReconciledRequestToOutcome: RecordReconciledRequest.outcome => RecordReconciled.outcome using clone; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::struct_excessive_bools,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelState)]
/// Folded durable state used to decide the record reconciled transition.
struct RecordReconciledState {
    #[model(default)]
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    #[model(default)]
    /// Whether retained history contains a definitive dispatch outcome.
    definitive_terminal: bool,
    #[model(default)]
    /// Whether retained history contains an ambiguity receipt.
    unknown: bool,
    #[model(default)]
    /// Whether retained history already contains a reconciliation result.
    reconciled: bool,
    #[model(default)]
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::struct_excessive_bools,
    reason = "EventCore derives require declarative field ordering and generate the private modeled plumbing"
)]
#[derive(ModelOutput)]
/// Modeled output selected by the record reconciled transition.
struct RecordReconciledDecision {
    /// Prepared mutation identity or preparation-state marker from retained history.
    prepared: Option<RepositoryMutationIdentity>,
    /// Whether retained history contains a definitive dispatch outcome.
    definitive_terminal: bool,
    /// Whether retained history contains an ambiguity receipt.
    unknown: bool,
    /// Whether retained history already contains a reconciliation result.
    reconciled: bool,
    /// Whether retained history violates the lifecycle invariant.
    malformed: bool,
}

mapping! { RecordReconciledStateToDecisionPrepared: RecordReconciledState.prepared => RecordReconciledDecision.prepared using clone; }
mapping! { RecordReconciledStateToDecisionTerminal: RecordReconciledState.definitive_terminal => RecordReconciledDecision.definitive_terminal using copy; }
mapping! { RecordReconciledStateToDecisionUnknown: RecordReconciledState.unknown => RecordReconciledDecision.unknown using copy; }
mapping! { RecordReconciledStateToDecisionReconciled: RecordReconciledState.reconciled => RecordReconciledDecision.reconciled using copy; }
mapping! { RecordReconciledStateToDecisionMalformed: RecordReconciledState.malformed => RecordReconciledDecision.malformed using copy; }
mapping! { RecordReconciledStreamToEvent: RecordReconciled.stream => RepositoryMutationEvent.stream using stream_id; }
mapping! { RecordReconciledToFact: (RecordReconciled.outcome, RecordReconciledDecision.prepared, RecordReconciledDecision.definitive_terminal, RecordReconciledDecision.unknown, RecordReconciledDecision.reconciled, RecordReconciledDecision.malformed) => RepositoryMutationEvent.fact using try reconciled_fact, error = CommandError; }

#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::missing_trait_methods,
    clippy::pattern_type_mismatch,
    reason = "EventCore fixes command method order and supplies checked default discovery plumbing"
)]
impl ModelCommandLogic for RecordReconciled {
    type Event = RepositoryMutationEvent;
    type State = RecordReconciledState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        let view = RepositoryMutationEventView::from_event(event);
        let same_stream = view.as_ref().stream == *self.stream.as_stream_id();
        match &view.as_ref().fact {
            RepositoryMutationFact::Proposed(_)
            | RepositoryMutationFact::Reproposed(_)
            | RepositoryMutationFact::Approved(_) => {
                folded.malformed |= !same_stream || folded.prepared.is_some();
            }
            RepositoryMutationFact::Denied(_) | RepositoryMutationFact::Cancelled(_) => {
                folded.malformed = true;
                folded.definitive_terminal = true;
            }
            RepositoryMutationFact::Prepared(identity) => {
                if !same_stream || folded.prepared.is_some() || folded.definitive_terminal {
                    folded.malformed = true;
                } else {
                    folded.prepared = Some(identity.clone());
                }
            }
            RepositoryMutationFact::Applied(receipt) => {
                folded.malformed |= !same_stream
                    || folded.prepared.as_ref() != Some(receipt.identity())
                    || folded.definitive_terminal
                    || folded.unknown;
                folded.definitive_terminal = true;
            }
            RepositoryMutationFact::Failed(failure) => {
                folded.malformed |= !same_stream
                    || folded.prepared.as_ref() != Some(failure.identity())
                    || folded.definitive_terminal
                    || folded.unknown;
                folded.definitive_terminal = true;
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                folded.malformed |= !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation.identity())
                    || folded.definitive_terminal
                    || folded.unknown
                    || folded.reconciled;
                folded.unknown = true;
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                folded.malformed |= !same_stream
                    || folded.prepared.as_ref() != Some(reconciliation_outcome_identity(outcome))
                    || !folded.unknown
                    || folded.reconciled;
                folded.reconciled = true;
            }
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = RecordReconciledDecision::model_builder()
            .prepared(RecordReconciledStateToDecisionPrepared::apply(
                state.as_ref(),
            ))
            .definitive_terminal(RecordReconciledStateToDecisionTerminal::apply(
                state.as_ref(),
            ))
            .unknown(RecordReconciledStateToDecisionUnknown::apply(
                state.as_ref(),
            ))
            .reconciled(RecordReconciledStateToDecisionReconciled::apply(
                state.as_ref(),
            ))
            .malformed(RecordReconciledStateToDecisionMalformed::apply(
                state.as_ref(),
            ))
            .build();
        Ok(ModeledEvents::one(
            RepositoryMutationEvent::model_builder()
                .stream(RecordReconciledStreamToEvent::apply(self))
                .fact(RecordReconciledToFact::apply((
                    self,
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                    decision.as_ref(),
                ))?)
                .build(),
        ))
    }
}

#[derive(Debug)]
/// Private adapter that preserves a static checked-command rejection message.
struct RepositoryCommandError(&'static str);

impl fmt::Display for RepositoryCommandError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "the private command adapter has no nested source or request-provider metadata"
)]
impl Error for RepositoryCommandError {}

/// One exact durable proposal publication.
pub struct RepositoryMutationPublication {
    /// Exact immutable event selected by checked command logic.
    event: RepositoryMutationEvent,
    /// Owning repository-mutation stream used for optimistic consistency.
    stream: RepositoryMutationStream,
}

impl RepositoryMutationPublication {
    /// Consumes the closed publication into its event and exact consistency fence.
    #[must_use]
    #[inline]
    pub fn into_event_and_consistency_streams(
        self,
    ) -> (RepositoryMutationEvent, [RepositoryMutationStream; 1]) {
        (self.event, [self.stream])
    }
}

/// Clones the exact stream identity consumed by generated command mappings.
fn stream_id(stream: &RepositoryMutationStream) -> StreamId {
    stream.as_stream_id().clone()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the initial durable proposal fact when no proposal is retained.
fn proposed_fact(
    proposal: &RepositoryMutationProposalIdentity,
    already_proposed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *already_proposed {
        return Err(command_error("repository_proposal_already_recorded"));
    }
    Ok(RepositoryMutationFact::Proposed(proposal.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects a replacement proposal fact after validating retained lifecycle identity.
fn reproposed_fact(
    proposal: &RepositoryMutationProposalIdentity,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *terminal {
        return Err(command_error("repository_mutation_already_decided"));
    }
    let Some(retained_proposal) = durable_proposal.as_ref() else {
        return Err(command_error("repository_proposal_missing"));
    };
    if retained_proposal == proposal {
        return Err(command_error("repository_proposal_unchanged"));
    }
    if retained_proposal.provenance() != proposal.provenance() {
        return Err(command_error("repository_workflow_provenance_stale"));
    }
    Ok(RepositoryMutationFact::Reproposed(proposal.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the exact owner-approval fact for the active workflow proposal.
fn approved_fact(
    proposal: &RepositoryMutationProposalIdentity,
    active_provenance: &RepositoryMutationProvenance,
    approval: &OwnerApprovalId,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    decided: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    validate_owner_decision(
        proposal,
        active_provenance,
        durable_proposal,
        *decided,
        *malformed,
    )?;
    Ok(RepositoryMutationFact::Approved(
        RepositoryMutationApprovalFact {
            approval: approval.clone(),
            proposal: proposal.clone(),
        },
    ))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the owner-denial fact for the active workflow proposal.
fn denied_fact(
    proposal: &RepositoryMutationProposalIdentity,
    active_provenance: &RepositoryMutationProvenance,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    decided: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    validate_owner_decision(
        proposal,
        active_provenance,
        durable_proposal,
        *decided,
        *malformed,
    )?;
    Ok(RepositoryMutationFact::Denied(proposal.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the owner-cancellation fact for the active workflow proposal.
fn cancelled_fact(
    proposal: &RepositoryMutationProposalIdentity,
    active_provenance: &RepositoryMutationProvenance,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    decided: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    validate_owner_decision(
        proposal,
        active_provenance,
        durable_proposal,
        *decided,
        *malformed,
    )?;
    Ok(RepositoryMutationFact::Cancelled(proposal.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects preparation authority after validating proposal and approval history.
fn prepared_fact(
    proposal: &RepositoryMutationProposalIdentity,
    identity: &RepositoryMutationIdentity,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    approval: &Option<RepositoryMutationApprovalFact>,
    prepared: &bool,
    terminal: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *terminal {
        return Err(command_error("repository_mutation_already_decided"));
    }
    if *prepared {
        return Err(command_error("repository_mutation_already_prepared"));
    }
    if durable_proposal.as_ref() != Some(proposal) {
        return Err(command_error("repository_proposal_stale"));
    }
    let Some(durable_approval) = approval.as_ref() else {
        return Err(command_error("repository_owner_approval_missing"));
    };
    if durable_approval.proposal() != proposal
        || durable_approval.approval() != identity.owner_approval()
    {
        return Err(command_error("repository_owner_approval_mismatch"));
    }
    if identity.provenance() != proposal.provenance() {
        return Err(command_error("repository_workflow_provenance_stale"));
    }
    Ok(RepositoryMutationFact::Prepared(identity.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the definitive applied receipt for the prepared identity.
fn applied_fact(
    receipt: &RepositoryMutationReceipt,
    prepared: &Option<RepositoryMutationIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *terminal {
        return Err(command_error("repository_terminal_already_recorded"));
    }
    if prepared.as_ref() != Some(receipt.identity()) {
        return Err(command_error("repository_prepared_identity_mismatch"));
    }
    Ok(RepositoryMutationFact::Applied(receipt.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the definitive not-applied receipt for the prepared identity.
fn failed_fact(
    failure: &RepositoryMutationFailure,
    prepared: &Option<RepositoryMutationIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *terminal {
        return Err(command_error("repository_terminal_already_recorded"));
    }
    if prepared.as_ref() != Some(failure.identity()) {
        return Err(command_error("repository_prepared_identity_mismatch"));
    }
    Ok(RepositoryMutationFact::Failed(failure.clone()))
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects the ambiguity receipt for the prepared identity.
fn unknown_fact(
    reconciliation: &RepositoryReconciliation,
    prepared: &Option<RepositoryMutationIdentity>,
    terminal: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *terminal {
        return Err(command_error("repository_terminal_already_recorded"));
    }
    if prepared.as_ref() != Some(reconciliation.identity()) {
        return Err(command_error("repository_prepared_identity_mismatch"));
    }
    Ok(RepositoryMutationFact::Unknown(reconciliation.clone()))
}

#[expect(
    clippy::pattern_type_mismatch,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
/// Returns the prepared identity carried by a reconciliation outcome.
fn reconciliation_outcome_identity(
    outcome: &RepositoryReconciliationOutcome,
) -> &RepositoryMutationIdentity {
    match outcome {
        RepositoryReconciliationOutcome::Applied(receipt)
        | RepositoryReconciliationOutcome::NotApplied(receipt)
        | RepositoryReconciliationOutcome::StillUnknown(receipt) => receipt.identity(),
    }
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping callbacks retain generated reference-shaped inputs at this pure fold boundary"
)]
#[expect(
    clippy::single_call_fn,
    reason = "this pure EventCore mapping callback has exactly one owning command transition"
)]
/// Selects a read-only reconciliation result for the ambiguous prepared identity.
fn reconciled_fact(
    outcome: &RepositoryReconciliationOutcome,
    prepared: &Option<RepositoryMutationIdentity>,
    definitive_terminal: &bool,
    unknown: &bool,
    reconciled: &bool,
    malformed: &bool,
) -> Result<RepositoryMutationFact, CommandError> {
    if *malformed || !*unknown {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if *definitive_terminal {
        return Err(command_error("repository_terminal_already_recorded"));
    }
    if *reconciled {
        return Err(command_error("repository_reconciliation_already_recorded"));
    }
    if prepared.as_ref() != Some(reconciliation_outcome_identity(outcome)) {
        return Err(command_error("repository_prepared_identity_mismatch"));
    }
    Ok(RepositoryMutationFact::Reconciled(outcome.clone()))
}

#[expect(
    clippy::ref_option,
    reason = "the shared owner-decision fold retains EventCore's generated optional reference shape"
)]
/// Validates proposal identity and active workflow provenance before an owner decision.
fn validate_owner_decision(
    proposal: &RepositoryMutationProposalIdentity,
    active_provenance: &RepositoryMutationProvenance,
    durable_proposal: &Option<RepositoryMutationProposalIdentity>,
    decided: bool,
    malformed: bool,
) -> Result<(), CommandError> {
    if malformed {
        return Err(command_error("repository_mutation_history_invalid"));
    }
    if decided {
        return Err(command_error("repository_mutation_already_decided"));
    }
    let Some(retained_proposal) = durable_proposal.as_ref() else {
        return Err(command_error("repository_proposal_missing"));
    };
    if retained_proposal != proposal {
        return Err(command_error("repository_proposal_stale"));
    }
    if proposal.provenance() != active_provenance {
        return Err(command_error("repository_workflow_provenance_stale"));
    }
    Ok(())
}

/// Builds the static error required by `EventCore` command callbacks.
fn command_error(code: &'static str) -> CommandError {
    CommandError::business_rule_violated(RepositoryCommandError(code))
}

/// Maps a checked command rejection into the stable service error vocabulary.
#[expect(
    clippy::shadow_unrelated,
    reason = "the adapter keeps the conventional error binding at its error-mapping boundary"
)]
fn modeled_service_error(error: &CommandError) -> RepositoryMutationServiceError {
    let Some(source) = Error::source(error) else {
        return RepositoryMutationServiceError::ModeledCommandFailed;
    };
    let Some(error) = source.downcast_ref::<RepositoryCommandError>() else {
        return RepositoryMutationServiceError::ModeledCommandFailed;
    };
    match error.0 {
        "repository_proposal_stale" => RepositoryMutationServiceError::StaleProposal,
        "repository_workflow_provenance_stale" => {
            RepositoryMutationServiceError::StaleWorkflowProvenance
        }
        "repository_mutation_history_invalid" => RepositoryMutationServiceError::InvalidHistory,
        "repository_proposal_missing" => RepositoryMutationServiceError::ProposalMissing,
        "repository_mutation_already_decided" => {
            RepositoryMutationServiceError::OwnerDecisionAlreadyRecorded
        }
        "repository_terminal_already_recorded" => {
            RepositoryMutationServiceError::TerminalAlreadyRecorded
        }
        "repository_reconciliation_already_recorded" => {
            RepositoryMutationServiceError::ReconciliationAlreadyRecorded
        }
        _ => RepositoryMutationServiceError::ModeledCommandFailed,
    }
}

/// Decides admission of one exact safe proposal identity.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] if the checked command rejects
/// the proposal or does not emit exactly one event.
#[expect(
    clippy::needless_pass_by_value,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    clippy::wildcard_enum_match_arm,
    reason = "the public pure fold preserves typed ownership and exact retained-history sequencing"
)]
#[inline]
pub fn decide_propose_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: RepositoryMutationProposal,
) -> Result<Option<RepositoryMutationPublication>, RepositoryMutationServiceError> {
    let proposal_identity = proposal.identity();
    let expected_stream = RepositoryMutationStream::new(&proposal_identity)?;
    if stream != expected_stream {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = ProposeMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal_identity.clone())
        .build();
    let command = ProposeMutation::model_builder()
        .stream(ProposeMutationRequestToStream::apply(request.as_ref()))
        .proposal(ProposeMutationRequestToProposal::apply(request.as_ref()))
        .build();
    let mut state: Modeled<ProposeMutationState> = Modeled::default();
    for event in history {
        if event.stream != *stream.as_stream_id() {
            return Err(RepositoryMutationServiceError::InvalidHistory);
        }
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    if !history.is_empty() {
        let [retained] = history else {
            return Err(RepositoryMutationServiceError::InvalidHistory);
        };
        return match retained.fact() {
            RepositoryMutationFact::Proposed(retained) if retained == &proposal_identity => {
                Ok(None)
            }
            RepositoryMutationFact::Proposed(_) => {
                Err(RepositoryMutationServiceError::StaleProposal)
            }
            _ => Err(RepositoryMutationServiceError::InvalidHistory),
        };
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(Some(RepositoryMutationPublication { event, stream }))
}

/// Decides durable replacement of a stale proposal after the caller rereads state.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history is invalid,
/// terminal, missing a proposal, or the replacement changes workflow provenance.
#[expect(
    clippy::needless_pass_by_value,
    clippy::shadow_reuse,
    reason = "the public pure fold preserves typed ownership and exact retained-history sequencing"
)]
#[inline]
pub fn decide_repropose_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: RepositoryMutationProposal,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let proposal = proposal.identity();
    if stream != RepositoryMutationStream::new(&proposal)? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = ReproposeMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal)
        .build();
    let command = ReproposeMutation::model_builder()
        .stream(ReproposeMutationRequestToStream::apply(request.as_ref()))
        .proposal(ReproposeMutationRequestToProposal::apply(request.as_ref()))
        .build();
    let mut state: Modeled<ReproposeMutationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

/// Decides explicit owner approval for the exact durable proposal and active workflow.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history is invalid,
/// the proposal digest is stale, workflow provenance is no longer active, or a
/// terminal owner decision already exists.
#[inline]
pub fn decide_approve_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: RepositoryMutationProposalIdentity,
    active_provenance: RepositoryMutationProvenance,
    approval: OwnerApprovalId,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    if stream != RepositoryMutationStream::new(&proposal)? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = ApproveMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal)
        .active_provenance(active_provenance)
        .approval(approval)
        .build();
    let command = ApproveMutation::model_builder()
        .stream(ApproveMutationRequestToStream::apply(request.as_ref()))
        .proposal(ApproveMutationRequestToProposal::apply(request.as_ref()))
        .active_provenance(ApproveMutationRequestToProvenance::apply(request.as_ref()))
        .approval(ApproveMutationRequestToApproval::apply(request.as_ref()))
        .build();
    let mut state: Modeled<ApproveMutationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

/// Decides owner approval and dispatch preparation as one closed publication batch.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when the exact proposal cannot be
/// approved or the resulting approved history cannot prepare the same mutation.
#[inline]
pub fn decide_approve_and_prepare_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: &RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
    active_provenance: RepositoryMutationProvenance,
    approval_id: OwnerApprovalId,
) -> Result<[RepositoryMutationPublication; 2], RepositoryMutationServiceError> {
    let approval = decide_approve_mutation(
        history,
        stream.clone(),
        proposal.identity(),
        active_provenance,
        approval_id.clone(),
    )?;
    let mut approved_history = history.to_vec();
    approved_history.push(approval.event.clone());
    let prepared = decide_prepare_mutation(
        &approved_history,
        stream,
        proposal,
        assignment,
        policy,
        approval_id,
    )?;
    Ok([approval, prepared])
}

/// Decides explicit owner denial for the exact durable proposal and active workflow.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when history is invalid, proposal
/// identity or workflow provenance is stale, or an owner decision already exists.
#[inline]
pub fn decide_deny_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: RepositoryMutationProposalIdentity,
    active_provenance: RepositoryMutationProvenance,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let request = DenyMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal)
        .active_provenance(active_provenance)
        .build();
    let command = DenyMutation::model_builder()
        .stream(DenyMutationRequestToStream::apply(request.as_ref()))
        .proposal(DenyMutationRequestToProposal::apply(request.as_ref()))
        .active_provenance(DenyMutationRequestToProvenance::apply(request.as_ref()))
        .build();
    let mut state: Modeled<DenyMutationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

/// Decides explicit owner cancellation for the exact durable proposal and active workflow.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when history is invalid, proposal
/// identity or workflow provenance is stale, or an owner decision already exists.
#[inline]
pub fn decide_cancel_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: RepositoryMutationProposalIdentity,
    active_provenance: RepositoryMutationProvenance,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let request = CancelMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal)
        .active_provenance(active_provenance)
        .build();
    let command = CancelMutation::model_builder()
        .stream(CancelMutationRequestToStream::apply(request.as_ref()))
        .proposal(CancelMutationRequestToProposal::apply(request.as_ref()))
        .active_provenance(CancelMutationRequestToProvenance::apply(request.as_ref()))
        .build();
    let mut state: Modeled<CancelMutationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

#[expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::missing_errors_doc,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    clippy::wildcard_enum_match_arm,
    reason = "the public pure fold preserves typed ownership and exact retained-history sequencing"
)]
#[inline]
pub fn decide_cancel_open_proposal_on_restart(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
) -> Result<Option<RepositoryMutationPublication>, RepositoryMutationServiceError> {
    let proposal = history.iter().rev().find_map(|event| match event.fact() {
        RepositoryMutationFact::Proposed(proposal)
        | RepositoryMutationFact::Reproposed(proposal) => Some(proposal.clone()),
        _ => None,
    });

    let Some(proposal) = proposal else {
        return if history.is_empty() {
            Ok(None)
        } else {
            Err(RepositoryMutationServiceError::InvalidHistory)
        };
    };

    if stream != RepositoryMutationStream::new(&proposal)? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }

    let provenance = proposal.provenance().clone();
    let owner_decision = history.iter().position(|event| {
        matches!(
            event.fact(),
            RepositoryMutationFact::Approved(_)
                | RepositoryMutationFact::Denied(_)
                | RepositoryMutationFact::Cancelled(_)
        )
    });
    let Some(owner_decision) = owner_decision else {
        return decide_cancel_mutation(history, stream, proposal, provenance).map(Some);
    };

    match decide_cancel_mutation(
        &history[..=owner_decision],
        stream.clone(),
        proposal,
        provenance,
    ) {
        Err(RepositoryMutationServiceError::OwnerDecisionAlreadyRecorded) => {}
        Err(error) => return Err(error),
        Ok(_publication) => return Err(RepositoryMutationServiceError::InvalidHistory),
    }

    let approved = matches!(
        history[owner_decision].fact(),
        RepositoryMutationFact::Approved(_)
    );
    let dispatch_history = &history[owner_decision + 1..];
    if !approved {
        return if dispatch_history.is_empty() {
            Ok(None)
        } else {
            Err(RepositoryMutationServiceError::InvalidHistory)
        };
    }
    if !dispatch_history.iter().all(|event| {
        matches!(
            event.fact(),
            RepositoryMutationFact::Prepared(_)
                | RepositoryMutationFact::Applied(_)
                | RepositoryMutationFact::Failed(_)
                | RepositoryMutationFact::Unknown(_)
                | RepositoryMutationFact::Reconciled(_)
        )
    }) {
        return Err(RepositoryMutationServiceError::InvalidHistory);
    }

    recover_prepared_from_history(history, &stream)?;
    Ok(None)
}

/// Decides durable preparation and returns the one-shot adapter authority that
/// becomes usable only after the returned publication is durably committed.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history or pure
/// repository policy rejects the exact approved proposal.
#[inline]
pub fn decide_prepare_mutation(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    proposal: &RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
    approval_id: OwnerApprovalId,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let proposal_identity = proposal.identity();
    if stream != RepositoryMutationStream::new(&proposal_identity)? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let identity = prepare_mutation_identity(proposal, assignment, policy, approval_id)
        .map_err(|_source| RepositoryMutationServiceError::AuthorizationRejected)?;
    let request = PrepareMutationRequest::model_builder()
        .stream(stream.clone())
        .proposal(proposal_identity)
        .identity(identity)
        .build();
    let command = PrepareMutation::model_builder()
        .stream(PrepareMutationRequestToStream::apply(request.as_ref()))
        .proposal(PrepareMutationRequestToProposal::apply(request.as_ref()))
        .identity(PrepareMutationRequestToIdentity::apply(request.as_ref()))
        .build();
    let mut state: Modeled<PrepareMutationState> = Modeled::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(&command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

/// Mints one-shot adapter authority only from an exact verified prepared history.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] unless history contains the exact
/// proposed, approved, and prepared chain for the supplied raw proposal with
/// no dispatched outcome.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "the public pure fold preserves typed ownership and exact retained-history sequencing"
)]
#[inline]
pub fn authorize_prepared_mutation(
    history: &[RepositoryMutationEvent],
    proposal: RepositoryMutationProposal,
    assignment: &RepositoryAssignmentContext,
    policy: &RepositoryMutationPolicy,
) -> Result<AuthorizedRepositoryMutation, RepositoryMutationServiceError> {
    let prepared = history
        .iter()
        .rev()
        .find_map(|event| match event.fact() {
            RepositoryMutationFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .ok_or(RepositoryMutationServiceError::InvalidHistory)?;
    validate_dispatch_authorization_history(history, &prepared)?;
    authorize_core_prepared_mutation(proposal, assignment, policy, &prepared)
        .map_err(|_source| RepositoryMutationServiceError::AuthorizationRejected)
}

/// Decides the definitive applied terminal publication for one prepared mutation.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history is invalid,
/// the receipt belongs to another effect stream, or a terminal already exists.
#[inline]
pub fn decide_record_applied(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    receipt: RepositoryMutationReceipt,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let identity = receipt.identity().clone();
    if stream != RepositoryMutationStream::for_provenance(identity.provenance())? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = RecordAppliedRequest::model_builder()
        .stream(stream.clone())
        .receipt(receipt)
        .build();
    let command = RecordApplied::model_builder()
        .stream(RecordAppliedRequestToStream::apply(request.as_ref()))
        .receipt(RecordAppliedRequestToReceipt::apply(request.as_ref()))
        .build();
    decide_terminal_publication(history, stream, &identity, &command)
}

/// Decides the definitive failed terminal publication for one prepared mutation.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history is invalid,
/// the failure belongs to another effect stream, or a terminal already exists.
#[inline]
pub fn decide_record_failed(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    failure: RepositoryMutationFailure,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let identity = failure.identity().clone();
    if stream != RepositoryMutationStream::for_provenance(identity.provenance())? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = RecordFailedRequest::model_builder()
        .stream(stream.clone())
        .failure(failure)
        .build();
    let command = RecordFailed::model_builder()
        .stream(RecordFailedRequestToStream::apply(request.as_ref()))
        .failure(RecordFailedRequestToFailure::apply(request.as_ref()))
        .build();
    decide_terminal_publication(history, stream, &identity, &command)
}

/// Decides the ambiguous terminal publication for one prepared mutation.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when retained history is invalid,
/// the handle belongs to another effect stream, or a terminal already exists.
#[inline]
pub fn decide_record_unknown(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    reconciliation: RepositoryReconciliation,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let identity = reconciliation.identity().clone();
    if stream != RepositoryMutationStream::for_provenance(identity.provenance())? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = RecordUnknownRequest::model_builder()
        .stream(stream.clone())
        .reconciliation(reconciliation)
        .build();
    let command = RecordUnknown::model_builder()
        .stream(RecordUnknownRequestToStream::apply(request.as_ref()))
        .reconciliation(RecordUnknownRequestToReconciliation::apply(
            request.as_ref(),
        ))
        .build();
    decide_terminal_publication(history, stream, &identity, &command)
}

/// Runs one checked terminal command against retained mutation history.
fn decide_terminal_publication<Command, Logic, State>(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    identity: &RepositoryMutationIdentity,
    command: &Command,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError>
where
    Command: AsRef<Logic> + CommandLogic<Event = RepositoryMutationEvent, State = Modeled<State>>,
    Logic: ModelCommandLogic<Event = RepositoryMutationEvent, State = State>,
    Modeled<State>: Default,
{
    validate_reconcilable_history(history, identity)?;
    let mut state = Modeled::<State>::default();
    for event in history {
        state = ModelCommandLogic::evolve(command.as_ref(), state, event);
    }
    let events: Vec<RepositoryMutationEvent> = CommandLogic::handle(command, state)
        .map_err(|source| modeled_service_error(&source))?
        .into();
    let [event] = events
        .try_into()
        .map_err(|_events| RepositoryMutationServiceError::InvalidModeledEmission)?;
    Ok(RepositoryMutationPublication { event, stream })
}

/// Validates the proposal-through-preparation prefix used by terminal commands.
#[expect(
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    reason = "the fold matches borrowed facts and deliberately narrows the accumulated proposal"
)]
fn validate_reconcilable_history(
    history: &[RepositoryMutationEvent],
    identity: &RepositoryMutationIdentity,
) -> Result<(), RepositoryMutationServiceError> {
    let mut proposal = None;
    let mut approved = false;
    let mut prepared = false;
    let mut unknown = false;
    for event in history {
        match event.fact() {
            RepositoryMutationFact::Proposed(candidate) => {
                if proposal.is_some() || approved || prepared || unknown {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                proposal = Some(candidate);
            }
            RepositoryMutationFact::Reproposed(candidate) => {
                if proposal.is_none() || prepared || unknown {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                proposal = Some(candidate);
                approved = false;
            }
            RepositoryMutationFact::Approved(approval) => {
                if proposal != Some(approval.proposal())
                    || approved
                    || prepared
                    || unknown
                    || approval.approval() != identity.owner_approval()
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                approved = true;
            }
            RepositoryMutationFact::Prepared(candidate) => {
                if !approved
                    || prepared
                    || unknown
                    || candidate != identity
                    || proposal.is_none_or(|proposal| !candidate.matches_proposal(proposal))
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                prepared = true;
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if !prepared || unknown || reconciliation.identity() != identity {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                unknown = true;
            }
            RepositoryMutationFact::Denied(_)
            | RepositoryMutationFact::Cancelled(_)
            | RepositoryMutationFact::Applied(_)
            | RepositoryMutationFact::Failed(_)
            | RepositoryMutationFact::Reconciled(_) => {
                return Err(RepositoryMutationServiceError::InvalidHistory);
            }
        }
    }
    if proposal.is_none() || !approved || !prepared {
        return Err(RepositoryMutationServiceError::InvalidHistory);
    }
    Ok(())
}

/// Validates the exact proposal, approval, and preparation authorization chain.
#[expect(
    clippy::single_call_fn,
    reason = "the authorization fold is owned by one dispatch boundary and matches borrowed facts"
)]
fn validate_dispatch_authorization_history(
    history: &[RepositoryMutationEvent],
    identity: &RepositoryMutationIdentity,
) -> Result<(), RepositoryMutationServiceError> {
    validate_reconcilable_history(history, identity)?;
    if history
        .iter()
        .any(|event| matches!(event.fact(), RepositoryMutationFact::Unknown(_)))
    {
        return Err(RepositoryMutationServiceError::InvalidHistory);
    }
    Ok(())
}

/// Recovers read-only reconciliation authority solely from verified durable history.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when the supplied history crosses
/// streams or violates prepared/terminal identity ordering.
#[inline]
pub fn recover_prepared_from_history(
    history: &[RepositoryMutationEvent],
    stream: &RepositoryMutationStream,
) -> Result<Option<RepositoryReconciliation>, RepositoryMutationServiceError> {
    let mut prepared: Option<RepositoryMutationIdentity> = None;
    let mut unknown: Option<RepositoryReconciliation> = None;
    let mut definitive_terminal = false;
    let mut reconciled = false;
    for event in history {
        if event.stream != *stream.as_stream_id() {
            return Err(RepositoryMutationServiceError::InvalidHistory);
        }
        match event.fact().clone() {
            RepositoryMutationFact::Prepared(identity) => {
                if prepared.is_some() || definitive_terminal || unknown.is_some() || reconciled {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                let identity_stream =
                    RepositoryMutationStream::for_provenance(identity.provenance())
                        .map_err(|_source| RepositoryMutationServiceError::InvalidHistory)?;
                if identity_stream != *stream {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                prepared = Some(identity.clone());
            }
            RepositoryMutationFact::Applied(receipt) => {
                if prepared.as_ref() != Some(receipt.identity())
                    || definitive_terminal
                    || unknown.is_some()
                    || reconciled
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                definitive_terminal = true;
            }
            RepositoryMutationFact::Failed(failure) => {
                if prepared.as_ref() != Some(failure.identity())
                    || definitive_terminal
                    || unknown.is_some()
                    || reconciled
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                definitive_terminal = true;
            }
            RepositoryMutationFact::Unknown(reconciliation) => {
                if prepared.as_ref() != Some(reconciliation.identity())
                    || definitive_terminal
                    || unknown.is_some()
                    || reconciled
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                unknown = Some(reconciliation.clone());
            }
            RepositoryMutationFact::Reconciled(outcome) => {
                if unknown.as_ref().map(RepositoryReconciliation::identity)
                    != Some(reconciliation_outcome_identity(&outcome))
                    || definitive_terminal
                    || reconciled
                {
                    return Err(RepositoryMutationServiceError::InvalidHistory);
                }
                reconciled = true;
            }
            RepositoryMutationFact::Proposed(_)
            | RepositoryMutationFact::Reproposed(_)
            | RepositoryMutationFact::Approved(_)
            | RepositoryMutationFact::Denied(_)
            | RepositoryMutationFact::Cancelled(_) => {}
        }
    }
    let Some(identity) = prepared.as_ref() else {
        return Ok(None);
    };
    if definitive_terminal || reconciled {
        let Some((terminal, lifecycle)) = history.split_last() else {
            return Err(RepositoryMutationServiceError::InvalidHistory);
        };
        let expected_terminal = if definitive_terminal {
            matches!(
                terminal.fact(),
                RepositoryMutationFact::Applied(_) | RepositoryMutationFact::Failed(_)
            )
        } else {
            matches!(terminal.fact(), RepositoryMutationFact::Reconciled(_))
        };
        if !expected_terminal {
            return Err(RepositoryMutationServiceError::InvalidHistory);
        }
        validate_reconcilable_history(lifecycle, identity)?;
        return Ok(None);
    }
    validate_reconcilable_history(history, identity)?;
    Ok(unknown.or_else(|| prepared.map(RepositoryReconciliation::from_durable_identity)))
}

/// Decides exactly one signed read-only reconciliation result publication.
///
/// # Errors
///
/// Returns [`RepositoryMutationServiceError`] when the outcome does not match
/// exact prepared history or reconciliation was already recorded.
#[inline]
pub fn decide_record_reconciled(
    history: &[RepositoryMutationEvent],
    stream: RepositoryMutationStream,
    outcome: RepositoryReconciliationOutcome,
) -> Result<RepositoryMutationPublication, RepositoryMutationServiceError> {
    let identity = reconciliation_outcome_identity(&outcome).clone();
    if stream != RepositoryMutationStream::for_provenance(identity.provenance())? {
        return Err(RepositoryMutationServiceError::StreamProposalMismatch);
    }
    let request = RecordReconciledRequest::model_builder()
        .stream(stream.clone())
        .outcome(outcome)
        .build();
    let command = RecordReconciled::model_builder()
        .stream(RecordReconciledRequestToStream::apply(request.as_ref()))
        .outcome(RecordReconciledRequestToOutcome::apply(request.as_ref()))
        .build();
    decide_terminal_publication(history, stream, &identity, &command)
}
