//! Risk-assessment domain intent and its command-specific folded state.

#![expect(
    clippy::missing_trait_methods,
    clippy::single_call_fn,
    clippy::trivially_copy_pass_by_ref,
    reason = "EventCore mapping signatures and static stream discovery are defined by the checked-model API"
)]

use eventcore::{
    CommandError, ModelCommand, ModelInput, ModelOutput, ModelState, StreamId, mapping,
    model::{ModelCommandLogic, Modeled, ModeledEvents, StreamIdentity as _},
};

use crate::__eventcore_model_reviewevent;
use crate::{ReviewEvent, ReviewFact, ReviewSnapshotId, ReviewStream, RiskAssessment};

#[derive(ModelInput)]
struct AssessRiskRequest {
    #[model(origin)]
    stream: ReviewStream,
    #[model(origin)]
    snapshot: ReviewSnapshotId,
    #[model(origin)]
    assessment: RiskAssessment,
}

/// Records the first risk assessment for one exact source snapshot.
#[derive(ModelCommand)]
struct AssessRisk {
    #[stream]
    stream: ReviewStream,
    snapshot: ReviewSnapshotId,
    assessment: RiskAssessment,
}

mapping! { AssessRiskRequestToStream: AssessRiskRequest.stream => AssessRisk.stream using clone; }
mapping! { AssessRiskRequestToSnapshot: AssessRiskRequest.snapshot => AssessRisk.snapshot using clone; }
mapping! { AssessRiskRequestToAssessment: AssessRiskRequest.assessment => AssessRisk.assessment using clone; }

#[derive(ModelState)]
struct AssessRiskState {
    #[model(default)]
    already_assessed: bool,
}

#[derive(ModelOutput)]
struct AssessRiskDecision {
    already_assessed: bool,
}

mapping! {
    AssessRiskStateToDecision:
        AssessRiskState.already_assessed => AssessRiskDecision.already_assessed
        using clone;
}

fn assessment_stream(stream: &ReviewStream) -> StreamId {
    stream.as_stream_id().clone()
}

mapping! { AssessRiskStreamToEvent: AssessRisk.stream => ReviewEvent.stream using assessment_stream; }

fn risk_assessed_fact(
    snapshot: &ReviewSnapshotId,
    assessment: &RiskAssessment,
    already_assessed: &bool,
) -> Result<ReviewFact, CommandError> {
    if *already_assessed {
        return Err(CommandError::ValidationError(
            "review_risk_already_assessed".to_owned(),
        ));
    }
    Ok(ReviewFact::RiskAssessed {
        snapshot: snapshot.clone(),
        assessment: assessment.clone(),
    })
}

mapping! {
    AssessRiskToFact:
        (AssessRisk.snapshot, AssessRisk.assessment, AssessRiskDecision.already_assessed) => ReviewEvent.fact
        using try risk_assessed_fact, error = CommandError;
}

impl ModelCommandLogic for AssessRisk {
    type Event = ReviewEvent;
    type State = AssessRiskState;

    fn evolve(&self, state: Modeled<Self::State>, event: &Self::Event) -> Modeled<Self::State> {
        let mut folded = state.into_inner();
        if matches!(event.fact, ReviewFact::RiskAssessed { .. }) {
            folded.already_assessed = true;
        }
        Modeled::from_built(folded)
    }

    fn decide(
        &self,
        state: Modeled<Self::State>,
    ) -> Result<ModeledEvents<Self::Event>, CommandError> {
        let decision = AssessRiskDecision::model_builder()
            .already_assessed(AssessRiskStateToDecision::apply(state.as_ref()))
            .build();
        if decision.as_ref().already_assessed {
            return Err(CommandError::ValidationError(
                "review_risk_already_assessed".to_owned(),
            ));
        }
        self.assessment
            .validate()
            .map_err(|error| CommandError::ValidationError(error.code().to_owned()))?;
        Ok(ModeledEvents::one(
            ReviewEvent::model_builder()
                .stream(AssessRiskStreamToEvent::apply(self))
                .fact(AssessRiskToFact::apply((self, self, decision.as_ref()))?)
                .build(),
        ))
    }
}

/// Builds the checked `EventCore` command from parsed boundary values.
#[must_use]
pub fn assess_risk(
    stream: ReviewStream,
    snapshot: ReviewSnapshotId,
    assessment: RiskAssessment,
) -> impl eventcore::CommandLogic<Event = ReviewEvent> {
    let request = AssessRiskRequest::model_builder()
        .stream(stream)
        .snapshot(snapshot)
        .assessment(assessment)
        .build();
    AssessRisk::model_builder()
        .stream(AssessRiskRequestToStream::apply(request.as_ref()))
        .snapshot(AssessRiskRequestToSnapshot::apply(request.as_ref()))
        .assessment(AssessRiskRequestToAssessment::apply(request.as_ref()))
        .build()
}

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::expect_used,
    clippy::panic,
    reason = "model contract fixtures use fail-fast test ergonomics and never enter shipping library code"
)]
mod tests {
    use eventcore::{RetryPolicy, execute};
    use eventcore_memory::InMemoryEventStore;

    use super::*;
    use crate::types::{
        EvidenceId, LensRoute, ModelRole, ReviewLens, ReviewSessionId, VerifierRoute,
    };

    fn parsed<T>(value: &str, parser: impl FnOnce(&str) -> Result<T, crate::ReviewError>) -> T {
        parser(value).unwrap_or_else(|error| panic!("valid fixture required: {error}"))
    }

    #[test]
    fn every_registered_review_command_has_checked_provenance() {
        let report = eventcore::model::check().expect("complete native review model");
        assert_eq!(report.status, eventcore::model::CheckStatus::Verified);
        assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    }

    #[test]
    fn risk_assessment_executes_once_through_eventcore() {
        let store = InMemoryEventStore::new();
        let session = parsed("review-1", ReviewSessionId::parse);
        let stream = ReviewStream::for_session(&session).expect("valid stream");
        let snapshot = parsed("snapshot-a", ReviewSnapshotId::parse);
        let assessment = RiskAssessment::parse(
            parsed("risk-evidence", EvidenceId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            vec![LensRoute::new(
                parsed("correctness", ReviewLens::parse),
                parsed("reviewer", ModelRole::parse),
                VerifierRoute::NotRequired,
                parsed("remediation-reviewer", ModelRole::parse),
            )],
            parsed("risk-agent", crate::types::AgentId::parse),
            parsed("risk-reviewer", ModelRole::parse),
            parsed("risk-context", crate::types::ContextReceiptId::parse),
            parsed("risk-life", crate::types::LifecycleReceiptId::parse),
        )
        .expect("valid assessment");

        futures::executor::block_on(execute(
            &store,
            assess_risk(stream.clone(), snapshot.clone(), assessment.clone()),
            RetryPolicy::new(),
        ))
        .expect("first assessment succeeds");
        let duplicate = futures::executor::block_on(execute(
            &store,
            assess_risk(stream, snapshot, assessment),
            RetryPolicy::new(),
        ));
        let _error = duplicate.expect_err("duplicate assessment must fail");
    }
}
