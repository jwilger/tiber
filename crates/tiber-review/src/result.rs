//! Result-acceptance intent with only the matching issued assignment and its
//! completion status folded for the decision.

#![expect(
    clippy::missing_trait_methods,
    clippy::ref_option,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping signatures and static stream discovery are defined by the checked-model API"
)]

use alloc::collections::BTreeSet;

use eventcore::{
    CommandError, ModelCommand, ModelInput, ModelOutput, ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity as _},
};

use crate::types::{
    AgentId, AssignmentId, ContextReceiptId, FindingOccurrence, LifecycleReceiptId, ModelRole,
    ReviewSnapshotId,
};
use crate::{
    __eventcore_model_reviewevent, AssignmentKind, AssignmentResult, ReviewAssignment, ReviewEvent,
    ReviewFact, ReviewLens, ReviewStream,
};

#[derive(ModelInput)]
struct AcceptAssignmentResultRequest {
    #[model(origin)]
    stream: ReviewStream,
    #[model(origin)]
    result: AssignmentResult,
}

/// Accepts one result with exact scheduler-issued provenance.
#[derive(ModelCommand)]
struct AcceptAssignmentResult {
    #[stream]
    stream: ReviewStream,
    result: AssignmentResult,
}

mapping! { AcceptResultRequestToStream: AcceptAssignmentResultRequest.stream => AcceptAssignmentResult.stream using clone; }
mapping! { AcceptResultRequestToResult: AcceptAssignmentResultRequest.result => AcceptAssignmentResult.result using clone; }

#[derive(ModelState)]
struct AcceptAssignmentResultState {
    #[model(default)]
    assignment_authority: Option<IssuedAssignmentAuthority>,
    #[model(default)]
    assessed_lenses: Option<BTreeSet<ReviewLens>>,
    #[model(default)]
    already_completed: bool,
    #[model(default)]
    superseded: bool,
    #[model(default)]
    invalidated_by_delta: bool,
}

#[derive(Clone)]
struct IssuedAssignmentAuthority {
    assignment_id: AssignmentId,
    source_snapshot: ReviewSnapshotId,
    agent_id: AgentId,
    model_role: ModelRole,
    context_receipt: ContextReceiptId,
    lifecycle_receipt: LifecycleReceiptId,
}

impl IssuedAssignmentAuthority {
    fn from_assignment(assignment: &ReviewAssignment) -> Self {
        Self {
            assignment_id: assignment.id().clone(),
            source_snapshot: assignment.snapshot().clone(),
            agent_id: assignment.agent_id().clone(),
            model_role: assignment.model_role().clone(),
            context_receipt: assignment.context_receipt().clone(),
            lifecycle_receipt: assignment.lifecycle_receipt().clone(),
        }
    }
}

#[derive(Clone)]
struct AcceptResultContext {
    assignment_authority: Option<IssuedAssignmentAuthority>,
    assessed_lenses: Option<BTreeSet<ReviewLens>>,
    already_completed: bool,
    superseded: bool,
    invalidated_by_delta: bool,
}

#[derive(ModelOutput)]
struct AcceptResultDecision {
    context: AcceptResultContext,
}

fn result_context(
    assignment_authority: &Option<IssuedAssignmentAuthority>,
    assessed_lenses: &Option<BTreeSet<ReviewLens>>,
    completed: &bool,
    superseded: &bool,
    invalidated_by_delta: &bool,
) -> AcceptResultContext {
    AcceptResultContext {
        assignment_authority: assignment_authority.clone(),
        assessed_lenses: assessed_lenses.clone(),
        already_completed: *completed,
        superseded: *superseded,
        invalidated_by_delta: *invalidated_by_delta,
    }
}
mapping! { AcceptResultStateToDecision: (AcceptAssignmentResultState.assignment_authority, AcceptAssignmentResultState.assessed_lenses, AcceptAssignmentResultState.already_completed, AcceptAssignmentResultState.superseded, AcceptAssignmentResultState.invalidated_by_delta) => AcceptResultDecision.context using result_context; }

fn result_stream(stream: &ReviewStream) -> StreamId {
    stream.as_stream_id().clone()
}
mapping! { AcceptResultStreamToEvent: AcceptAssignmentResult.stream => ReviewEvent.stream using result_stream; }

fn accepted_result_fact(
    result: &AssignmentResult,
    context: &AcceptResultContext,
) -> Result<ReviewFact, CommandError> {
    if context.assignment_authority.is_none()
        || context.already_completed
        || context.superseded
        || context.invalidated_by_delta
    {
        return Err(CommandError::ValidationError(
            "review_result_not_authorized".to_owned(),
        ));
    }
    Ok(ReviewFact::AssignmentResultAccepted {
        result: result.clone(),
    })
}
mapping! { AcceptResultToFact: (AcceptAssignmentResult.result, AcceptResultDecision.context) => ReviewEvent.fact using try accepted_result_fact, error = CommandError; }

