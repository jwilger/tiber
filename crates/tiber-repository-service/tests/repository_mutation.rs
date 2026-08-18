mod fixture;

use core::slice::from_ref;
use eventcore::model::{CheckStatus, StreamIdentity as _, check};
use tiber_repository_core::{
    RepositoryDispatchOutcome, RepositoryMutationFailureCode, RepositoryReconciliationState,
};
use tiber_repository_service::{
    RepositoryMutationEvent, RepositoryMutationFact, RepositoryMutationServiceError,
    RepositoryMutationStream, authorize_prepared_mutation, decide_approve_mutation,
    decide_cancel_mutation, decide_cancel_open_proposal_on_restart, decide_deny_mutation,
    decide_prepare_mutation, decide_propose_mutation, decide_record_applied, decide_record_failed,
    decide_record_reconciled, decide_record_unknown, decide_repropose_mutation,
    recover_prepared_from_history,
};

trait IntoTestPublication {
    fn into_test_publication(self) -> tiber_repository_service::RepositoryMutationPublication;
}

impl IntoTestPublication for tiber_repository_service::RepositoryMutationPublication {
    fn into_test_publication(self) -> tiber_repository_service::RepositoryMutationPublication {
        self
    }
}

impl IntoTestPublication for Option<tiber_repository_service::RepositoryMutationPublication> {
    fn into_test_publication(self) -> tiber_repository_service::RepositoryMutationPublication {
        self.expect("a new proposal must emit one publication")
    }
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn every_repository_mutation_command_has_complete_checked_provenance() {
    let report = check().expect("complete repository mutation model");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn identical_retained_proposal_is_admitted_as_no_new_publication() {
    let bytes = b"same proposal retry\n";
    let proposal = fixture::write_proposal(bytes);
    let stream = RepositoryMutationStream::new(&proposal.identity()).expect("stream");
    let first = decide_propose_mutation(&[], stream.clone(), proposal)
        .expect("first proposal admission")
        .expect("first proposal must publish");
    let (event, _) = first.into_event_and_consistency_streams();

    let retry = decide_propose_mutation(from_ref(&event), stream, fixture::write_proposal(bytes))
        .expect("identical retained proposal should reconcile");

    assert!(
        retry.is_none(),
        "retry must not append a duplicate Proposed fact"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn conflicting_or_malformed_retained_proposal_history_fails_closed() {
    let original = fixture::write_proposal(b"original proposal\n");
    let stream = RepositoryMutationStream::new(&original.identity()).expect("stream");
    let first = decide_propose_mutation(&[], stream.clone(), original)
        .expect("first proposal admission")
        .expect("first proposal must publish");
    let (event, _) = first.into_event_and_consistency_streams();
    let conflicting = decide_propose_mutation(
        from_ref(&event),
        stream.clone(),
        fixture::write_proposal(b"conflicting proposal\n"),
    );
    assert!(conflicting.is_err(), "conflicting retry must fail closed");

    let malformed = decide_propose_mutation(
        &[event.clone(), event],
        stream,
        fixture::write_proposal(b"original proposal\n"),
    );
    assert!(
        malformed.is_err(),
        "duplicate retained history must fail closed"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn exact_safe_proposal_is_recorded_without_raw_write_content() {
    let proposal = fixture::write_proposal(b"private source bytes");
    let expected_identity = proposal.identity();

    let stream = RepositoryMutationStream::new(&expected_identity)
        .expect("proposal provenance should make a valid durable stream");
    let publication = decide_propose_mutation(&[], stream, proposal)
        .expect("a new safe proposal should be accepted");
    let (event, [consistency_stream]) = publication
        .expect("a new proposal must emit one publication")
        .into_event_and_consistency_streams();
    let serialized =
        serde_json::to_string(&event).expect("a durable proposal event should serialize");

    assert_eq!(
        event.fact(),
        &RepositoryMutationFact::Proposed(expected_identity)
    );
    assert!(!serialized.contains("private source bytes"));
    assert_eq!(
        consistency_stream.as_stream_id().as_ref(),
        "tiber:repository-mutation:effect-1"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn signed_shell_can_close_prepare_and_applied_publications_before_and_after_dispatch() {
    let bytes = b"signed shell publication";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal publication"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity.clone(),
            identity.provenance().clone(),
            approval_id.clone(),
        )
        .expect("approval publication"),
    );
    let mut history = vec![proposed, approved];
    let prepared = publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("prepared publication"),
    );
    assert!(matches!(
        prepared.fact(),
        RepositoryMutationFact::Prepared(_)
    ));
    history.push(prepared);

    let authority = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("verified prepared history releases authority");
    let receipt = authority.into_applied_receipt();
    let applied = publication_event(
        decide_record_applied(&history, stream, receipt).expect("applied publication"),
    );
    assert!(matches!(applied.fact(), RepositoryMutationFact::Applied(_)));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    clippy::shadow_unrelated,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn restart_recovers_only_read_only_reconciliation_and_records_one_result() {
    let bytes = b"restart reconciliation bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity.clone(),
            identity.provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let mut history = vec![proposed, approved];
    let prepared = publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("dispatch should be prepared"),
    );
    history.push(prepared);
    let authority = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("verified prepared history releases authority");
    let RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) = authority.into_ambiguity()
    else {
        panic!("forced ambiguity should expose only reconciliation authority");
    };
    history.push(publication_event(
        decide_record_unknown(&history, stream.clone(), reconciliation.clone())
            .expect("unknown dispatch should become durable"),
    ));

    let recovered = recover_prepared_from_history(&history, &stream)
        .expect("prepared unknown should recover")
        .expect("recovery should yield read-only reconciliation authority");
    assert_eq!(recovered.identity().provenance(), identity.provenance());
    let result = recovered
        .clone()
        .bind_outcome(RepositoryReconciliationState::NotApplied);
    history.push(publication_event(
        decide_record_reconciled(&history, stream.clone(), result.clone())
            .expect("read-only reconciliation result should become durable"),
    ));

    assert!(matches!(
        history.iter().map(RepositoryMutationEvent::fact).collect::<Vec<_>>().as_slice(),
        [
            RepositoryMutationFact::Proposed(_),
            RepositoryMutationFact::Approved(_),
            RepositoryMutationFact::Prepared(prepared),
            RepositoryMutationFact::Unknown(unknown),
            RepositoryMutationFact::Reconciled(outcome),
        ] if *prepared == *recovered.identity()
            && *unknown == recovered
            && *outcome == result
    ));
    assert!(
        decide_record_reconciled(&history, stream, result).is_err(),
        "verified history must reject a second reconciliation publication"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::indexing_slicing,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn restart_rejects_prepared_identity_from_another_effect_stream() {
    let bytes = b"foreign prepared bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let source_stream = RepositoryMutationStream::new(&identity).expect("valid source stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], source_stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            source_stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let prepared = publication_event(
        decide_prepare_mutation(
            &[proposed, approved],
            source_stream,
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("source dispatch should be prepared"),
    );

    let foreign = fixture::write_proposal_for_effect(b"foreign", "effect-foreign");
    let foreign_stream =
        RepositoryMutationStream::new(&foreign.identity()).expect("foreign stream should be valid");
    let mut wire = serde_json::to_value(prepared).expect("prepared event should serialize");
    wire["stream"] =
        serde_json::to_value(foreign_stream.as_stream_id()).expect("stream should serialize");
    let malformed = serde_json::from_value(wire).expect("wire remains structurally valid");

    assert_eq!(
        recover_prepared_from_history(&[malformed], &foreign_stream),
        Err(RepositoryMutationServiceError::InvalidHistory)
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn recovery_rejects_terminal_histories_missing_proposal_and_approval_chain() {
    let bytes = b"malformed terminal recovery";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal publication"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("approval publication"),
    );
    let mut history = vec![proposed, approved];
    let prepared = publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("prepared publication"),
    );
    history.push(prepared.clone());
    let authority = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("prepared history should authorize once");
    let applied = publication_event(
        decide_record_applied(&history, stream.clone(), authority.into_applied_receipt())
            .expect("applied publication"),
    );
    assert_eq!(
        recover_prepared_from_history(&[prepared], &stream),
        Err(RepositoryMutationServiceError::InvalidHistory),
        "a terminal-free prepared suffix must not bypass its proposal and approval chain"
    );
    assert_eq!(
        recover_prepared_from_history(&[history[2].clone(), applied], &stream),
        Err(RepositoryMutationServiceError::InvalidHistory),
        "an applied suffix must not bypass its proposal and approval chain"
    );

    let reconciliation = authorize_prepared_mutation(
        &history,
        fixture::write_proposal(bytes),
        &assignment,
        &policy,
    )
    .expect("prepared history should authorize ambiguity fixture")
    .into_ambiguity();
    let RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) = reconciliation else {
        panic!("direct ambiguity conversion should return reconciliation authority")
    };
    let unknown = publication_event(
        decide_record_unknown(&history, stream.clone(), reconciliation.clone())
            .expect("unknown publication"),
    );
    let mut unknown_history = history;
    unknown_history.push(unknown.clone());
    let outcome = reconciliation.bind_outcome(RepositoryReconciliationState::NotApplied);
    let reconciled = publication_event(
        decide_record_reconciled(&unknown_history, stream.clone(), outcome)
            .expect("reconciled publication"),
    );
    assert_eq!(
        recover_prepared_from_history(&[unknown_history[2].clone(), unknown, reconciled], &stream),
        Err(RepositoryMutationServiceError::InvalidHistory),
        "a reconciled suffix must not bypass its proposal and approval chain"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn recovery_rejects_terminal_history_prepared_for_another_exact_proposal() {
    let bytes_a = b"proposal A terminal recovery";
    let (proposal_a, _assignment_a, _policy_a, approval_id_a) = fixture::write_request(bytes_a);
    let identity_a = proposal_a.identity();
    let stream = RepositoryMutationStream::new(&identity_a).expect("valid stream");
    let proposed_a = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes_a))
            .expect("proposal A publication"),
    );
    let approved_a = publication_event(
        decide_approve_mutation(
            from_ref(&proposed_a),
            stream.clone(),
            identity_a,
            proposal_a.identity().provenance().clone(),
            approval_id_a,
        )
        .expect("approval A publication"),
    );

    let bytes_b = b"proposal B terminal recovery";
    let (proposal_b, assignment_b, policy_b, approval_id_b) = fixture::write_request(bytes_b);
    let proposed_b = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes_b))
            .expect("proposal B publication"),
    );
    let approved_b = publication_event(
        decide_approve_mutation(
            from_ref(&proposed_b),
            stream.clone(),
            proposal_b.identity(),
            proposal_b.identity().provenance().clone(),
            approval_id_b.clone(),
        )
        .expect("approval B publication"),
    );
    let mut history_b = vec![proposed_b, approved_b];
    let prepared_b = publication_event(
        decide_prepare_mutation(
            &history_b,
            stream.clone(),
            &proposal_b,
            &assignment_b,
            &policy_b,
            approval_id_b,
        )
        .expect("prepared B publication"),
    );
    history_b.push(prepared_b.clone());
    let authority_b = authorize_prepared_mutation(&history_b, proposal_b, &assignment_b, &policy_b)
        .expect("proposal B history should authorize");
    let applied_b = publication_event(
        decide_record_applied(
            &history_b,
            stream.clone(),
            authority_b.into_applied_receipt(),
        )
        .expect("applied B publication"),
    );

    assert_eq!(
        recover_prepared_from_history(&[proposed_a, approved_a, prepared_b, applied_b], &stream,),
        Err(RepositoryMutationServiceError::InvalidHistory),
        "recovery must bind Prepared and terminal identity to the exact durable proposal"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn restart_cancellation_rejects_closed_history_from_another_effect_stream() {
    let bytes = b"foreign closed proposal";
    let proposal = fixture::write_proposal(bytes);
    let identity = proposal.identity();
    let source_stream = RepositoryMutationStream::new(&identity).expect("valid source stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], source_stream.clone(), fixture::write_proposal(bytes))
            .expect("source proposal should be recorded"),
    );
    let cancelled = publication_event(
        decide_cancel_mutation(
            from_ref(&proposed),
            source_stream,
            identity.clone(),
            identity.provenance().clone(),
        )
        .expect("source cancellation should be recorded"),
    );
    let foreign = fixture::write_proposal_for_effect(bytes, "effect-foreign-closed");
    let foreign_stream =
        RepositoryMutationStream::new(&foreign.identity()).expect("valid foreign stream");

    assert!(matches!(
        decide_cancel_open_proposal_on_restart(&[proposed, cancelled], foreign_stream),
        Err(RepositoryMutationServiceError::StreamProposalMismatch)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn repository_mutation_failures_expose_distinct_stable_codes() {
    let invalid_history = RepositoryMutationServiceError::InvalidHistory;
    let stream_mismatch = RepositoryMutationServiceError::StreamProposalMismatch;

    assert_eq!(
        invalid_history.code(),
        "repository_mutation_history_invalid"
    );
    assert_eq!(
        stream_mismatch.code(),
        "repository_mutation_stream_proposal_mismatch"
    );
    assert_ne!(invalid_history.code(), stream_mismatch.code());
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn unknown_is_the_single_durable_dispatch_outcome_and_returns_only_reconciliation_authority() {
    let bytes = b"unknown bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let redispatch_proposal = fixture::write_proposal(bytes);
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let mut history = vec![proposed, approved];
    history.push(publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("dispatch should be prepared"),
    ));
    let authorized = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("verified prepared history releases authority");
    let RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) = authorized.into_ambiguity()
    else {
        panic!("ambiguous dispatch must expose only reconciliation authority");
    };

    let unknown = publication_event(
        decide_record_unknown(&history, stream.clone(), reconciliation.clone())
            .expect("unknown outcome should become durable"),
    );
    history.push(unknown);
    assert_eq!(
        history.last().map(RepositoryMutationEvent::fact),
        Some(&RepositoryMutationFact::Unknown(reconciliation.clone()))
    );
    assert!(matches!(
        authorize_prepared_mutation(&history, redispatch_proposal, &assignment, &policy,),
        Err(RepositoryMutationServiceError::InvalidHistory)
    ));
    assert!(matches!(
        decide_record_unknown(&history, stream, reconciliation),
        Err(RepositoryMutationServiceError::TerminalAlreadyRecorded)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn failed_is_the_single_durable_terminal_after_prepared_dispatch() {
    let bytes = b"failed bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let mut history = vec![proposed, approved];
    history.push(publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("dispatch should be prepared"),
    ));
    let authorized = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("verified prepared history releases authority");
    let failure = authorized.into_failure(RepositoryMutationFailureCode::PreconditionNotMet);

    history.push(publication_event(
        decide_record_failed(&history, stream.clone(), failure.clone())
            .expect("definitive failure should become durable"),
    ));
    assert_eq!(
        history.last().map(RepositoryMutationEvent::fact),
        Some(&RepositoryMutationFact::Failed(failure.clone()))
    );
    assert!(
        decide_record_failed(&history, stream, failure).is_err(),
        "verified history must reject a second failure publication"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn applied_is_the_single_durable_terminal_after_prepared_dispatch() {
    let bytes = b"applied bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let mut history = vec![proposed, approved];
    history.push(publication_event(
        decide_prepare_mutation(
            &history,
            stream.clone(),
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("dispatch should be prepared"),
    ));
    let authorized = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("verified prepared history releases authority");
    let receipt = authorized.into_applied_receipt();

    history.push(publication_event(
        decide_record_applied(&history, stream.clone(), receipt.clone())
            .expect("applied outcome should become durable"),
    ));
    assert_eq!(
        history.last().map(RepositoryMutationEvent::fact),
        Some(&RepositoryMutationFact::Applied(receipt.clone()))
    );
    assert!(
        decide_record_applied(&history, stream, receipt).is_err(),
        "verified history must reject a second applied publication"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn prepared_publication_precedes_verified_dispatch_authority() {
    let bytes = b"prepared bytes";
    let (proposal, assignment, policy, approval_id) = fixture::write_request(bytes);
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = publication_event(
        decide_propose_mutation(&[], stream.clone(), fixture::write_proposal(bytes))
            .expect("proposal should be recorded"),
    );
    let approved = publication_event(
        decide_approve_mutation(
            from_ref(&proposed),
            stream.clone(),
            identity,
            proposal.identity().provenance().clone(),
            approval_id.clone(),
        )
        .expect("proposal should be approved"),
    );
    let mut history = vec![proposed, approved];
    let prepared = publication_event(
        decide_prepare_mutation(
            &history,
            stream,
            &proposal,
            &assignment,
            &policy,
            approval_id,
        )
        .expect("approved proposal should prepare a closed publication"),
    );
    assert!(matches!(
        prepared.fact(),
        RepositoryMutationFact::Prepared(_)
    ));
    history.push(prepared);

    let authorized = authorize_prepared_mutation(&history, proposal, &assignment, &policy)
        .expect("only verified prepared history releases dispatch authority");
    assert!(matches!(
        history.last().map(RepositoryMutationEvent::fact),
        Some(RepositoryMutationFact::Prepared(recorded)) if *recorded == authorized.identity()
    ));
}

fn publication_event<T: IntoTestPublication>(
    publication: T,
) -> tiber_repository_service::RepositoryMutationEvent {
    let (event, [_consistency_stream]) = publication
        .into_test_publication()
        .into_event_and_consistency_streams();
    event
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn owner_can_cancel_only_the_exact_durable_proposal_under_its_active_workflow() {
    let proposal = fixture::write_proposal(b"cancelled bytes");
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = decide_propose_mutation(&[], stream.clone(), proposal)
        .expect("proposal should be recorded");
    let proposed = publication_event(proposed);

    let cancelled = decide_cancel_mutation(
        from_ref(&proposed),
        stream,
        identity.clone(),
        identity.provenance().clone(),
    )
    .expect("the exact active proposal should be cancellable");
    let cancelled = publication_event(cancelled);

    assert_eq!(
        cancelled.fact(),
        &RepositoryMutationFact::Cancelled(identity)
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn owner_can_deny_only_the_exact_durable_proposal_under_its_active_workflow() {
    let proposal = fixture::write_proposal(b"denied bytes");
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = decide_propose_mutation(&[], stream.clone(), proposal)
        .expect("proposal should be recorded");
    let proposed = publication_event(proposed);

    let denied = decide_deny_mutation(
        from_ref(&proposed),
        stream,
        identity.clone(),
        identity.provenance().clone(),
    )
    .expect("the exact active proposal should be deniable");
    let denied = publication_event(denied);

    assert_eq!(denied.fact(), &RepositoryMutationFact::Denied(identity));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn proposal_cannot_be_recorded_on_another_effects_stream() {
    let proposal = fixture::write_proposal(b"proposal A");
    let foreign = fixture::write_proposal_for_effect(b"proposal B", "effect-2");
    let foreign_stream = RepositoryMutationStream::new(&foreign.identity()).expect("valid stream");

    assert!(decide_propose_mutation(&[], foreign_stream, proposal).is_err());
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn owner_approval_is_bound_to_the_exact_durable_proposal_and_active_workflow() {
    let proposal = fixture::write_proposal(b"approved bytes");
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = decide_propose_mutation(&[], stream.clone(), proposal)
        .expect("proposal should be accepted");
    let proposed = publication_event(proposed);
    let approval = fixture::approval_id("approval-1");

    let approved = decide_approve_mutation(
        from_ref(&proposed),
        stream,
        identity.clone(),
        identity.provenance().clone(),
        approval.clone(),
    )
    .expect("the exact active proposal should be approvable");
    let approved = publication_event(approved);

    let RepositoryMutationFact::Approved(recorded) = approved.fact() else {
        panic!("approval should emit an approved fact");
    };
    assert_eq!(recorded.proposal(), &identity);
    assert_eq!(recorded.approval(), &approval);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn owner_approval_rejects_stale_active_workflow_provenance() {
    let proposal = fixture::write_proposal(b"approval bytes");
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed = decide_propose_mutation(&[], stream.clone(), proposal)
        .expect("proposal should be recorded");
    let proposed = publication_event(proposed);

    assert!(
        decide_approve_mutation(
            from_ref(&proposed),
            stream,
            identity.clone(),
            tiber_repository_core::RepositoryMutationProvenance::new(
                identity.provenance().session_id().clone(),
                identity.provenance().agent_id().clone(),
                identity.provenance().workflow_id().clone(),
                identity.provenance().assignment_id().clone(),
                identity.provenance().assignment_scope().clone(),
                identity.provenance().assignment_epoch(),
                identity.provenance().attempt_number(),
                identity.provenance().context_receipt_id().clone(),
                tiber_workflow_core::PolicyDecisionId::parse("policy-replaced")
                    .expect("deterministic policy decision fixture should parse"),
                identity.provenance().effect_id().clone(),
                identity.provenance().idempotency_key().clone(),
                identity.provenance().deadline_milliseconds(),
            ),
            fixture::approval_id("approval-stale-workflow"),
        )
        .is_err()
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn approval_rejects_a_stream_not_owned_by_the_proposal_effect() {
    let proposal = fixture::write_proposal(b"approval stream bytes");
    let identity = proposal.identity();
    let stream = RepositoryMutationStream::new(&identity).expect("valid stream");
    let proposed =
        decide_propose_mutation(&[], stream, proposal).expect("proposal should be recorded");
    let proposed = publication_event(proposed);
    let foreign = fixture::write_proposal_for_effect(b"other", "effect-other");
    let foreign_stream =
        RepositoryMutationStream::new(&foreign.identity()).expect("foreign stream is valid");

    let Err(error) = decide_approve_mutation(
        from_ref(&proposed),
        foreign_stream,
        identity.clone(),
        identity.provenance().clone(),
        fixture::approval_id("approval-foreign-stream"),
    ) else {
        panic!("approval must reject a stream not owned by the proposal effect");
    };
    assert_eq!(
        error,
        RepositoryMutationServiceError::StreamProposalMismatch
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn terminal_history_for_another_proposal_identity_is_invalid() {
    let proposal_a = fixture::write_proposal(b"proposal A");
    let identity_a = proposal_a.identity();
    let stream = RepositoryMutationStream::new(&identity_a).expect("valid stream");
    let proposed_a = decide_propose_mutation(&[], stream.clone(), proposal_a)
        .expect("proposal A should be recorded");
    let proposed_a = publication_event(proposed_a);

    let proposal_b = fixture::write_proposal(b"proposal B");
    let identity_b = proposal_b.identity();
    let proposed_b = decide_propose_mutation(&[], stream.clone(), proposal_b)
        .expect("proposal B should be independently recordable");
    let proposed_b = publication_event(proposed_b);
    let denied_b = decide_deny_mutation(
        from_ref(&proposed_b),
        stream.clone(),
        identity_b.clone(),
        identity_b.provenance().clone(),
    )
    .expect("proposal B should be deniable");
    let denied_b = publication_event(denied_b);

    let Err(error) = decide_approve_mutation(
        &[proposed_a.clone(), denied_b.clone()],
        stream,
        identity_a.clone(),
        identity_a.provenance().clone(),
        fixture::approval_id("approval-after-foreign-terminal"),
    ) else {
        panic!("foreign terminal identity must invalidate retained history");
    };
    assert_eq!(error, RepositoryMutationServiceError::InvalidHistory);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    reason = "this public-boundary integration test keeps its scenario and explicit fail-fast assertions at crate scope"
)]
fn stale_proposal_digest_cannot_be_approved() {
    let original = fixture::write_proposal(b"original bytes");
    let stale = fixture::write_proposal(b"changed bytes");
    let original_identity = original.identity();
    let stream = RepositoryMutationStream::new(&original_identity).expect("valid stream");
    let proposed = decide_propose_mutation(&[], stream.clone(), original)
        .expect("original proposal should be recorded");
    let proposed = publication_event(proposed);
    let stale_identity = stale.identity();

    let Err(error) = decide_approve_mutation(
        from_ref(&proposed),
        stream.clone(),
        stale_identity.clone(),
        stale_identity.provenance().clone(),
        fixture::approval_id("approval-stale"),
    ) else {
        panic!("changed bytes must invalidate prior approval authority");
    };
    assert_eq!(error, RepositoryMutationServiceError::StaleProposal);

    let reproposed = decide_repropose_mutation(from_ref(&proposed), stream.clone(), stale)
        .expect("the caller may durably repropose after rereading changed content");
    let reproposed = publication_event(reproposed);
    assert_eq!(
        reproposed.fact(),
        &RepositoryMutationFact::Reproposed(stale_identity.clone())
    );
    let durable_history = [proposed.clone(), reproposed.clone()];
    assert!(matches!(
        decide_approve_mutation(
            &durable_history,
            stream.clone(),
            original_identity.clone(),
            original_identity.provenance().clone(),
            fixture::approval_id("approval-old-after-reproposal"),
        ),
        Err(RepositoryMutationServiceError::StaleProposal)
    ));
    let approved = decide_approve_mutation(
        &durable_history,
        stream,
        stale_identity.clone(),
        stale_identity.provenance().clone(),
        fixture::approval_id("approval-after-reproposal"),
    )
    .expect("only the newly durable proposal can receive approval");
    let approved = publication_event(approved);
    let RepositoryMutationFact::Approved(recorded) = approved.fact() else {
        panic!("reproposed identity should be recorded in approval");
    };
    assert_eq!(recorded.proposal(), &stale_identity);
}
