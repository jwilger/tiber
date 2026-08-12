//! Finding-resolution intent with only the named blocking occurrence's
//! acceptance and resolution status folded for the decision.

#![expect(
    clippy::missing_trait_methods,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping signatures and static stream discovery are defined by the checked-model API"
)]

use eventcore::{
    CommandError, ModelCommand, ModelInput, ModelOutput, ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity as _},
};

use crate::types::{
    AssignmentId, AssignmentKind, AssignmentResult, EvidenceId, FindingOccurrenceId,
    FindingSeverity, ReviewAssignment,
};
use crate::{__eventcore_model_reviewevent, ReviewEvent, ReviewFact, ReviewStream};

#[derive(ModelInput)]
struct VerifyFindingResolutionRequest {
    #[model(origin)]
    stream: ReviewStream,
    #[model(origin)]
    finding_id: FindingOccurrenceId,
    #[model(origin)]
    remediation_assignment_id: AssignmentId,
}

/// Verifies the resolution of one previously accepted blocking occurrence.
#[derive(ModelCommand)]
struct VerifyFindingResolution {
    #[stream]
    stream: ReviewStream,
    finding_id: FindingOccurrenceId,
    remediation_assignment_id: AssignmentId,
}

mapping! { VerifyResolutionRequestToStream: VerifyFindingResolutionRequest.stream => VerifyFindingResolution.stream using clone; }
mapping! { VerifyResolutionRequestToFinding: VerifyFindingResolutionRequest.finding_id => VerifyFindingResolution.finding_id using clone; }
mapping! { VerifyResolutionRequestToAssignment: VerifyFindingResolutionRequest.remediation_assignment_id => VerifyFindingResolution.remediation_assignment_id using clone; }

#[derive(ModelState)]
struct VerifyFindingResolutionState {
    #[model(default)]
    blocking_occurrence_accepted: bool,
    #[model(default)]
    already_resolved: bool,
    #[model(default)]
    remediation_authority: Option<IssuedRemediationAuthority>,
    #[model(default)]
    remediation_evidence: Option<AcceptedRemediationEvidence>,
}

#[derive(Clone)]
struct IssuedRemediationAuthority {
    assignment_id: AssignmentId,
    finding_target: FindingOccurrenceId,
}

impl IssuedRemediationAuthority {
    fn from_assignment(assignment: &ReviewAssignment) -> Option<Self> {
        let finding_target = assignment.finding_target()?.clone();
        if assignment.id().kind() != AssignmentKind::RemediationVerifier
            || assignment.id().remediation_occurrence() != Some(&finding_target)
        {
            return None;
        }
        Some(Self {
            assignment_id: assignment.id().clone(),
            finding_target,
        })
    }
}

#[derive(Clone)]
struct AcceptedRemediationEvidence {
    assignment_id: AssignmentId,
    evidence_id: EvidenceId,
}

impl AcceptedRemediationEvidence {
    fn from_result(result: &AssignmentResult) -> Self {
        Self {
            assignment_id: result.assignment_id().clone(),
            evidence_id: result.evidence_id().clone(),
        }
    }
}

#[derive(Clone)]
struct ResolutionContext {
    blocking_occurrence_accepted: bool,
    already_resolved: bool,
    remediation_authority: Option<IssuedRemediationAuthority>,
    remediation_evidence: Option<AcceptedRemediationEvidence>,
}

#[derive(ModelOutput)]
struct ResolutionDecision {
    context: ResolutionContext,
}

fn resolution_context(
    accepted: &bool,
    resolved: &bool,
    authority: &Option<IssuedRemediationAuthority>,
    evidence: &Option<AcceptedRemediationEvidence>,
) -> ResolutionContext {
    ResolutionContext {
        blocking_occurrence_accepted: *accepted,
        already_resolved: *resolved,
        remediation_authority: authority.clone(),
        remediation_evidence: evidence.clone(),
    }
}
mapping! { ResolutionStateToDecision: (VerifyFindingResolutionState.blocking_occurrence_accepted, VerifyFindingResolutionState.already_resolved, VerifyFindingResolutionState.remediation_authority, VerifyFindingResolutionState.remediation_evidence) => ResolutionDecision.context using resolution_context; }

fn resolution_stream(stream: &ReviewStream) -> StreamId {
    stream.as_stream_id().clone()
}
mapping! { ResolutionStreamToEvent: VerifyFindingResolution.stream => ReviewEvent.stream using resolution_stream; }

