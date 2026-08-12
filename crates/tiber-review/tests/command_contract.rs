#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::shadow_unrelated,
    clippy::too_many_lines,
    reason = "black-box EventCore contract fixtures use fail-fast test ergonomics without entering shipping library code"
)]
mod tests {
    use eventcore::{RetryPolicy, execute};
    use eventcore_memory::InMemoryEventStore;
    use tiber_review::{
        ReviewStream,
        assignment::issue_assignment,
        clean::accept_clean_review,
        delta::reassess_delta,
        resolution::verify_finding_resolution,
        result::accept_assignment_result,
        risk::assess_risk,
        supersession::supersede_assignment,
        types::{
            AgentId, AssignmentAttempt, AssignmentId, AssignmentKind, AssignmentResult,
            ContextReceiptId, EvidenceId, FindingOccurrence, FindingOccurrenceId, FindingSeverity,
            LensDeltaClassification, LensRoute, LifecycleReceiptId, ModelRole, ReviewAssignment,
            ReviewIteration, ReviewLens, ReviewSessionId, ReviewSnapshotId, RiskAssessment,
            VerifierRoute,
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
        snapshot: ReviewSnapshotId,
        lens: ReviewLens,
        reviewer_role: ModelRole,
        session: ReviewSessionId,
    }

    fn fixture() -> Fixture {
        let session = parsed("review-contract", ReviewSessionId::parse);
        Fixture {
            stream: ReviewStream::for_session(&session).expect("fixture stream must be valid"),
            snapshot: parsed("source-snapshot-a", ReviewSnapshotId::parse),
            lens: parsed("correctness", ReviewLens::parse),
            reviewer_role: parsed("reviewer", ModelRole::parse),
            session,
        }
    }

    fn assessment(fixture: &Fixture) -> RiskAssessment {
        RiskAssessment::parse(
            parsed("risk-evidence", EvidenceId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            vec![LensRoute::new(
                fixture.lens.clone(),
                fixture.reviewer_role.clone(),
                VerifierRoute::NotRequired,
                parsed("remediation-reviewer", ModelRole::parse),
            )],
            parsed("risk-agent", AgentId::parse),
            parsed("risk-reviewer", ModelRole::parse),
            parsed("risk-context", ContextReceiptId::parse),
            parsed("risk-life", LifecycleReceiptId::parse),
        )
        .expect("fixture assessment must be valid")
    }

    fn assignment(
        fixture: &Fixture,
        attempt: AssignmentAttempt,
        context: &str,
    ) -> ReviewAssignment {
        let lifecycle = format!("{context}-closed");
        ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                attempt,
                AssignmentKind::Lens,
            ),
            fixture.snapshot.clone(),
            parsed("reviewer-agent", AgentId::parse),
            fixture.reviewer_role.clone(),
            parsed(context, ContextReceiptId::parse),
            parsed(&lifecycle, LifecycleReceiptId::parse),
        )
    }

