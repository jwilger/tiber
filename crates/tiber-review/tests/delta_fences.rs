#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::shadow_unrelated,
    clippy::too_many_lines,
    reason = "black-box delta fence coverage uses fail-fast fixtures without entering shipping library code"
)]
mod tests {
    use eventcore::{RetryPolicy, execute};
    use eventcore_memory::InMemoryEventStore;
    use tiber_review::{
        ReviewStream,
        assignment::issue_assignment,
        clean::accept_clean_review,
        delta::reassess_delta,
        result::accept_assignment_result,
        risk::assess_risk,
        supersession::supersede_assignment,
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
        snapshot_a: ReviewSnapshotId,
        lens: ReviewLens,
        reviewer_role: ModelRole,
        session: ReviewSessionId,
    }

    fn fixture() -> Fixture {
        let session = parsed("delta-fence", ReviewSessionId::parse);
        Fixture {
            stream: ReviewStream::for_session(&session).expect("fixture stream must be valid"),
            snapshot_a: parsed("source-snapshot-a", ReviewSnapshotId::parse),
            lens: parsed("correctness", ReviewLens::parse),
            reviewer_role: parsed("reviewer", ModelRole::parse),
            session,
        }
    }

    fn assessment(fixture: &Fixture, second_lens: Option<ReviewLens>) -> RiskAssessment {
        let mut routes = vec![LensRoute::new(
            fixture.lens.clone(),
            fixture.reviewer_role.clone(),
            VerifierRoute::NotRequired,
            parsed("remediation-reviewer", ModelRole::parse),
        )];
        if let Some(lens) = second_lens {
            routes.push(LensRoute::new(
                lens,
                fixture.reviewer_role.clone(),
                VerifierRoute::NotRequired,
                parsed("second-remediation-reviewer", ModelRole::parse),
            ));
        }
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

    fn lens_assignment(fixture: &Fixture) -> ReviewAssignment {
        ReviewAssignment::new(
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
        )
    }

    fn delta_assignment(
        fixture: &Fixture,
        lens: ReviewLens,
        attempt: AssignmentAttempt,
        target: ReviewSnapshotId,
        receipt_suffix: &str,
    ) -> ReviewAssignment {
        ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                lens,
                ReviewIteration::FIRST,
                attempt,
                AssignmentKind::DeltaRisk,
            ),
            fixture.snapshot_a.clone(),
            parsed("delta-agent", AgentId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            parsed(
                &format!("delta-context-{receipt_suffix}"),
                ContextReceiptId::parse,
            ),
            parsed(
                &format!("delta-life-{receipt_suffix}"),
                LifecycleReceiptId::parse,
            ),
        )
        .with_target_snapshot(target)
    }

