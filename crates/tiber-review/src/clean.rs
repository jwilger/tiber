//! Clean-review acceptance as a business-domain intent with only the evidence
//! needed to decide that intent folded from durable review facts.

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

use crate::types::FindingSeverity;
use crate::{
    __eventcore_model_reviewevent, AssignmentKind, EvidenceId, FindingOccurrenceId, ReviewEvent,
    ReviewFact, ReviewLens, ReviewSnapshotId, ReviewStream, VerifierRoute,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RequiredWork {
    lens: ReviewLens,
    kind: AssignmentKind,
}

impl RequiredWork {
    const fn new(lens: ReviewLens, kind: AssignmentKind) -> Self {
        Self { lens, kind }
    }
}

/// The only `DeltaRisk` authority this command needs: the declared source-content
/// transition which still fences clean acceptance.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveDeltaTransition {
    from_snapshot: ReviewSnapshotId,
    to_snapshot: Option<ReviewSnapshotId>,
}

#[derive(ModelInput)]
struct AcceptCleanReviewRequest {
    #[model(origin)]
    stream: ReviewStream,
    #[model(origin)]
    snapshot: ReviewSnapshotId,
    #[model(origin)]
    evidence_id: EvidenceId,
}

/// Accepts an exact source snapshot only after all routed work is current and
/// every blocking finding occurrence has separately verified resolution.
#[derive(ModelCommand)]
struct AcceptCleanReview {
    #[stream]
    stream: ReviewStream,
    snapshot: ReviewSnapshotId,
    evidence_id: EvidenceId,
}

mapping! { AcceptCleanRequestToStream: AcceptCleanReviewRequest.stream => AcceptCleanReview.stream using clone; }
mapping! { AcceptCleanRequestToSnapshot: AcceptCleanReviewRequest.snapshot => AcceptCleanReview.snapshot using clone; }
mapping! { AcceptCleanRequestToEvidence: AcceptCleanReviewRequest.evidence_id => AcceptCleanReview.evidence_id using clone; }

#[derive(ModelState)]
struct AcceptCleanReviewState {
    #[model(default)]
    current_snapshot: Option<ReviewSnapshotId>,
    #[model(default)]
    required_work: BTreeSet<RequiredWork>,
    #[model(default)]
    accepted_work: BTreeSet<RequiredWork>,
    #[model(default)]
    unresolved_blockers: BTreeSet<FindingOccurrenceId>,
    #[model(default)]
    active_delta_transition: Option<ActiveDeltaTransition>,
    #[model(default)]
    already_clean: bool,
}

#[derive(Clone)]
struct CleanReviewContext {
    current_snapshot: Option<ReviewSnapshotId>,
    required_work: BTreeSet<RequiredWork>,
    accepted_work: BTreeSet<RequiredWork>,
    unresolved_blockers: BTreeSet<FindingOccurrenceId>,
    active_delta_transition: Option<ActiveDeltaTransition>,
    already_clean: bool,
}

#[derive(ModelOutput)]
struct AcceptCleanReviewDecision {
    context: CleanReviewContext,
}

fn clean_review_context(
    snapshot: &Option<ReviewSnapshotId>,
    required: &BTreeSet<RequiredWork>,
    accepted: &BTreeSet<RequiredWork>,
    blockers: &BTreeSet<FindingOccurrenceId>,
    active_delta: &Option<ActiveDeltaTransition>,
    already_clean: &bool,
) -> CleanReviewContext {
    CleanReviewContext {
        current_snapshot: snapshot.clone(),
        required_work: required.clone(),
        accepted_work: accepted.clone(),
        unresolved_blockers: blockers.clone(),
        active_delta_transition: active_delta.clone(),
        already_clean: *already_clean,
    }
}

mapping! {
    AcceptCleanStateToDecision:
        (AcceptCleanReviewState.current_snapshot, AcceptCleanReviewState.required_work, AcceptCleanReviewState.accepted_work, AcceptCleanReviewState.unresolved_blockers, AcceptCleanReviewState.active_delta_transition, AcceptCleanReviewState.already_clean) => AcceptCleanReviewDecision.context
        using clean_review_context;
}

fn clean_review_stream(stream: &ReviewStream) -> StreamId {
    stream.as_stream_id().clone()
}

mapping! { AcceptCleanStreamToEvent: AcceptCleanReview.stream => ReviewEvent.stream using clean_review_stream; }

fn clean_review_fact(
    snapshot: &ReviewSnapshotId,
    evidence_id: &EvidenceId,
    context: &CleanReviewContext,
) -> Result<ReviewFact, CommandError> {
    if context.current_snapshot.as_ref() != Some(snapshot)
        || context.already_clean
        || !context.required_work.is_subset(&context.accepted_work)
        || !context.unresolved_blockers.is_empty()
        || context.active_delta_transition.is_some()
    {
        return Err(CommandError::ValidationError(
            "review_clean_not_authorized".to_owned(),
        ));
    }
    Ok(ReviewFact::CleanReviewAccepted {
        snapshot: snapshot.clone(),
        evidence_id: evidence_id.clone(),
    })
}

mapping! {
    AcceptCleanToFact:
        (AcceptCleanReview.snapshot, AcceptCleanReview.evidence_id, AcceptCleanReviewDecision.context) => ReviewEvent.fact
        using try clean_review_fact, error = CommandError;
}