fn validate_delta_classifications(
    assignment_authority: &IssuedAssignmentAuthority,
    result: &AssignmentResult,
    context: &AcceptResultContext,
) -> Result<(), CommandError> {
    match assignment_authority.assignment_id.kind() {
        AssignmentKind::DeltaRisk => {
            let assessed_lenses = context.assessed_lenses.as_ref().ok_or_else(|| {
                CommandError::ValidationError("review_risk_assessment_required".to_owned())
            })?;
            let classified_lenses = result
                .delta_classifications()
                .iter()
                .map(|classification| classification.lens().clone())
                .collect::<BTreeSet<_>>();
            if classified_lenses.len() != result.delta_classifications().len()
                || classified_lenses != *assessed_lenses
            {
                return Err(CommandError::ValidationError(
                    "review_delta_classification_incomplete".to_owned(),
                ));
            }
        }
        AssignmentKind::Lens | AssignmentKind::Verifier | AssignmentKind::RemediationVerifier
            if !result.delta_classifications().is_empty() =>
        {
            return Err(CommandError::ValidationError(
                "review_delta_classifications_not_permitted".to_owned(),
            ));
        }
        AssignmentKind::Lens | AssignmentKind::Verifier | AssignmentKind::RemediationVerifier => {}
    }
    Ok(())
}

impl ModelCommandLogic for AcceptAssignmentResult {
    type Event = ReviewEvent;
    type State = AcceptAssignmentResultState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        match &event.fact {
            ReviewFact::RiskAssessed { assessment, .. } => {
                folded.assessed_lenses = Some(
                    assessment
                        .routes()
                        .iter()
                        .map(|route| route.lens().clone())
                        .collect(),
                );
            }
            ReviewFact::AssignmentIssued { assignment }
                if assignment.id() == self.result.assignment_id() =>
            {
                folded.assignment_authority =
                    Some(IssuedAssignmentAuthority::from_assignment(assignment));
                folded.invalidated_by_delta = false;
            }
            ReviewFact::AssignmentResultAccepted { result }
                if result.assignment_id() == self.result.assignment_id() =>
            {
                folded.already_completed = true;
            }
            ReviewFact::AssignmentSuperseded { assignment_id, .. }
                if assignment_id == self.result.assignment_id() =>
            {
                folded.superseded = true;
            }
            ReviewFact::DeltaReassessed { from_snapshot, .. }
                if !folded.already_completed
                    && folded
                        .assignment_authority
                        .as_ref()
                        .is_some_and(|authority| authority.source_snapshot == *from_snapshot) =>
            {
                folded.invalidated_by_delta = true;
            }
            ReviewFact::AssignmentIssued { .. }
            | ReviewFact::AssignmentResultAccepted { .. }
            | ReviewFact::AssignmentSuperseded { .. }
            | ReviewFact::DeltaReassessed { .. }
            | ReviewFact::FindingResolutionVerified { .. }
            | ReviewFact::CleanReviewAccepted { .. } => {}
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = AcceptResultDecision::model_builder()
            .context(AcceptResultStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let context = &decision.as_ref().context;
        let assignment_authority = context.assignment_authority.as_ref().ok_or_else(|| {
            CommandError::ValidationError("review_assignment_not_issued".to_owned())
        })?;
        if context.already_completed {
            return Err(CommandError::ValidationError(
                "review_assignment_already_completed".to_owned(),
            ));
        }
        if context.superseded {
            return Err(CommandError::ValidationError(
                "review_assignment_superseded".to_owned(),
            ));
        }
        if context.invalidated_by_delta {
            return Err(CommandError::ValidationError(
                "review_assignment_invalidated_by_delta".to_owned(),
            ));
        }
        if assignment_authority.assignment_id != *self.result.assignment_id()
            || assignment_authority.source_snapshot != *self.result.snapshot()
            || assignment_authority.agent_id != *self.result.agent_id()
            || assignment_authority.model_role != *self.result.model_role()
            || assignment_authority.context_receipt != *self.result.context_receipt()
            || assignment_authority.lifecycle_receipt != *self.result.lifecycle_receipt()
        {
            return Err(CommandError::ValidationError(
                "review_assignment_provenance_mismatch".to_owned(),
            ));
        }
        if !self.result.findings().is_empty()
            && matches!(
                assignment_authority.assignment_id.kind(),
                AssignmentKind::DeltaRisk | AssignmentKind::RemediationVerifier
            )
        {
            return Err(CommandError::ValidationError(
                "review_assignment_findings_not_permitted".to_owned(),
            ));
        }
        validate_delta_classifications(assignment_authority, &self.result, context)?;
        let finding_ids = self
            .result
            .findings()
            .iter()
            .map(FindingOccurrence::id)
            .collect::<BTreeSet<_>>();
        if finding_ids.len() != self.result.findings().len()
            || self
                .result
                .findings()
                .iter()
                .any(|finding| finding.id().assignment_id() != self.result.assignment_id())
        {
            return Err(CommandError::ValidationError(
                "review_finding_identity_invalid".to_owned(),
            ));
        }
        Ok(ModeledEvents::one(
            ReviewEvent::model_builder()
                .stream(AcceptResultStreamToEvent::apply(self))
                .fact(AcceptResultToFact::apply((self, decision.as_ref()))?)
                .build(),
        ))
    }
}

/// Builds the checked result-acceptance command.
#[must_use]
pub fn accept_assignment_result(
    stream: ReviewStream,
    result: AssignmentResult,
) -> impl eventcore::CommandLogic<Event = ReviewEvent> {
    let request = AcceptAssignmentResultRequest::model_builder()
        .stream(stream)
        .result(result)
        .build();
    AcceptAssignmentResult::model_builder()
        .stream(AcceptResultRequestToStream::apply(request.as_ref()))
        .result(AcceptResultRequestToResult::apply(request.as_ref()))
        .build()
}