    #[test]
    fn issued_assignment_accepts_only_exact_scheduler_provenance() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let issued = assignment(&fixture, AssignmentAttempt::FIRST, "fresh-context-1");
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment must succeed");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), issued.clone()),
            RetryPolicy::new(),
        ))
        .expect("assignment must succeed");

        let mismatched = AssignmentResult::new(
            issued.id().clone(),
            issued.snapshot().clone(),
            issued.agent_id().clone(),
            parsed("wrong-role", ModelRole::parse),
            issued.context_receipt().clone(),
            issued.lifecycle_receipt().clone(),
            parsed("result-evidence", EvidenceId::parse),
            vec![],
        );
        let mismatch = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), mismatched),
            RetryPolicy::new(),
        ));
        let _error = mismatch.expect_err("role mismatch must be rejected");

        let accepted = AssignmentResult::new(
            issued.id().clone(),
            issued.snapshot().clone(),
            issued.agent_id().clone(),
            issued.model_role().clone(),
            issued.context_receipt().clone(),
            issued.lifecycle_receipt().clone(),
            parsed("result-evidence", EvidenceId::parse),
            vec![],
        );
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream, accepted),
            RetryPolicy::new(),
        ))
        .expect("exact provenance must be accepted");
    }

    #[test]
    fn supersession_requires_a_fresh_bounded_attempt_and_context() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let first = assignment(&fixture, AssignmentAttempt::FIRST, "fresh-context-1");
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment must succeed");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), first.clone()),
            RetryPolicy::new(),
        ))
        .expect("first assignment must succeed");
        futures::executor::block_on(execute(
            &store,
            supersede_assignment(
                fixture.stream.clone(),
                first.id().clone(),
                parsed("cancelled", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("incomplete assignment may be superseded");

        let second_attempt = AssignmentAttempt::parse(2).expect("second attempt is bounded");
        let reused_context = assignment(&fixture, second_attempt, "fresh-context-1");
        let reused = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), reused_context),
            RetryPolicy::new(),
        ));
        let _error = reused.expect_err("a context receipt cannot be reused");

        let replacement = assignment(&fixture, second_attempt, "fresh-context-2");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream, replacement),
            RetryPolicy::new(),
        ))
        .expect("replacement uses the required attempt and a fresh context");
    }

    #[test]
    fn clean_review_requires_current_results_and_exact_finding_resolution() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let issued = assignment(&fixture, AssignmentAttempt::FIRST, "fresh-context-1");
        let finding_id = FindingOccurrenceId::new(
            issued.id().clone(),
            parsed("blocking-finding", EvidenceId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment must succeed");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), issued.clone()),
            RetryPolicy::new(),
        ))
        .expect("assignment must succeed");
        let result = AssignmentResult::new(
            issued.id().clone(),
            issued.snapshot().clone(),
            issued.agent_id().clone(),
            issued.model_role().clone(),
            issued.context_receipt().clone(),
            issued.lifecycle_receipt().clone(),
            parsed("result-evidence", EvidenceId::parse),
            vec![FindingOccurrence::new(
                finding_id.clone(),
                FindingSeverity::Blocking,
            )],
        );
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), result),
            RetryPolicy::new(),
        ))
        .expect("result must be accepted");
        let blocked = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                parsed("clean-evidence", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = blocked.expect_err("unresolved blocker must prevent clean review");
        let remediation = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::RemediationVerifier,
            )
            .with_remediation_occurrence(finding_id.clone()),
            fixture.snapshot.clone(),
            parsed("remediation-agent", AgentId::parse),
            parsed("remediation-reviewer", ModelRole::parse),
            parsed("remediation-context", ContextReceiptId::parse),
            parsed("remediation-closed", LifecycleReceiptId::parse),
        )
        .with_finding_target(finding_id.clone());
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), remediation.clone()),
            RetryPolicy::new(),
        ))
        .expect("remediation verifier must be assigned");
        let remediation_result = AssignmentResult::new(
            remediation.id().clone(),
            remediation.snapshot().clone(),
            remediation.agent_id().clone(),
            remediation.model_role().clone(),
            remediation.context_receipt().clone(),
            remediation.lifecycle_receipt().clone(),
            parsed("remediation-verification", EvidenceId::parse),
            vec![],
        );
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), remediation_result),
            RetryPolicy::new(),
        ))
        .expect("remediation verifier result must be accepted");
        futures::executor::block_on(execute(
            &store,
            verify_finding_resolution(fixture.stream.clone(), finding_id, remediation.id().clone()),
            RetryPolicy::new(),
        ))
        .expect("exact blocking occurrence may be resolved");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                fixture.snapshot,
                parsed("clean-evidence", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("all current evidence permits the clean transition");
    }

    #[test]
    fn verifier_blocking_finding_can_be_remediated_and_cleaned() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let lens = assignment(&fixture, AssignmentAttempt::FIRST, "lens-context");
        let verifier = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Verifier,
            ),
            fixture.snapshot.clone(),
            parsed("verifier-agent", AgentId::parse),
            parsed("verifier", ModelRole::parse),
            parsed("verifier-context", ContextReceiptId::parse),
            parsed("verifier-closed", LifecycleReceiptId::parse),
        );
        let finding_id = FindingOccurrenceId::new(
            verifier.id().clone(),
            parsed("verifier-blocking-finding", EvidenceId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                RiskAssessment::parse(
                    parsed("risk-evidence-required-verifier", EvidenceId::parse),
                    parsed("delta-reviewer", ModelRole::parse),
                    vec![LensRoute::new(
                        fixture.lens.clone(),
                        fixture.reviewer_role.clone(),
                        VerifierRoute::Required {
                            model_role: parsed("verifier", ModelRole::parse),
                        },
                        parsed("remediation-reviewer", ModelRole::parse),
                    )],
                    parsed("risk-agent", AgentId::parse),
                    parsed("risk-reviewer", ModelRole::parse),
                    parsed("risk-context-required-verifier", ContextReceiptId::parse),
                    parsed("risk-life-required-verifier", LifecycleReceiptId::parse),
                )
                .expect("fixture assessment with verifier must be valid"),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment with verifier succeeds");
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
        .expect("clean lens result succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), verifier.clone()),
            RetryPolicy::new(),
        ))
        .expect("required verifier assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    verifier.id().clone(),
                    verifier.snapshot().clone(),
                    verifier.agent_id().clone(),
                    verifier.model_role().clone(),
                    verifier.context_receipt().clone(),
                    verifier.lifecycle_receipt().clone(),
                    parsed("verifier-result", EvidenceId::parse),
                    vec![FindingOccurrence::new(
                        finding_id.clone(),
                        FindingSeverity::Blocking,
                    )],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("verifier blocker is accepted");
        let blocked = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                parsed("clean-before-remediation", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = blocked.expect_err("verifier blocker prevents a clean review");

        let verifier_self_remediation = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::RemediationVerifier,
            )
            .with_remediation_occurrence(finding_id.clone()),
            fixture.snapshot.clone(),
            verifier.agent_id().clone(),
            parsed("remediation-reviewer", ModelRole::parse),
            parsed("verifier-self-remediation-context", ContextReceiptId::parse),
            parsed(
                "verifier-self-remediation-closed",
                LifecycleReceiptId::parse,
            ),
        )
        .with_finding_target(finding_id.clone());
        let self_remediation = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), verifier_self_remediation),
            RetryPolicy::new(),
        ));
        let _error = self_remediation
            .expect_err("the verifier that raised a blocker cannot verify its remediation");

        let remediation = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::RemediationVerifier,
            )
            .with_remediation_occurrence(finding_id.clone()),
            fixture.snapshot.clone(),
            parsed("remediation-agent", AgentId::parse),
            parsed("remediation-reviewer", ModelRole::parse),
            parsed("remediation-context", ContextReceiptId::parse),
            parsed("remediation-closed", LifecycleReceiptId::parse),
        )
        .with_finding_target(finding_id.clone());
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), remediation.clone()),
            RetryPolicy::new(),
        ))
        .expect("verifier blocker gets a targeted remediation assignment");

        let remediation_with_finding = AssignmentResult::new(
            remediation.id().clone(),
            remediation.snapshot().clone(),
            remediation.agent_id().clone(),
            remediation.model_role().clone(),
            remediation.context_receipt().clone(),
            remediation.lifecycle_receipt().clone(),
            parsed("remediation-result-with-finding", EvidenceId::parse),
            vec![FindingOccurrence::new(
                FindingOccurrenceId::new(
                    remediation.id().clone(),
                    parsed("remediation-finding", EvidenceId::parse),
                ),
                FindingSeverity::Blocking,
            )],
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), remediation_with_finding),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("remediation verification cannot create a new finding");

        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    remediation.id().clone(),
                    remediation.snapshot().clone(),
                    remediation.agent_id().clone(),
                    remediation.model_role().clone(),
                    remediation.context_receipt().clone(),
                    remediation.lifecycle_receipt().clone(),
                    parsed("remediation-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("clean remediation verification succeeds");
        futures::executor::block_on(execute(
            &store,
            verify_finding_resolution(fixture.stream.clone(), finding_id, remediation.id().clone()),
            RetryPolicy::new(),
        ))
        .expect("verifier blocker resolution is verified");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                fixture.snapshot,
                parsed("clean-after-remediation", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("resolved verifier blocker permits a clean review");
    }

    #[test]
    fn same_evidence_from_distinct_assignments_has_distinct_remediation_authority() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let lens = assignment(&fixture, AssignmentAttempt::FIRST, "lens-origin-context");
        let verifier = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Verifier,
            ),
            fixture.snapshot.clone(),
            parsed("verifier-origin-agent", AgentId::parse),
            parsed("verifier", ModelRole::parse),
            parsed("verifier-origin-context", ContextReceiptId::parse),
            parsed("verifier-origin-closed", LifecycleReceiptId::parse),
        );
        let shared_evidence = parsed("same-blocking-evidence", EvidenceId::parse);
        let lens_finding = FindingOccurrenceId::new(lens.id().clone(), shared_evidence.clone());
        let verifier_finding =
            FindingOccurrenceId::new(verifier.id().clone(), shared_evidence.clone());

        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                RiskAssessment::parse(
                    parsed("risk-evidence-same-remediation-evidence", EvidenceId::parse),
                    parsed("delta-reviewer", ModelRole::parse),
                    vec![LensRoute::new(
                        fixture.lens.clone(),
                        fixture.reviewer_role.clone(),
                        VerifierRoute::Required {
                            model_role: parsed("verifier", ModelRole::parse),
                        },
                        parsed("remediation-reviewer", ModelRole::parse),
                    )],
                    parsed("risk-agent", AgentId::parse),
                    parsed("risk-reviewer", ModelRole::parse),
                    parsed(
                        "risk-context-same-remediation-evidence",
                        ContextReceiptId::parse,
                    ),
                    parsed(
                        "risk-life-same-remediation-evidence",
                        LifecycleReceiptId::parse,
                    ),
                )
                .expect("required-verifier fixture assessment is valid"),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment with a required verifier succeeds");
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
                    parsed("lens-origin-result", EvidenceId::parse),
                    vec![FindingOccurrence::new(
                        lens_finding.clone(),
                        FindingSeverity::Blocking,
                    )],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("lens blocker is accepted");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), verifier.clone()),
            RetryPolicy::new(),
        ))
        .expect("required verifier assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    verifier.id().clone(),
                    verifier.snapshot().clone(),
                    verifier.agent_id().clone(),
                    verifier.model_role().clone(),
                    verifier.context_receipt().clone(),
                    verifier.lifecycle_receipt().clone(),
                    parsed("verifier-origin-result", EvidenceId::parse),
                    vec![FindingOccurrence::new(
                        verifier_finding.clone(),
                        FindingSeverity::Blocking,
                    )],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("verifier blocker is accepted");

        let lens_remediation = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::RemediationVerifier,
            )
            .with_remediation_occurrence(lens_finding.clone()),
            fixture.snapshot.clone(),
            parsed("lens-remediation-agent", AgentId::parse),
            parsed("remediation-reviewer", ModelRole::parse),
            parsed("lens-remediation-context", ContextReceiptId::parse),
            parsed("lens-remediation-closed", LifecycleReceiptId::parse),
        )
        .with_finding_target(lens_finding.clone());
        let verifier_remediation = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::RemediationVerifier,
            )
            .with_remediation_occurrence(verifier_finding.clone()),
            fixture.snapshot.clone(),
            parsed("verifier-remediation-agent", AgentId::parse),
            parsed("remediation-reviewer", ModelRole::parse),
            parsed("verifier-remediation-context", ContextReceiptId::parse),
            parsed("verifier-remediation-closed", LifecycleReceiptId::parse),
        )
        .with_finding_target(verifier_finding.clone());
        assert_ne!(
            lens_remediation.id(),
            verifier_remediation.id(),
            "remediation identity includes the full source finding occurrence"
        );

        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), lens_remediation.clone()),
            RetryPolicy::new(),
        ))
        .expect("lens blocker gets an occurrence-bound remediation assignment");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    lens_remediation.id().clone(),
                    lens_remediation.snapshot().clone(),
                    lens_remediation.agent_id().clone(),
                    lens_remediation.model_role().clone(),
                    lens_remediation.context_receipt().clone(),
                    lens_remediation.lifecycle_receipt().clone(),
                    parsed("lens-remediation-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("lens remediation result is accepted");
        futures::executor::block_on(execute(
            &store,
            verify_finding_resolution(
                fixture.stream.clone(),
                lens_finding,
                lens_remediation.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("lens occurrence resolution is verified");

        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), verifier_remediation.clone()),
            RetryPolicy::new(),
        ))
        .expect("verifier blocker gets a distinct occurrence-bound remediation assignment");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    verifier_remediation.id().clone(),
                    verifier_remediation.snapshot().clone(),
                    verifier_remediation.agent_id().clone(),
                    verifier_remediation.model_role().clone(),
                    verifier_remediation.context_receipt().clone(),
                    verifier_remediation.lifecycle_receipt().clone(),
                    parsed("verifier-remediation-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("verifier remediation result is accepted");
        futures::executor::block_on(execute(
            &store,
            verify_finding_resolution(
                fixture.stream.clone(),
                verifier_finding,
                verifier_remediation.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("verifier occurrence resolution is verified");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                fixture.snapshot,
                parsed("both-occurrences-clean", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("every independently resolved occurrence permits clean review");
    }

    #[test]
    fn assignment_authority_and_supersession_fence_stale_work() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk assessment must succeed");
        let foreign = ReviewAssignment::new(
            AssignmentId::new(
                parsed("other", ReviewSessionId::parse),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Lens,
            ),
            fixture.snapshot.clone(),
            parsed("agent", AgentId::parse),
            fixture.reviewer_role.clone(),
            parsed("foreign-context", ContextReceiptId::parse),
            parsed("foreign-life", LifecycleReceiptId::parse),
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), foreign),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("foreign session must be rejected");

        let issued = assignment(&fixture, AssignmentAttempt::FIRST, "late-context");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), issued.clone()),
            RetryPolicy::new(),
        ))
        .expect("assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            supersede_assignment(
                fixture.stream.clone(),
                issued.id().clone(),
                parsed("cancelled", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("supersession succeeds");
        let late = AssignmentResult::new(
            issued.id().clone(),
            issued.snapshot().clone(),
            issued.agent_id().clone(),
            issued.model_role().clone(),
            issued.context_receipt().clone(),
            issued.lifecycle_receipt().clone(),
            parsed("late-result", EvidenceId::parse),
            vec![],
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream, late),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("superseded assignment result must be rejected");
    }

    #[test]
    fn content_identical_delivery_reuses_the_clean_source_snapshot() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let lens = assignment(&fixture, AssignmentAttempt::FIRST, "lens-context");
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), lens.clone()),
            RetryPolicy::new(),
        ))
        .expect("lens assignment succeeds");
        let lens_result = AssignmentResult::new(
            lens.id().clone(),
            lens.snapshot().clone(),
            lens.agent_id().clone(),
            lens.model_role().clone(),
            lens.context_receipt().clone(),
            lens.lifecycle_receipt().clone(),
            parsed("lens-result", EvidenceId::parse),
            vec![],
        );
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), lens_result),
            RetryPolicy::new(),
        ))
        .expect("lens result succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                parsed("clean-source-content", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("final source-content review succeeds");

        // A signed commit and push are delivery facts. They keep the exact
        // canonical source snapshot rather than creating a new review scope.
        let staged_snapshot = fixture.snapshot.clone();
        let head_snapshot = staged_snapshot.clone();
        let signed_commit_snapshot = head_snapshot.clone();
        let pushed_snapshot = signed_commit_snapshot.clone();
        assert_eq!(pushed_snapshot, fixture.snapshot);

        let delivery_metadata_assignment = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::DeltaRisk,
            ),
            fixture.snapshot.clone(),
            parsed("delta-agent", AgentId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            parsed("delta-context", ContextReceiptId::parse),
            parsed("delta-life", LifecycleReceiptId::parse),
        )
        .with_target_snapshot(pushed_snapshot.clone());
        let delta_assignment = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delivery_metadata_assignment.clone()),
            RetryPolicy::new(),
        ));
        let _error = delta_assignment.expect_err(
            "delivery metadata cannot issue a delta-risk assignment for unchanged source content",
        );

        let reassessment = futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                pushed_snapshot.clone(),
                delivery_metadata_assignment.id().clone(),
            ),
            RetryPolicy::new(),
        ));
        let _error = reassessment.expect_err(
            "content-identical commit and push cannot create a source-content reassessment",
        );

        let repeat_clean = futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                pushed_snapshot,
                parsed("duplicate-clean", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ));
        let _error = repeat_clean
            .expect_err("delivery verification does not require a second clean-review acceptance");
    }

    #[test]
    fn material_delta_rejects_stale_results_and_requires_next_iteration() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let stale = assignment(&fixture, AssignmentAttempt::FIRST, "stale-context");
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                assessment(&fixture),
            ),
            RetryPolicy::new(),
        ))
        .expect("risk succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), stale.clone()),
            RetryPolicy::new(),
        ))
        .expect("initial lens assignment succeeds");

        let changed = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        let delta = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::DeltaRisk,
            ),
            fixture.snapshot.clone(),
            parsed("delta-agent", AgentId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            parsed("delta-context-material", ContextReceiptId::parse),
            parsed("delta-life-material", LifecycleReceiptId::parse),
        )
        .with_target_snapshot(changed.clone());
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta.clone()),
            RetryPolicy::new(),
        ))
        .expect("delta assignment succeeds");
        let delta_with_finding = AssignmentResult::new(
            delta.id().clone(),
            delta.snapshot().clone(),
            delta.agent_id().clone(),
            delta.model_role().clone(),
            delta.context_receipt().clone(),
            delta.lifecycle_receipt().clone(),
            parsed("delta-result-with-finding", EvidenceId::parse),
            vec![FindingOccurrence::new(
                FindingOccurrenceId::new(
                    delta.id().clone(),
                    parsed("delta-finding", EvidenceId::parse),
                ),
                FindingSeverity::Blocking,
            )],
        )
        .with_delta_classifications(vec![LensDeltaClassification::new(
            fixture.lens.clone(),
            true,
        )]);
        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), delta_with_finding),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("delta risk cannot create a review finding");
        let delta_result = AssignmentResult::new(
            delta.id().clone(),
            delta.snapshot().clone(),
            delta.agent_id().clone(),
            delta.model_role().clone(),
            delta.context_receipt().clone(),
            delta.lifecycle_receipt().clone(),
            parsed("material-delta-result", EvidenceId::parse),
            vec![],
        )
        .with_delta_classifications(vec![LensDeltaClassification::new(
            fixture.lens.clone(),
            true,
        )]);
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), delta_result),
            RetryPolicy::new(),
        ))
        .expect("delta result succeeds");
        futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                changed.clone(),
                delta.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("material delta succeeds");

        let stale_result = AssignmentResult::new(
            stale.id().clone(),
            stale.snapshot().clone(),
            stale.agent_id().clone(),
            stale.model_role().clone(),
            stale.context_receipt().clone(),
            stale.lifecycle_receipt().clone(),
            parsed("stale-result", EvidenceId::parse),
            vec![],
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), stale_result),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("pre-delta result must be rejected");

        let replacement = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session,
                fixture.lens,
                ReviewIteration::parse(2).expect("second iteration is bounded"),
                AssignmentAttempt::FIRST,
                AssignmentKind::Lens,
            ),
            changed,
            parsed("replacement-agent", AgentId::parse),
            fixture.reviewer_role,
            parsed("replacement-context", ContextReceiptId::parse),
            parsed("replacement-life", LifecycleReceiptId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), replacement.clone()),
            RetryPolicy::new(),
        ))
        .expect("affected lens must be reassigned in the next iteration");
        let replacement_result = AssignmentResult::new(
            replacement.id().clone(),
            replacement.snapshot().clone(),
            replacement.agent_id().clone(),
            replacement.model_role().clone(),
            replacement.context_receipt().clone(),
            replacement.lifecycle_receipt().clone(),
            parsed("replacement-result", EvidenceId::parse),
            vec![],
        );
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), replacement_result),
            RetryPolicy::new(),
        ))
        .expect("post-delta replacement result must be accepted");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                replacement.snapshot().clone(),
                parsed("replacement-clean", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("replacement evidence may complete the changed snapshot review");
    }

    #[test]
    fn unchanged_lens_keeps_completed_evidence_but_fences_in_flight_snapshot_work() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let in_flight_lens = parsed("architecture", ReviewLens::parse);
        let two_lens_assessment = RiskAssessment::parse(
            parsed("risk-evidence-two-lens-delta", EvidenceId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            vec![
                LensRoute::new(
                    fixture.lens.clone(),
                    fixture.reviewer_role.clone(),
                    VerifierRoute::NotRequired,
                    parsed("remediation-reviewer", ModelRole::parse),
                ),
                LensRoute::new(
                    in_flight_lens.clone(),
                    fixture.reviewer_role.clone(),
                    VerifierRoute::NotRequired,
                    parsed("remediation-reviewer", ModelRole::parse),
                ),
            ],
            parsed("risk-agent", AgentId::parse),
            parsed("risk-reviewer", ModelRole::parse),
            parsed("risk-context-two-lens-delta", ContextReceiptId::parse),
            parsed("risk-life-two-lens-delta", LifecycleReceiptId::parse),
        )
        .expect("two-lens assessment must be valid");
        let completed = assignment(&fixture, AssignmentAttempt::FIRST, "completed-context");
        let in_flight = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                in_flight_lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Lens,
            ),
            fixture.snapshot.clone(),
            parsed("in-flight-agent", AgentId::parse),
            fixture.reviewer_role.clone(),
            parsed("in-flight-context", ContextReceiptId::parse),
            parsed("in-flight-life", LifecycleReceiptId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            assess_risk(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                two_lens_assessment,
            ),
            RetryPolicy::new(),
        ))
        .expect("two-lens risk assessment succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), completed.clone()),
            RetryPolicy::new(),
        ))
        .expect("completed unchanged lens assignment succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    completed.id().clone(),
                    completed.snapshot().clone(),
                    completed.agent_id().clone(),
                    completed.model_role().clone(),
                    completed.context_receipt().clone(),
                    completed.lifecycle_receipt().clone(),
                    parsed("completed-unchanged-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("completed unchanged lens result succeeds");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), in_flight.clone()),
            RetryPolicy::new(),
        ))
        .expect("second lens assignment starts before the source delta");

        let changed = parsed("source-snapshot-b", ReviewSnapshotId::parse);
        let delta = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session.clone(),
                fixture.lens.clone(),
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::DeltaRisk,
            ),
            fixture.snapshot.clone(),
            parsed("delta-agent", AgentId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            parsed("delta-context-unaffected", ContextReceiptId::parse),
            parsed("delta-life-unaffected", LifecycleReceiptId::parse),
        )
        .with_target_snapshot(changed.clone());
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), delta.clone()),
            RetryPolicy::new(),
        ))
        .expect("delta assignment succeeds");
        let delta_result = AssignmentResult::new(
            delta.id().clone(),
            delta.snapshot().clone(),
            delta.agent_id().clone(),
            delta.model_role().clone(),
            delta.context_receipt().clone(),
            delta.lifecycle_receipt().clone(),
            parsed("unaffected-delta-result", EvidenceId::parse),
            vec![],
        )
        .with_delta_classifications(vec![
            LensDeltaClassification::new(fixture.lens.clone(), false),
            LensDeltaClassification::new(in_flight_lens.clone(), false),
        ]);
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), delta_result),
            RetryPolicy::new(),
        ))
        .expect("complete all-unaffected delta result succeeds");
        futures::executor::block_on(execute(
            &store,
            reassess_delta(
                fixture.stream.clone(),
                fixture.snapshot.clone(),
                changed.clone(),
                delta.id().clone(),
            ),
            RetryPolicy::new(),
        ))
        .expect("actual A-to-B delta succeeds even when all lenses are unaffected");

        let stale_result = AssignmentResult::new(
            in_flight.id().clone(),
            in_flight.snapshot().clone(),
            in_flight.agent_id().clone(),
            in_flight.model_role().clone(),
            in_flight.context_receipt().clone(),
            in_flight.lifecycle_receipt().clone(),
            parsed("stale-unaffected-result", EvidenceId::parse),
            vec![],
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            accept_assignment_result(fixture.stream.clone(), stale_result),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err(
            "an in-flight assignment bound to A cannot accept its result after the A-to-B delta",
        );

        let replacement = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session,
                in_flight_lens,
                ReviewIteration::FIRST,
                AssignmentAttempt::parse(2).expect("second attempt is bounded"),
                AssignmentKind::Lens,
            ),
            changed.clone(),
            parsed("replacement-agent", AgentId::parse),
            fixture.reviewer_role,
            parsed("replacement-context", ContextReceiptId::parse),
            parsed("replacement-life", LifecycleReceiptId::parse),
        );
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), replacement.clone()),
            RetryPolicy::new(),
        ))
        .expect("the fenced unchanged lens receives a fresh B-bound replacement");
        futures::executor::block_on(execute(
            &store,
            accept_assignment_result(
                fixture.stream.clone(),
                AssignmentResult::new(
                    replacement.id().clone(),
                    replacement.snapshot().clone(),
                    replacement.agent_id().clone(),
                    replacement.model_role().clone(),
                    replacement.context_receipt().clone(),
                    replacement.lifecycle_receipt().clone(),
                    parsed("replacement-unaffected-result", EvidenceId::parse),
                    vec![],
                ),
            ),
            RetryPolicy::new(),
        ))
        .expect("fresh B-bound replacement result succeeds");
        futures::executor::block_on(execute(
            &store,
            accept_clean_review(
                fixture.stream,
                changed,
                parsed("clean-after-unaffected-replacement", EvidenceId::parse),
            ),
            RetryPolicy::new(),
        ))
        .expect("completed unchanged evidence and replacement evidence complete B");
    }

    #[test]
    fn concurrent_lenses_require_distinct_agents_at_issuance() {
        let store = InMemoryEventStore::new();
        let fixture = fixture();
        let second_lens = parsed("architecture", ReviewLens::parse);
        let assessment = RiskAssessment::parse(
            parsed("risk-evidence-two-lenses", EvidenceId::parse),
            parsed("delta-reviewer", ModelRole::parse),
            vec![
                LensRoute::new(
                    fixture.lens.clone(),
                    fixture.reviewer_role.clone(),
                    VerifierRoute::NotRequired,
                    parsed("remediation-reviewer", ModelRole::parse),
                ),
                LensRoute::new(
                    second_lens.clone(),
                    fixture.reviewer_role.clone(),
                    VerifierRoute::NotRequired,
                    parsed("remediation-reviewer", ModelRole::parse),
                ),
            ],
            parsed("risk-agent", AgentId::parse),
            parsed("risk-reviewer", ModelRole::parse),
            parsed("risk-context", ContextReceiptId::parse),
            parsed("risk-life", LifecycleReceiptId::parse),
        )
        .expect("two-lens assessment must be valid");
        futures::executor::block_on(execute(
            &store,
            assess_risk(fixture.stream.clone(), fixture.snapshot.clone(), assessment),
            RetryPolicy::new(),
        ))
        .expect("risk succeeds");
        let first = assignment(&fixture, AssignmentAttempt::FIRST, "first-lens-context");
        futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream.clone(), first.clone()),
            RetryPolicy::new(),
        ))
        .expect("first lens assignment succeeds");
        let reused_agent = ReviewAssignment::new(
            AssignmentId::new(
                fixture.session,
                second_lens,
                ReviewIteration::FIRST,
                AssignmentAttempt::FIRST,
                AssignmentKind::Lens,
            ),
            fixture.snapshot,
            first.agent_id().clone(),
            fixture.reviewer_role,
            parsed("second-lens-context", ContextReceiptId::parse),
            parsed("second-lens-life", LifecycleReceiptId::parse),
        );
        let rejected = futures::executor::block_on(execute(
            &store,
            issue_assignment(fixture.stream, reused_agent),
            RetryPolicy::new(),
        ));
        let _error = rejected.expect_err("concurrent lenses must use distinct agents");
    }
}