    #[test]
    fn clean_review_waits_for_one_active_delta_assessment() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let lens = lens_assignment(&fixture);
        let snapshot_b = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        let snapshot_c = parsed("source-snapshot-c", ReviewSnapshotId::parse);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assessment(&fixture, None),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), lens.clone()),
            RetryPolicy::new(),
        ))
        .expect("lens assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    lens.id().clone(),
                    lens.snapshot().clone(),
                    lens.agent_id().clone(),
                    lens.model_role().clone(),
                    lens.context_receipt().clone(),
                    lens.lifecycle_receipt().clone(),
                    parsed("lens-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("A-bound lens evidence makes A otherwise cleanable");

        let delta_to_b = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_b.clone(),
            "b",
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta_to_b.clone()),
            RetryPolicy::new(),
        ))
        .expect("one A-to-B delta assessment starts");

        let stale_clean = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                parsed("clean-during-delta", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = stale_clean
            .expect_err("the pending A-to-B assessment fences stale clean acceptance for A");

        let delta_to_c = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_c,
            "c",
        );
        let concurrent_delta = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta_to_c),
            RetryPolicy::new(),
        ));
        let _error = concurrent_delta
            .expect_err("a second DeltaRisk assignment cannot race the active A-to-B assessment");

        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    delta_to_b.id().clone(),
                    delta_to_b.snapshot().clone(),
                    delta_to_b.agent_id().clone(),
                    delta_to_b.model_role().clone(),
                    delta_to_b.context_receipt().clone(),
                    delta_to_b.lifecycle_receipt().clone(),
                    parsed("delta-result", EvidenceId::parse),
                    vec![],
                )
                .with_delta_classifications(vec![LensDeltaClassification::new(
                    fixture.lens.clone(),
                    false,
                )]),
            ),
            RetryPolicy::new(),
        ))
        .expect("delta evidence is accepted before the source snapshot transition");

        let stale_clean = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                parsed("clean-before-reassessment", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = stale_clean.expect_err(
            "accepted delta evidence alone cannot complete the source-content reassessment",
        );

        futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                snapshot_b.clone(),
                delta_to_b.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("the complete A-to-B assessment transitions review scope");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                snapshot_b,
                parsed("clean-b", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("unaffected evidence can complete the transitioned B snapshot");
    }

    #[test]
    fn superseding_delta_work_does_not_reopen_the_stale_source_snapshot() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let lens = lens_assignment(&fixture);
        let snapshot_b = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assessment(&fixture, None),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), lens.clone()),
            RetryPolicy::new(),
        ))
        .expect("lens assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    lens.id().clone(),
                    lens.snapshot().clone(),
                    lens.agent_id().clone(),
                    lens.model_role().clone(),
                    lens.context_receipt().clone(),
                    lens.lifecycle_receipt().clone(),
                    parsed("lens-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("A is otherwise cleanable before delta discovery");

        let delta_to_b = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_b,
            "superseded",
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta_to_b.clone()),
            RetryPolicy::new(),
        ))
        .expect("A-to-B assessment starts");
        futures::executor::block_on(execute(
            &store,
            supersede_assignment(
                fixture.stream.clone(),
                delta_to_b.id().clone(),
                parsed("delta-cancelled", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("delta work may be superseded for a replacement");

        let stale_clean = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                fixture.snapshot_a,
                parsed("clean-after-delta-supersession", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = stale_clean.expect_err(
            "superseding A-to-B work does not make the known source transition disappear",
        );
    }

    #[test]
    fn delta_transitions_are_serialized_across_lenses_and_allow_exact_replacement() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let other_lens = parsed("architecture", ReviewLens::parse);
        let snapshot_b = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        let snapshot_c = parsed("source-snapshot-c", ReviewSnapshotId::parse);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assessment(&fixture, Some(other_lens.clone())),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment succeeds");

        let delta_to_b = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_b.clone(),
            "first-transition",
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta_to_b.clone()),
            RetryPolicy::new(),
        ))
        .expect("the first source transition starts");

        let delta_to_c = delta_assignment(
            &fixture,
            other_lens,
            AssignmentAttempt::FIRST,
            snapshot_c,
            "concurrent-transition",
        );
        let concurrent = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta_to_c),
            RetryPolicy::new(),
        ));
        let error = concurrent.expect_err(
            "a distinct-lens DeltaRisk assignment cannot start a second source transition",
        );
        assert!(matches!(
            error,
            eventcore::CommandError::ValidationError(code)
                if code == "review_delta_transition_pending"
        ));

        futures::executor::block_on(execute(
            &store,
            supersede_assignment(
                fixture.stream.clone(),
                delta_to_b.id().clone(),
                parsed("replace-transition", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("the original transition can be superseded");

        let replacement = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::parse(2).expect("second attempt must be valid"),
            snapshot_b,
            "replacement-transition",
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream, replacement),
            RetryPolicy::new(),
        ))
        .expect("only the superseded transition's next logical assignment may replace it");
    }

    #[test]
    fn same_lens_result_cannot_clear_an_active_delta_assignment() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let snapshot_b = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot_a.clone(),
                assessment(&fixture, None),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment succeeds");

        let active_delta = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_b.clone(),
            "active-delta",
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), active_delta.clone()),
            RetryPolicy::new(),
        ))
        .expect("delta work starts");

        let lens = lens_assignment(&fixture);
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), lens.clone()),
            RetryPolicy::new(),
        ))
        .expect("independent lens work can be issued while delta work is active");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    lens.id().clone(),
                    lens.snapshot().clone(),
                    lens.agent_id().clone(),
                    lens.model_role().clone(),
                    lens.context_receipt().clone(),
                    lens.lifecycle_receipt().clone(),
                    parsed("same-lens-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("the same lens result is accepted");

        let duplicate = delta_assignment(
            &fixture,
            fixture.lens.clone(),
            AssignmentAttempt::FIRST,
            snapshot_b,
            "duplicate-delta",
        );
        let reissued = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream, duplicate),
            RetryPolicy::new(),
        ));
        let error = reissued.expect_err(
            "a different same-lens result cannot clear the active DeltaRisk assignment",
        );
        assert!(matches!(
            error,
            eventcore::CommandError::ValidationError(code) if code == "review_assignment_active"
        ));
    }
}
