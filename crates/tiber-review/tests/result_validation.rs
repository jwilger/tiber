#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    clippy::expect_used,
    clippy::implicit_return,
    reason = "black-box result-acceptance fixtures use fail-fast setup without entering shipping library code"
)]
mod tests {
    use eventcore::{RetryPolicy, execute};
    use eventcore_memory::InMemoryEventStore;
    use tiber_review::{
        ReviewStream,
        assignment::issue_assignment,
        delta::reassess_delta,
        result::accept_assignment_result,
        risk::assess_risk,
        types::{
            AgentId, AssignmentAttempt, AssignmentId, AssignmentKind, AssignmentResult,
            ContextReceiptId, EvidenceId, LensDeltaClassification, LensRoute, LifecycleReceiptId,
            ModelRole, ReviewAssignment, ReviewIteration, ReviewLens, ReviewSessionId,
            ReviewSnapshotId, RiskAssessment, VerifierRoute,
        },
    };

    fn parsed<T>(
        value: &str,
        parser: impl FnOnce(&str) -> Result<T, tiber_review::types::ReviewError>,
    ) -> T {
        parser(value).expect("fixture value must be valid")
    }

    struct Fixture {
        stream: ReviewStream,
        session: ReviewSessionId,
        snapshot_a: ReviewSnapshotId,
        snapshot_b: ReviewSnapshotId,
        lens: ReviewLens,
        reviewer_role: ModelRole,
    }

    fn fixture() -> Fixture {
        let session = parsed("result-validation", ReviewSessionId::parse);
        Fixture {
            stream: ReviewStream::for_session(&session).expect("fixture stream must be valid"),
            session,
            snapshot_a: parsed("source-snapshot-a", ReviewSnapshotId::parse),
            snapshot_b: parsed("source-snapshot-b", ReviewSnapshotId::parse),
            lens: parsed("correctness", ReviewLens::parse),
            reviewer_role: parsed("reviewer", ModelRole::parse),
        }
    }

    fn route(fixture: &Fixture, lens: ReviewLens) -> LensRoute {
        LensRoute::new(
            lens,
            fixture.reviewer_role.clone(),
            VerifierRoute::NotRequired,
            parsed("remediation-reviewer", ModelRole::parse),
        )
    }

    fn assess(routes: Vec<LensRoute>) -> RiskAssessment {
        RiskAssessment::parse(
            parsed("risk-evidence", EvidenceId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            routes,
            parsed("risk-agent", AgentId::parse),
            parsed("risk-reviewer", ModelRole::parse),
            parsed("risk-context", ContextReceiptId::parse),
            parsed("risk-life", LifecycleReceiptId::parse),
        )
        .expect("fixture assessment must be valid")
    }

    fn delta_assignment(fixture: &Fixture) -> ReviewAssignment {
        ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::DeltaRisk,
            ),
            fixture.snapshot_a.clone(),
            parsed("delta-agent", AgentId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            parsed("delta-context", ContextReceiptId::parse),
            parsed("delta-life", LifecycleReceiptId::parse),
        )
        .with_target_snapshot(fixture.snapshot_b.clone())
    }

    fn delta_result(
        assignment: &ReviewAssignment,
        evidence_id: &str,
        classifications: Vec<LensDeltaClassification>,
    ) -> AssignmentResult {
        AssignmentResult::new(
            assignment.id().clone(),
            assignment.snapshot().clone(),
            assignment.agent_id().clone(),
            assignment.model_role().clone(),
            assignment.context_receipt().clone(),
            assignment.lifecycle_receipt().clone(),
            parsed(evidence_id, EvidenceId::parse),
            vec![],
        )
        .with_delta_classifications(classifications)
    }

    #[test]
    fn empty_delta_classification_is_rejected_before_the_original_result_can_complete() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let assignment = delta_assignment(&fixture);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assess(vec![route(&fixture, fixture.lens.clone())]),
            ),
            RetryPolicy::new(),
        ))
        .expect("one-lens risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), assignment.clone()),
            RetryPolicy::new(),
        ))
        .expect("delta-risk assignment succeeds");

        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                delta_result(&assignment, "empty-delta-result", vec![]),
            ),
            RetryPolicy::new(),
        ));
        let _error =
            rejected.expect_err("an empty delta classification cannot complete an assignment");

        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                delta_result(
                    &assignment,
                    "complete-delta-result",
                    vec![LensDeltaClassification::new(fixture.lens.clone(), false)],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("the original assignment remains acceptable after rejected input");
        futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream,
                fixture.snapshot_a,
                fixture.snapshot_b,
                assignment.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("the complete original result authorizes reassessment");
    }

    #[test]
    fn multi_lens_delta_rejects_partial_and_duplicate_classifications_before_reassessment() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let second_lens = parsed("architecture", ReviewLens::parse);
        let assignment = delta_assignment(&fixture);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assess(vec![
                    route(&fixture, fixture.lens.clone()),
                    route(&fixture, second_lens.clone()),
                ]),
            ),
            RetryPolicy::new(),
        ))
        .expect("multi-lens risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), assignment.clone()),
            RetryPolicy::new(),
        ))
        .expect("delta-risk assignment succeeds");

        for (evidence, classifications) in [
            (
                "partial-delta-result",
                vec![LensDeltaClassification::new(fixture.lens.clone(), false)],
            ),
            (
                "duplicate-delta-result",
                vec![
                    LensDeltaClassification::new(fixture.lens.clone(), false),
                    LensDeltaClassification::new(fixture.lens.clone(), true),
                ],
            ),
        ] {
            let rejected = futures::executor::block_on(execute(
                &store,
                accept_assignment_result(
                    fixture.stream.clone(),
                    delta_result(&assignment, evidence, classifications),
                ),
                RetryPolicy::new(),
            ));
            let _error = rejected.expect_err(
                "every delta result must classify each risk-assessed lens exactly once",
            );
        }

        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                delta_result(
                    &assignment,
                    "complete-multi-lens-delta-result",
                    vec![
                        LensDeltaClassification::new(fixture.lens.clone(), false),
                        LensDeltaClassification::new(second_lens, true),
                    ],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("a complete replacement result remains acceptable");
        futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream,
                fixture.snapshot_a,
                fixture.snapshot_b,
                assignment.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("a complete multi-lens result authorizes reassessment");
    }

    #[test]
    fn non_delta_result_rejects_delta_classifications_without_consuming_assignment() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let assignment = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Lens,
            ),
            fixture.snapshot_a.clone(),
            parsed("lens-agent", AgentId::parse),
            fixture.reviewer_role.clone(),
            parsed("lens-context", ContextReceiptId::parse),
            parsed("lens-life", LifecycleReceiptId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assess(vec![route(&fixture, fixture.lens.clone())]),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), assignment.clone()),
            RetryPolicy::new(),
        ))
        .expect("lens assignment succeeds");

        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                delta_result(
                    &assignment,
                    "lens-result-with-delta-classification",
                    vec![LensDeltaClassification::new(fixture.lens.clone(), false)],
                ),
            ),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("lens results cannot carry delta classifications");

        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream,
                delta_result(&assignment, "ordinary-lens-result", vec![]),
            ),
            RetryPolicy::new(),
        ))
        .expect("rejected classification input does not consume a lens assignment");
    }
}