fn resolution_fact(
    finding_id: &FindingOccurrenceId,
    context: &ResolutionContext,
) -> Result<ReviewFact, CommandError> {
    let evidence = context.remediation_evidence.as_ref().ok_or_else(|| {
        CommandError::ValidationError("review_remediation_result_required".to_owned())
    })?;
    Ok(ReviewFact::FindingResolutionVerified {
        finding_id: finding_id.clone(),
        evidence_id: evidence.evidence_id.clone(),
    })
}
mapping! { ResolutionToFact: (VerifyFindingResolution.finding_id, ResolutionDecision.context) => ReviewEvent.fact using try resolution_fact, error = CommandError; }

impl ModelCommandLogic for VerifyFindingResolution {
    type Event = ReviewEvent;
    type State = VerifyFindingResolutionState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        match &event.fact {
            ReviewFact::AssignmentResultAccepted { result } => {
                folded.blocking_occurrence_accepted |= result.findings().iter().any(|finding| {
                    finding.id() == &self.finding_id
                        && finding.severity() == FindingSeverity::Blocking
                });
                if result.assignment_id() == &self.remediation_assignment_id {
                    folded.remediation_evidence =
                        Some(AcceptedRemediationEvidence::from_result(result));
                }
            }
            ReviewFact::AssignmentIssued { assignment }
                if assignment.id() == &self.remediation_assignment_id =>
            {
                if let Some(authority) = IssuedRemediationAuthority::from_assignment(assignment)
                    && authority.finding_target == self.finding_id
                {
                    folded.remediation_authority = Some(authority);
                }
            }
            ReviewFact::DeltaReassessed {
                affected_lenses, ..
            } if affected_lenses.contains(self.finding_id.assignment_id().lens()) => {
                folded.blocking_occurrence_accepted = false;
                folded.remediation_authority = None;
                folded.remediation_evidence = None;
            }
            ReviewFact::FindingResolutionVerified { finding_id, .. }
                if finding_id == &self.finding_id =>
            {
                folded.already_resolved = true;
            }
            ReviewFact::RiskAssessed { .. }
            | ReviewFact::AssignmentIssued { .. }
            | ReviewFact::AssignmentSuperseded { .. }
            | ReviewFact::FindingResolutionVerified { .. }
            | ReviewFact::DeltaReassessed { .. }
            | ReviewFact::CleanReviewAccepted { .. } => {}
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = ResolutionDecision::model_builder()
            .context(ResolutionStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let context = &decision.as_ref().context;
        if !context.blocking_occurrence_accepted {
            return Err(CommandError::ValidationError(
                "review_blocking_finding_not_accepted".to_owned(),
            ));
        }
        if context.already_resolved {
            return Err(CommandError::ValidationError(
                "review_finding_already_resolved".to_owned(),
            ));
        }
        let authority = context.remediation_authority.as_ref().ok_or_else(|| {
            CommandError::ValidationError("review_remediation_assignment_required".to_owned())
        })?;
        let evidence = context.remediation_evidence.as_ref().ok_or_else(|| {
            CommandError::ValidationError("review_remediation_result_required".to_owned())
        })?;
        if authority.assignment_id != evidence.assignment_id
            || authority.assignment_id != self.remediation_assignment_id
            || authority.assignment_id.remediation_occurrence() != Some(&self.finding_id)
            || authority.finding_target != self.finding_id
        {
            return Err(CommandError::ValidationError(
                "review_remediation_provenance_mismatch".to_owned(),
            ));
        }
        Ok(ModeledEvents::one(
            ReviewEvent::model_builder()
                .stream(ResolutionStreamToEvent::apply(self))
                .fact(ResolutionToFact::apply((self, decision.as_ref()))?)
                .build(),
        ))
    }
}

/// Builds the checked blocking-finding resolution command.
#[must_use]
pub fn verify_finding_resolution(
    stream: ReviewStream,
    finding_id: FindingOccurrenceId,
    remediation_assignment_id: AssignmentId,
) -> impl eventcore::CommandLogic<Event = ReviewEvent> {
    let request = VerifyFindingResolutionRequest::model_builder()
        .stream(stream)
        .finding_id(finding_id)
        .remediation_assignment_id(remediation_assignment_id)
        .build();
    VerifyFindingResolution::model_builder()
        .stream(VerifyResolutionRequestToStream::apply(request.as_ref()))
        .finding_id(VerifyResolutionRequestToFinding::apply(request.as_ref()))
        .remediation_assignment_id(VerifyResolutionRequestToAssignment::apply(request.as_ref()))
        .build()
}