impl ModelCommandLogic for AcceptCleanReview {
    type Event = ReviewEvent;
    type State = AcceptCleanReviewState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        match &event.fact {
            ReviewFact::RiskAssessed {
                snapshot,
                assessment,
            } => {
                folded.current_snapshot = Some(snapshot.clone());
                folded.required_work.clear();
                folded.accepted_work.clear();
                folded.unresolved_blockers.clear();
                folded.active_delta_transition = None;
                folded.already_clean = false;
                for route in assessment.routes() {
                    folded.required_work.insert(RequiredWork::new(
                        route.lens().clone(),
                        AssignmentKind::Lens,
                    ));
                    if matches!(route.verifier(), VerifierRoute::Required { .. }) {
                        folded.required_work.insert(RequiredWork::new(
                            route.lens().clone(),
                            AssignmentKind::Verifier,
                        ));
                    }
                }
            }
            ReviewFact::AssignmentResultAccepted { result }
                if folded.current_snapshot.as_ref() == Some(result.snapshot()) =>
            {
                folded.accepted_work.insert(RequiredWork::new(
                    result.assignment_id().lens().clone(),
                    result.assignment_id().kind(),
                ));
                for finding in result.findings() {
                    if finding.severity() == FindingSeverity::Blocking {
                        folded.unresolved_blockers.insert(finding.id().clone());
                    }
                }
            }
            ReviewFact::AssignmentIssued { assignment }
                if assignment.id().kind() == AssignmentKind::DeltaRisk
                    && folded.current_snapshot.as_ref() == Some(assignment.snapshot()) =>
            {
                if folded.active_delta_transition.is_none() {
                    folded.active_delta_transition = Some(ActiveDeltaTransition {
                        from_snapshot: assignment.snapshot().clone(),
                        to_snapshot: assignment.target_snapshot().cloned(),
                    });
                }
            }
            ReviewFact::DeltaReassessed {
                from_snapshot,
                to_snapshot,
                affected_lenses,
                ..
            } if folded.current_snapshot.as_ref() == Some(from_snapshot)
                && folded
                    .active_delta_transition
                    .as_ref()
                    .is_none_or(|transition| {
                        transition.from_snapshot == *from_snapshot
                            && transition.to_snapshot.as_ref() == Some(to_snapshot)
                    }) =>
            {
                folded.current_snapshot = Some(to_snapshot.clone());
                folded.active_delta_transition = None;
                folded
                    .accepted_work
                    .retain(|work| !affected_lenses.contains(&work.lens));
                folded.unresolved_blockers.retain(|finding_id| {
                    !affected_lenses.contains(finding_id.assignment_id().lens())
                });
                folded.already_clean = false;
            }
            ReviewFact::FindingResolutionVerified { finding_id, .. } => {
                folded.unresolved_blockers.remove(finding_id);
            }
            ReviewFact::CleanReviewAccepted { snapshot, .. }
                if folded.current_snapshot.as_ref() == Some(snapshot) =>
            {
                folded.already_clean = true;
            }
            ReviewFact::AssignmentIssued { .. }
            | ReviewFact::AssignmentResultAccepted { .. }
            | ReviewFact::AssignmentSuperseded { .. }
            | ReviewFact::DeltaReassessed { .. }
            | ReviewFact::CleanReviewAccepted { .. } => {}
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = AcceptCleanReviewDecision::model_builder()
            .context(AcceptCleanStateToDecision::apply((
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
                state.as_ref(),
            )))
            .build();
        let context = &decision.as_ref().context;
        if context.current_snapshot.as_ref() != Some(&self.snapshot) {
            return Err(CommandError::ValidationError(
                "review_clean_snapshot_mismatch".to_owned(),
            ));
        }
        if context.already_clean {
            return Err(CommandError::ValidationError(
                "review_snapshot_already_clean".to_owned(),
            ));
        }
        if !context.required_work.is_subset(&context.accepted_work) {
            return Err(CommandError::ValidationError(
                "review_required_work_incomplete".to_owned(),
            ));
        }
        if !context.unresolved_blockers.is_empty() {
            return Err(CommandError::ValidationError(
                "review_blocking_findings_unresolved".to_owned(),
            ));
        }
        if context.active_delta_transition.is_some() {
            return Err(CommandError::ValidationError(
                "review_delta_assessment_incomplete".to_owned(),
            ));
        }
        Ok(ModeledEvents::one(
            ReviewEvent::model_builder()
                .stream(AcceptCleanStreamToEvent::apply(self))
                .fact(AcceptCleanToFact::apply((self, self, decision.as_ref()))?)
                .build(),
        ))
    }
}

/// Builds the checked clean-review acceptance command.
#[must_use]
pub fn accept_clean_review(
    stream: ReviewStream,
    snapshot: ReviewSnapshotId,
    evidence_id: EvidenceId,
) -> impl eventcore::CommandLogic<Event = ReviewEvent> {
    let request = AcceptCleanReviewRequest::model_builder()
        .stream(stream)
        .snapshot(snapshot)
        .evidence_id(evidence_id)
        .build();
    AcceptCleanReview::model_builder()
        .stream(AcceptCleanRequestToStream::apply(request.as_ref()))
        .snapshot(AcceptCleanRequestToSnapshot::apply(request.as_ref()))
        .evidence_id(AcceptCleanRequestToEvidence::apply(request.as_ref()))
        .build()
}
