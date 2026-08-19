#![expect(
    clippy::indexing_slicing,
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    reason = "public lifecycle scenarios inspect known positions and borrowed non-exhaustive durable facts from locally constructed modeled publications"
)]

use core::{fmt, iter, slice::from_ref, time::Duration};
use std::path::{Path, PathBuf};

use eventcore::model::{CheckStatus, StreamIdentity as _, check};
use tiber_process_core::{
    AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog, ConfiguredCommandId,
    FixedEnvironment, LiteralArgument, OutputBounds, ProcessInvocationId, ProcessRequest,
    RelativeWorkingDirectory,
};
use tiber_process_service::{
    CapturedProcessBytes, MAX_PROCESS_INVOCATION_STREAMS, PreparedProcessIdentity,
    ProcessCancelled, ProcessEvent, ProcessExitStatus, ProcessFact, ProcessReceipt,
    ProcessReconciliationOutcome, ProcessRefusal, ProcessRestartState, ProcessServiceError,
    ProcessSpawnFailure, ProcessSpawnFailureCode, ProcessStream, ProcessTimedOut, ProcessUnknown,
    admit_process_invocation, authorize_prepared_process, authorize_process_retirement,
    classify_process_restart, decide_process_request, decide_record_cancelled,
    decide_record_completed, decide_record_reconciled, decide_record_spawn_failed,
    decide_record_timed_out, decide_record_unknown, recover_process_reconciliation,
};
use tiber_workflow_core::{AssignmentId, EffectId, WorkflowId};

fn parsed<T, E: fmt::Debug>(result: Result<T, E>) -> T {
    result.expect("fixture value should satisfy the semantic boundary")
}

fn assert_retirement_unavailable(
    history: &[ProcessEvent],
    process_stream: &ProcessStream,
    message: &str,
) {
    assert!(
        authorize_process_retirement(history, process_stream)
            .expect("lifecycle history should be valid")
            .is_none(),
        "{message}"
    );
}

fn assert_retirement_identity(
    history: &[ProcessEvent],
    process_stream: &ProcessStream,
    identity: &PreparedProcessIdentity,
) {
    let retirement = authorize_process_retirement(history, process_stream)
        .expect("closed history should be valid")
        .expect("signed closed history should authorize private-artifact retirement");
    assert_eq!(
        retirement.prepared_identity(),
        identity,
        "retirement authority must retain the exact validated preparation identity"
    );
}

fn request(command_id: &str, effect_id: &str) -> ProcessRequest {
    ProcessRequest::for_invocation(
        parsed(ConfiguredCommandId::parse(command_id)),
        parsed(ProcessInvocationId::parse(&format!(
            "{effect_id}-invocation"
        ))),
        AssignmentWorkflowProvenance::new(
            parsed(WorkflowId::parse("workflow-3")),
            parsed(AssignmentId::parse("assignment-3")),
            parsed(EffectId::parse(effect_id)),
        ),
    )
}

fn invocation_request(command_id: &str, effect_id: &str, invocation_id: &str) -> ProcessRequest {
    ProcessRequest::for_invocation(
        parsed(ConfiguredCommandId::parse(command_id)),
        parsed(ProcessInvocationId::parse(invocation_id)),
        AssignmentWorkflowProvenance::new(
            parsed(WorkflowId::parse("workflow-3")),
            parsed(AssignmentId::parse("assignment-3")),
            parsed(EffectId::parse(effect_id)),
        ),
    )
}

fn catalog(command_id: &str) -> ConfiguredCommandCatalog {
    catalog_with_program(command_id, "/nix/store/example/bin/cargo")
}

fn catalog_with_program(command_id: &str, program: &str) -> ConfiguredCommandCatalog {
    parsed(ConfiguredCommandCatalog::new([(
        parsed(ConfiguredCommandId::parse(command_id)),
        parsed(ConfiguredCommand::new(
            PathBuf::from(program),
            vec![parsed(LiteralArgument::parse("test"))],
            parsed(RelativeWorkingDirectory::parse("crates/tiber-process-core")),
            parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
            Duration::from_secs(30),
            parsed(OutputBounds::new(0x4000, 0x2000)),
        )),
    )]))
}

fn catalog_with_shape(
    command_id: &str,
    argv: &[&str],
    cwd: &str,
    environment: &[(&str, &str)],
) -> ConfiguredCommandCatalog {
    parsed(ConfiguredCommandCatalog::new([(
        parsed(ConfiguredCommandId::parse(command_id)),
        parsed(ConfiguredCommand::new(
            PathBuf::from("/trusted/bin/tool"),
            argv.iter()
                .map(|argument| parsed(LiteralArgument::parse(argument)))
                .collect(),
            parsed(RelativeWorkingDirectory::parse(cwd)),
            parsed(FixedEnvironment::new(environment.iter().copied())),
            Duration::from_secs(30),
            parsed(OutputBounds::new(0x4000, 0x2000)),
        )),
    )]))
}

#[expect(
    clippy::single_call_fn,
    reason = "the named fixture keeps the output-limit policy scenario readable"
)]
fn catalog_with_output_bounds(
    command_id: &str,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> ConfiguredCommandCatalog {
    parsed(ConfiguredCommandCatalog::new([(
        parsed(ConfiguredCommandId::parse(command_id)),
        parsed(ConfiguredCommand::new(
            PathBuf::from("/trusted/bin/tool"),
            Vec::new(),
            parsed(RelativeWorkingDirectory::parse(".")),
            parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
            Duration::from_secs(30),
            parsed(OutputBounds::new(stdout_bytes, stderr_bytes)),
        )),
    )]))
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn prepared_history_cannot_authorize_a_same_id_catalog_replacement() {
    let request = request("unit-test", "effect-process-catalog-change");
    let original_catalog = catalog_with_program("unit-test", "/trusted/bin/original");
    let replacement_catalog = catalog_with_program("unit-test", "/trusted/bin/replacement");
    let stream = stream("effect-process-catalog-change");
    let publication =
        decide_process_request(&[], stream.clone(), request.clone(), &original_catalog)
            .expect("original catalog decision should prepare");
    let (events, _) = publication.into_events_and_consistency_streams();

    let refusal = authorize_prepared_process(&events, &stream, &request, &replacement_catalog)
        .expect_err("prepared authority must be bound to the exact trusted catalog entry");

    assert_eq!(refusal, ProcessServiceError::CatalogChanged);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn catalog_identity_separates_argv_cwd_and_environment_domains() {
    let request = request("unit-test", "effect-process-catalog-framing");
    let original_catalog =
        catalog_with_shape("unit-test", &[], "a", &[("KEY1", "dir"), ("KEY2", "value")]);
    let replacement_catalog =
        catalog_with_shape("unit-test", &["a", "KEY1"], "dir", &[("KEY2", "value")]);
    let stream = stream("effect-process-catalog-framing");
    let publication =
        decide_process_request(&[], stream.clone(), request.clone(), &original_catalog)
            .expect("original catalog decision should prepare");
    let (events, _) = publication.into_events_and_consistency_streams();

    let refusal = authorize_prepared_process(&events, &stream, &request, &replacement_catalog)
        .expect_err("cross-field catalog framing must not collide");

    assert_eq!(refusal, ProcessServiceError::CatalogChanged);
}

fn stream(effect: &str) -> ProcessStream {
    ProcessStream::for_invocation(
        &parsed(EffectId::parse(effect)),
        &parsed(ProcessInvocationId::parse(&format!("{effect}-invocation"))),
    )
    .expect("valid process stream")
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn two_invocations_under_one_effect_have_distinct_signed_streams() {
    let effect = parsed(EffectId::parse("effect-process-retry"));
    let first = parsed(ProcessInvocationId::parse("request-1"));
    let retry = parsed(ProcessInvocationId::parse("request-2"));

    let first_stream = ProcessStream::for_invocation(&effect, &first).expect("first stream");
    let retry_stream = ProcessStream::for_invocation(&effect, &retry).expect("retry stream");

    assert_ne!(first_stream, retry_stream);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn one_invocation_history_cannot_authorize_another_invocation() {
    let first = invocation_request("unit-test", "effect-process-retry-history", "request-1");
    let retry = invocation_request("unit-test", "effect-process-retry-history", "request-2");
    let catalog = catalog("unit-test");
    let first_stream =
        ProcessStream::for_invocation(first.provenance().effect_id(), first.invocation_id())
            .expect("first stream");
    let retry_stream =
        ProcessStream::for_invocation(retry.provenance().effect_id(), retry.invocation_id())
            .expect("retry stream");
    let publication = decide_process_request(&[], first_stream, first, &catalog)
        .expect("first invocation should prepare");
    let (history, _) = publication.into_events_and_consistency_streams();

    let error = decide_process_request(&history, retry_stream, retry, &catalog)
        .expect_err("cross-invocation history must be rejected");

    assert_eq!(error, ProcessServiceError::InvalidHistory);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn every_process_command_has_complete_checked_provenance() {
    let report = check().expect("complete process authority model");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn signed_requested_and_prepared_history_is_required_before_adapter_authority() {
    let request = request("unit-test", "effect-process-1");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-1");

    let publication = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
        .expect("configured request should be admitted");
    let (events, [consistency_stream]) = publication.into_events_and_consistency_streams();

    assert_eq!(events.len(), 2);
    let requested = events.first().expect("requested event");
    let prepared = events.get(1).expect("prepared event");
    assert!(matches!(requested.fact(), ProcessFact::Requested(recorded) if recorded == &request));
    assert!(
        matches!(prepared.fact(), ProcessFact::Prepared(identity) if identity.request() == &request)
    );
    assert_eq!(consistency_stream, stream);
    assert_eq!(
        ProcessStream::from_verified_effect_stream(
            request.provenance().effect_id(),
            consistency_stream.as_stream_id(),
        ),
        Some(consistency_stream.clone()),
        "the invocation-digest stream must remain discoverable after restart"
    );

    let unsigned = authorize_prepared_process(&[], &stream, &request, &catalog)
        .expect_err("a catalog decision without signed history must not authorize dispatch");
    assert_eq!(unsigned, ProcessServiceError::PreparedHistoryRequired);

    let authorized = authorize_prepared_process(&events, &stream, &request, &catalog)
        .expect("matching signed requested and prepared history should authorize dispatch");
    let plan = authorized.into_adapter_execution_plan();
    assert_eq!(plan.program(), Path::new("/nix/store/example/bin/cargo"));
    assert_eq!(plan.argv().collect::<Vec<_>>(), ["test"]);
    assert_eq!(plan.provenance(), request.provenance());
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn unknown_command_is_durably_refused_without_preparation_or_authority() {
    let request = request("unknown-secret-marker", "effect-process-refused");
    let catalog = catalog("known");
    let stream = stream("effect-process-refused");

    let publication = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
        .expect("policy refusal should be a durable modeled outcome");
    let (events, _) = publication.into_events_and_consistency_streams();

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events.first().expect("refused event").fact(),
        ProcessFact::Refused {
            code: ProcessRefusal::UnknownConfiguredCommand,
            ..
        }
    ));
    let serialized = parsed(serde_json::to_string(&events));
    assert!(!serialized.contains("/nix/store/example/bin/cargo"));
    assert!(!serialized.contains("unknown-secret-marker"));

    let refusal = authorize_prepared_process(&events, &stream, &request, &catalog)
        .expect_err("refusal history must never mint process authority");
    assert_eq!(refusal, ProcessServiceError::RequestRefused);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn durable_refusal_rejects_an_unrecognized_policy_code() {
    let request = request("unknown", "effect-process-invalid-refusal");
    let catalog = catalog("known");
    let publication = decide_process_request(
        &[],
        stream("effect-process-invalid-refusal"),
        request,
        &catalog,
    )
    .expect("unknown command should produce a refusal");
    let (events, _) = publication.into_events_and_consistency_streams();
    let encoded = parsed(serde_json::to_string(
        events.first().expect("refused event"),
    ));
    let crafted = encoded.replace(
        "process_policy_unknown_configured_command",
        "process_policy_fabricated_code",
    );

    let decoded = serde_json::from_str::<ProcessEvent>(&crafted);

    assert!(
        decoded.is_err(),
        "unknown refusal codes must not become facts"
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn request_cannot_use_a_stream_owned_by_another_effect() {
    let request = request("unit-test", "effect-process-owned");

    let refusal = decide_process_request(
        &[],
        stream("effect-process-other"),
        request,
        &catalog("unit-test"),
    )
    .expect_err("a request must use only its exact effect-owned process stream");

    assert_eq!(refusal, ProcessServiceError::StreamRequestMismatch);
}

#[test]
#[expect(
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::tests_outside_test_module,
    reason = "the public test makes an explicit prepared-fact assertion before exercising non-UTF8 receipt behavior"
)]
fn completed_process_records_bounded_non_utf8_output_and_exit_status() {
    let request = request("unit-test", "effect-process-completed");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-completed");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(identity) = events.get(1).expect("prepared event").fact() else {
        panic!("second fact must be Prepared");
    };
    let receipt = ProcessReceipt::new(
        identity.clone(),
        ProcessExitStatus::Exited(7),
        &CapturedProcessBytes::new(vec![0xff, 0x00, b'o']).expect("bounded stdout"),
        &CapturedProcessBytes::new(vec![0xfe, b'e']).expect("bounded stderr"),
    )
    .expect("captured output should fit prepared limits");

    let terminal = decide_record_completed(&events, stream, receipt.clone())
        .expect("exact prepared process should complete");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();
    events.extend(terminal_events.clone());

    assert_eq!(terminal_events.len(), 1);
    assert_eq!(terminal_events[0].fact(), &ProcessFact::Completed(receipt));
    let serialized = serde_json::to_string(&events).expect("terminal history should serialize");
    assert!(
        !serialized.contains("\"stdout\":["),
        "durable history must retain only output identity, not raw bytes"
    );
    assert!(
        !format!("{events:?}").contains("[255, 0, 111]"),
        "debug output must redact captured process bytes"
    );
    assert_eq!(
        serde_json::from_str::<Vec<ProcessEvent>>(&serialized)
            .expect("non-UTF8 output identity should round-trip"),
        events
    );
}

#[test]
#[expect(
    clippy::pattern_type_mismatch,
    clippy::tests_outside_test_module,
    clippy::wildcard_enum_match_arm,
    reason = "the public test extracts only its prepared fixture while retaining forward-compatible non-prepared facts"
)]
fn completed_receipt_enforces_the_prepared_commands_output_limits() {
    let request = request("small-output", "effect-process-small-output");
    let catalog = catalog_with_output_bounds("small-output", 2, 1);
    let stream = stream("effect-process-small-output");
    let publication = decide_process_request(&[], stream, request, &catalog)
        .expect("configured request should prepare");
    let (events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let oversized = ProcessReceipt::new(
        identity,
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(vec![1, 2, 3]).expect("globally bounded stdout"),
        &CapturedProcessBytes::new(vec![4]).expect("globally bounded stderr"),
    );

    assert_eq!(oversized, Err(ProcessServiceError::OutputTooLarge));
}

#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::pattern_type_mismatch,
    clippy::tests_outside_test_module,
    clippy::wildcard_enum_match_arm,
    reason = "the public malformed-history fixture duplicates its already-asserted requested event and extracts only preparation"
)]
fn duplicate_completion_rejects_malformed_retained_history() {
    let request = request("unit-test", "effect-process-malformed-completion");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-malformed-completion");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let receipt = ProcessReceipt::new(
        identity,
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
    )
    .expect("empty output fits limits");
    let terminal = decide_record_completed(&events, stream.clone(), receipt.clone())
        .expect("valid completion");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();
    let malformed = vec![
        events[0].clone(),
        events[1].clone(),
        events[0].clone(),
        terminal_events[0].clone(),
    ];

    let retry = decide_record_completed(&malformed, stream, receipt)
        .expect_err("duplicate shortcut must not accept malformed history");

    assert_eq!(retry, ProcessServiceError::InvalidHistory);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn spawn_failure_is_content_free_and_bound_to_the_prepared_identity() {
    let request = request("unit-test", "effect-process-spawn-failure");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-spawn-failure");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let failure =
        ProcessSpawnFailure::new(identity, ProcessSpawnFailureCode::ExecutableUnavailable);

    let terminal = decide_record_spawn_failed(&events, stream, failure.clone())
        .expect("exact prepared process should record spawn failure");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();
    let serialized = serde_json::to_string(&terminal_events).expect("failure should serialize");

    assert_eq!(terminal_events.len(), 1);
    assert_eq!(
        terminal_events[0].fact(),
        &ProcessFact::SpawnFailed(failure)
    );
    assert!(!serialized.contains("/nix/store/example/bin/cargo"));
    assert!(!format!("{terminal_events:?}").contains("/nix/store/example/bin/cargo"));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn identical_spawn_failure_reconciles_without_duplicate_terminal() {
    let request = request("unit-test", "effect-process-spawn-retry");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-spawn-retry");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let failure =
        ProcessSpawnFailure::new(identity, ProcessSpawnFailureCode::ExecutableUnavailable);
    let terminal = decide_record_spawn_failed(&events, stream.clone(), failure.clone())
        .expect("first failure publication");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();
    events.extend(terminal_events);

    let retry = decide_record_spawn_failed(&events, stream, failure)
        .expect("identical retained failure should reconcile");
    let (duplicate_events, _) = retry.into_events_and_consistency_streams();

    assert!(duplicate_events.is_empty());
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn timed_out_process_records_one_exact_terminal() {
    let request = request("unit-test", "effect-process-timeout");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-timeout");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let timed_out = ProcessTimedOut::new(identity);

    let terminal = decide_record_timed_out(&events, stream.clone(), timed_out.clone())
        .expect("exact prepared process should record timeout");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();

    assert_eq!(terminal_events.len(), 1);
    assert_eq!(
        terminal_events.first().expect("timeout event").fact(),
        &ProcessFact::TimedOut(timed_out.clone())
    );
    events.extend(terminal_events.clone());
    let retry = decide_record_timed_out(&events, stream.clone(), timed_out.clone())
        .expect("identical timeout should reconcile");
    assert!(retry.into_events_and_consistency_streams().0.is_empty());

    events.extend(terminal_events);
    assert_eq!(
        decide_record_timed_out(&events, stream, timed_out)
            .expect_err("duplicate retained timeout is malformed"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn cancelled_process_records_one_exact_terminal() {
    let request = request("unit-test", "effect-process-cancelled");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-cancelled");
    let publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let cancelled = ProcessCancelled::new(identity);

    let terminal = decide_record_cancelled(&events, stream.clone(), cancelled.clone())
        .expect("exact prepared process should record cancellation");
    let (terminal_events, _) = terminal.into_events_and_consistency_streams();

    assert_eq!(terminal_events.len(), 1);
    assert_eq!(
        terminal_events.first().expect("cancelled event").fact(),
        &ProcessFact::Cancelled(cancelled.clone())
    );
    events.extend(terminal_events.clone());
    let retry = decide_record_cancelled(&events, stream.clone(), cancelled.clone())
        .expect("identical cancellation should reconcile");
    assert!(retry.into_events_and_consistency_streams().0.is_empty());

    events.extend(terminal_events);
    assert_eq!(
        decide_record_cancelled(&events, stream, cancelled)
            .expect_err("duplicate retained cancellation is malformed"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn identical_prepared_history_reconciles_without_duplicate_publication() {
    let request = request("unit-test", "effect-process-retry");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-retry");
    let first = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
        .expect("first admission");
    let (events, _) = first.into_events_and_consistency_streams();

    let retry = decide_process_request(&events, stream, request, &catalog)
        .expect("identical retained preparation should reconcile");
    let (duplicate_events, _) = retry.into_events_and_consistency_streams();

    assert!(duplicate_events.is_empty());
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn conflicting_malformed_and_cross_stream_history_fail_closed() {
    let process_request = request("unit-test", "effect-process-conflict");
    let catalog = catalog("unit-test");
    let process_stream = stream("effect-process-conflict");
    let first = decide_process_request(
        &[],
        process_stream.clone(),
        process_request.clone(),
        &catalog,
    )
    .expect("first admission");
    let (events, _) = first.into_events_and_consistency_streams();

    assert_eq!(
        decide_process_request(
            &events,
            process_stream.clone(),
            request("other", "effect-process-conflict"),
            &catalog,
        )
        .expect_err("conflicting request must fail closed"),
        ProcessServiceError::InvalidHistory
    );
    assert_eq!(
        decide_process_request(
            from_ref(events.first().expect("requested event")),
            process_stream.clone(),
            process_request.clone(),
            &catalog,
        )
        .expect_err("requested-only history is incomplete"),
        ProcessServiceError::InvalidHistory
    );

    let other_request = request("unit-test", "effect-process-other");
    let other =
        decide_process_request(&[], stream("effect-process-other"), other_request, &catalog)
            .expect("other stream admission");
    let (other_events, _) = other.into_events_and_consistency_streams();
    assert_eq!(
        decide_process_request(&other_events, process_stream, process_request, &catalog)
            .expect_err("cross-stream history must fail closed"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn unknown_process_mints_read_only_reconciliation_once_without_redispatch_authority() {
    let request = request("unit-test", "effect-process-unknown");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-unknown");
    let request_publication =
        decide_process_request(&[], stream.clone(), request.clone(), &catalog)
            .expect("configured request should prepare");
    let (mut events, _) = request_publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let prepared_events = events.clone();

    assert_eq!(
        recover_process_reconciliation(&events, &stream)
            .expect_err("prepared history without Unknown is not recoverable reconciliation"),
        ProcessServiceError::InvalidHistory
    );

    let unknown = ProcessUnknown::new(identity.clone());
    assert_eq!(
        decide_record_unknown(
            &events,
            crate::stream("effect-process-unknown-wrong-stream"),
            unknown.clone(),
        )
        .expect_err("Unknown must use the exact effect-owned stream"),
        ProcessServiceError::StreamRequestMismatch
    );
    let unknown_publication = decide_record_unknown(&events, stream.clone(), unknown.clone())
        .expect("exact prepared process should become unknown");
    let (unknown_events, _) = unknown_publication.into_events_and_consistency_streams();
    assert_eq!(unknown_events.len(), 1);
    assert_eq!(
        unknown_events.first().expect("unknown event").fact(),
        &ProcessFact::Unknown(unknown.clone())
    );
    events.extend(unknown_events.clone());

    let unknown_retry = decide_record_unknown(&events, stream.clone(), unknown.clone())
        .expect("an identical Unknown retry should reconcile without publication");
    assert!(
        unknown_retry
            .into_events_and_consistency_streams()
            .0
            .is_empty()
    );
    let mut duplicate_unknown_history = events.clone();
    duplicate_unknown_history.extend(unknown_events);
    assert_eq!(
        decide_record_unknown(&duplicate_unknown_history, stream.clone(), unknown)
            .expect_err("a retained duplicate Unknown fact is malformed"),
        ProcessServiceError::InvalidHistory
    );

    assert_eq!(
        authorize_prepared_process(&events, &stream, &request, &catalog)
            .expect_err("unknown history must never reauthorize process dispatch"),
        ProcessServiceError::InvalidHistory
    );
    let capability = recover_process_reconciliation(&events, &stream)
        .expect("exact signed unknown history should be valid")
        .expect("unknown history should mint one read-only reconciliation capability");
    assert_eq!(capability.prepared_identity(), &identity);
    let reconciled = capability.into_reconciled(ProcessReconciliationOutcome::StillUnknown);
    assert_eq!(
        decide_record_reconciled(&prepared_events, stream.clone(), reconciled.clone())
            .expect_err("reconciliation cannot be recorded before Unknown"),
        ProcessServiceError::ModeledCommandFailed
    );
    let reconciliation_publication =
        decide_record_reconciled(&events, stream.clone(), reconciled.clone())
            .expect("read-only reconciliation should append one closed outcome");
    let (reconciled_events, _) = reconciliation_publication.into_events_and_consistency_streams();
    assert_eq!(reconciled_events.len(), 1);
    assert!(matches!(
        reconciled_events.first().expect("reconciled event").fact(),
        ProcessFact::Reconciled(recorded)
            if recorded.identity() == &identity
                && recorded.outcome() == &ProcessReconciliationOutcome::StillUnknown
    ));
    events.extend(reconciled_events.clone());

    let retry = decide_record_reconciled(&events, stream.clone(), reconciled.clone())
        .expect("an identical retry should reconcile without publication");
    assert!(retry.into_events_and_consistency_streams().0.is_empty());

    assert!(
        recover_process_reconciliation(&events, &stream)
            .expect("reconciled history should remain valid")
            .is_none(),
        "a subsequent restart must not mint another reconciliation capability"
    );

    events.extend(reconciled_events);
    assert_eq!(
        decide_record_reconciled(&events, stream, reconciled)
            .expect_err("a retained duplicate reconciliation is malformed"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::panic,
    clippy::tests_outside_test_module,
    reason = "the public malformed-history scenario destructures locally constructed modeled fixture facts"
)]
fn restart_classification_rejects_malformed_signed_lifecycles() {
    let effect = "effect-process-restart-malformed";
    let process_stream = stream(effect);
    let configured = request("configured", effect);
    let configured_catalog = catalog("configured");
    let publication =
        decide_process_request(&[], process_stream.clone(), configured, &configured_catalog)
            .expect("configured request should prepare");
    let (prepared, _) = publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(borrowed_identity) = prepared.get(1).expect("prepared fact").fact()
    else {
        panic!("second fact should be preparation");
    };
    let identity = borrowed_identity.clone();

    let unknown = decide_record_unknown(
        &prepared,
        process_stream.clone(),
        ProcessUnknown::new(identity.clone()),
    )
    .expect("prepared process may become unknown")
    .into_events_and_consistency_streams()
    .0;
    let receipt = ProcessReceipt::new(
        identity,
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
    )
    .expect("empty output fits limits");
    let completed = decide_record_completed(&prepared, process_stream.clone(), receipt)
        .expect("prepared process may complete")
        .into_events_and_consistency_streams()
        .0;
    let unknown_then_completed = prepared
        .iter()
        .chain(unknown.iter())
        .chain(completed.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        classify_process_restart(&unknown_then_completed, &process_stream)
            .expect_err("Unknown followed by Completed is not a lifecycle"),
        ProcessServiceError::InvalidHistory
    );

    let refused = decide_process_request(
        &[],
        process_stream.clone(),
        request("unconfigured", effect),
        &configured_catalog,
    )
    .expect("unconfigured request should be refused")
    .into_events_and_consistency_streams()
    .0;
    let requested_then_refused = [prepared[0].clone(), refused[0].clone()];
    assert_eq!(
        classify_process_restart(&requested_then_refused, &process_stream)
            .expect_err("a refusal cannot follow a Requested fact"),
        ProcessServiceError::InvalidHistory
    );

    let replacement = request("replacement", effect);
    let replacement_publication = decide_process_request(
        &[],
        process_stream.clone(),
        replacement,
        &catalog("replacement"),
    )
    .expect("replacement request should prepare");
    let (replacement_prepared, _) = replacement_publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(borrowed_replacement_identity) =
        replacement_prepared.get(1).expect("prepared fact").fact()
    else {
        panic!("second fact should be preparation");
    };
    let replacement_identity = borrowed_replacement_identity.clone();
    let replacement_receipt = ProcessReceipt::new(
        replacement_identity,
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
    )
    .expect("empty output fits limits");
    let replacement_completed = decide_record_completed(
        &replacement_prepared,
        process_stream.clone(),
        replacement_receipt,
    )
    .expect("replacement process may complete")
    .into_events_and_consistency_streams()
    .0;
    let mismatched_terminal = [
        prepared[0].clone(),
        prepared[1].clone(),
        replacement_completed[0].clone(),
    ];
    assert_eq!(
        classify_process_restart(&mismatched_terminal, &process_stream)
            .expect_err("terminal identity must match preparation"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::panic,
    clippy::tests_outside_test_module,
    reason = "the public retained-history scenario constructs two modeled identities and a forged signed lifecycle through serialization"
)]
fn request_retry_rejects_reconciled_completion_for_another_identity() {
    let effect = "effect-process-retry-reconciled-mismatch";
    let process_stream = stream(effect);
    let process_request = request("configured", effect);
    let publication = decide_process_request(
        &[],
        process_stream.clone(),
        process_request.clone(),
        &catalog("configured"),
    )
    .expect("configured request should prepare");
    let (mut history, _) = publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(borrowed_identity) = history.get(1).expect("prepared fact").fact()
    else {
        panic!("second fact should be preparation");
    };
    let identity = borrowed_identity.clone();
    let unknown = decide_record_unknown(
        &history,
        process_stream.clone(),
        ProcessUnknown::new(identity),
    )
    .expect("prepared process may become unknown")
    .into_events_and_consistency_streams()
    .0;
    history.extend(unknown);

    let replacement_request = invocation_request(
        "configured",
        effect,
        "effect-process-retry-reconciled-mismatch-replacement",
    );
    let replacement_publication = decide_process_request(
        &[],
        ProcessStream::for_request(&replacement_request).expect("replacement process stream"),
        replacement_request,
        &catalog("configured"),
    )
    .expect("replacement request should prepare");
    let (replacement_history, _) = replacement_publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(replacement_identity) = replacement_history
        .get(1)
        .expect("replacement prepared fact")
        .fact()
    else {
        panic!("second replacement fact should be preparation");
    };
    let mismatched_receipt = ProcessReceipt::new(
        replacement_identity.clone(),
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
    )
    .expect("empty output fits limits");

    let capability = recover_process_reconciliation(&history, &process_stream)
        .expect("unknown history should be valid")
        .expect("unknown history should mint reconciliation capability");
    let malformed = capability.into_reconciled(ProcessReconciliationOutcome::Completed(Box::new(
        mismatched_receipt,
    )));
    let reconciled = decide_record_reconciled(&history, process_stream.clone(), malformed)
        .expect_err("command boundary must reject a mismatched completed reconciliation");
    assert_eq!(reconciled, ProcessServiceError::ModeledCommandFailed);

    let valid_capability = recover_process_reconciliation(&history, &process_stream)
        .expect("unknown history should remain valid")
        .expect("unknown history should mint reconciliation capability");
    let valid_reconciled =
        valid_capability.into_reconciled(ProcessReconciliationOutcome::StillUnknown);
    let reconciled_event =
        decide_record_reconciled(&history, process_stream.clone(), valid_reconciled)
            .expect("matching reconciliation should record")
            .into_events_and_consistency_streams()
            .0;
    let mut malformed_event = serde_json::to_value(
        reconciled_event
            .first()
            .expect("one reconciled event should be published"),
    )
    .expect("reconciled event should serialize");
    malformed_event["fact"]["Reconciled"]["outcome"] =
        serde_json::to_value(ProcessReconciliationOutcome::Completed(Box::new(
            ProcessReceipt::new(
                replacement_identity.clone(),
                ProcessExitStatus::Exited(0),
                &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
                &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
            )
            .expect("empty output fits limits"),
        )))
        .expect("reconciliation outcome should serialize");
    history.push(parsed(serde_json::from_value(malformed_event)));

    assert_eq!(
        decide_process_request(
            &history,
            process_stream,
            process_request,
            &catalog("configured"),
        )
        .expect_err("an exact request retry must reject the malformed retained lifecycle"),
        ProcessServiceError::InvalidHistory
    );
}

#[test]
#[expect(
    clippy::panic,
    clippy::tests_outside_test_module,
    reason = "the public restart scenario destructures locally constructed modeled fixture facts"
)]
fn restart_classification_preserves_valid_closed_and_reconciliation_states() {
    let effect = "effect-process-restart-valid";
    let process_stream = stream(effect);
    let process_request = request("configured", effect);
    let publication = decide_process_request(
        &[],
        process_stream.clone(),
        process_request,
        &catalog("configured"),
    )
    .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let ProcessFact::Prepared(borrowed_identity) = events.get(1).expect("prepared fact").fact()
    else {
        panic!("second fact should be preparation");
    };
    let identity = borrowed_identity.clone();
    assert!(matches!(
        classify_process_restart(&events, &process_stream).expect("prepared history is valid"),
        ProcessRestartState::Prepared(recorded) if recorded == identity
    ));
    assert_retirement_unavailable(
        &events,
        &process_stream,
        "recoverable preparation must not authorize private-artifact retirement",
    );

    let completed_receipt = ProcessReceipt::new(
        identity.clone(),
        ProcessExitStatus::Exited(0),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stdout"),
        &CapturedProcessBytes::new(Vec::new()).expect("empty stderr"),
    )
    .expect("empty output fits limits");
    let completed = decide_record_completed(&events, process_stream.clone(), completed_receipt)
        .expect("prepared process may complete")
        .into_events_and_consistency_streams()
        .0;
    let completed_history = events
        .iter()
        .chain(completed.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert!(matches!(
        classify_process_restart(&completed_history, &process_stream)
            .expect("completed history is valid"),
        ProcessRestartState::Closed
    ));
    assert_retirement_identity(&completed_history, &process_stream, &identity);

    let unknown = decide_record_unknown(
        &events,
        process_stream.clone(),
        ProcessUnknown::new(identity.clone()),
    )
    .expect("prepared process may become unknown")
    .into_events_and_consistency_streams()
    .0;
    events.extend(unknown);
    let ProcessRestartState::Unknown(capability) =
        classify_process_restart(&events, &process_stream).expect("unknown history is valid")
    else {
        panic!("unknown history should mint reconciliation authority");
    };
    assert_retirement_unavailable(
        &events,
        &process_stream,
        "unreconciled unknown authority must retain private artifacts",
    );
    let reconciled = capability.into_reconciled(ProcessReconciliationOutcome::StillUnknown);
    let reconciliation = decide_record_reconciled(&events, process_stream.clone(), reconciled)
        .expect("unknown process may be reconciled")
        .into_events_and_consistency_streams()
        .0;
    events.extend(reconciliation);
    assert!(matches!(
        classify_process_restart(&events, &process_stream).expect("reconciled history is valid"),
        ProcessRestartState::Reconciled(ProcessReconciliationOutcome::StillUnknown)
    ));
    assert_retirement_identity(&events, &process_stream, &identity);

    let refused = decide_process_request(
        &[],
        process_stream.clone(),
        request("unconfigured", effect),
        &catalog("configured"),
    )
    .expect("unconfigured request should be refused")
    .into_events_and_consistency_streams()
    .0;
    assert!(matches!(
        classify_process_restart(&refused, &process_stream).expect("refusal history is valid"),
        ProcessRestartState::Closed
    ));
    assert_retirement_unavailable(
        &refused,
        &process_stream,
        "refusal creates no adapter artifacts to retire",
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn reconciliation_rejects_history_reenveloped_under_another_effect_stream() {
    let request = request("unit-test", "effect-process-forged-source");
    let catalog = catalog("unit-test");
    let source_stream = stream("effect-process-forged-source");
    let publication = decide_process_request(&[], source_stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let unknown = decide_record_unknown(&events, source_stream, ProcessUnknown::new(identity))
        .expect("exact prepared process should become unknown");
    events.extend(unknown.into_events_and_consistency_streams().0);

    let forged_stream = stream("effect-process-forged-envelope");
    let mut wire = parsed(serde_json::to_value(&events));
    for event in wire.as_array_mut().expect("serialized event array") {
        event["stream"] =
            serde_json::Value::String(forged_stream.as_stream_id().as_ref().to_owned());
    }
    let forged_history: Vec<ProcessEvent> = parsed(serde_json::from_value(wire));

    assert_eq!(
        recover_process_reconciliation(&forged_history, &forged_stream)
            .expect_err("event envelopes cannot replace the request-owned process stream"),
        ProcessServiceError::StreamRequestMismatch
    );
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn completed_reconciliation_retains_only_the_exact_content_free_completion_identity() {
    let request = request("unit-test", "effect-process-reconciled-completed");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-reconciled-completed");
    let request_publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = request_publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let unknown = decide_record_unknown(
        &events,
        stream.clone(),
        ProcessUnknown::new(identity.clone()),
    )
    .expect("exact prepared process should become unknown");
    events.extend(unknown.into_events_and_consistency_streams().0);
    let capability = recover_process_reconciliation(&events, &stream)
        .expect("unknown history should be valid")
        .expect("unknown history should mint reconciliation capability");
    let stdout = parsed(CapturedProcessBytes::new(b"private-stdout-marker".to_vec()));
    let stderr = parsed(CapturedProcessBytes::new(Vec::new()));
    let receipt = parsed(ProcessReceipt::new(
        identity.clone(),
        ProcessExitStatus::Exited(0),
        &stdout,
        &stderr,
    ));
    let other_request = crate::request("unit-test", "effect-process-other-completed-identity");
    let other_stream = crate::stream("effect-process-other-completed-identity");
    let other_publication = decide_process_request(&[], other_stream, other_request, &catalog)
        .expect("other configured request should prepare");
    let (other_events, _) = other_publication.into_events_and_consistency_streams();
    let other_identity = other_events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(other_prepared) => Some(other_prepared.clone()),
            _ => None,
        })
        .expect("other prepared identity");
    let other_receipt = parsed(ProcessReceipt::new(
        other_identity,
        ProcessExitStatus::Exited(0),
        &stdout,
        &stderr,
    ));
    let mismatched_capability = recover_process_reconciliation(&events, &stream)
        .expect("unknown history should remain valid")
        .expect("unknown history should mint read-only capability");
    let mismatched = mismatched_capability.into_reconciled(
        ProcessReconciliationOutcome::Completed(Box::new(other_receipt)),
    );
    assert_eq!(
        decide_record_reconciled(&events, stream.clone(), mismatched)
            .expect_err("completed reconciliation must match the exact unknown identity"),
        ProcessServiceError::ModeledCommandFailed
    );
    let reconciled = capability.into_reconciled(ProcessReconciliationOutcome::Completed(Box::new(
        receipt.clone(),
    )));

    assert_eq!(
        decide_record_reconciled(
            &events,
            crate::stream("effect-process-reconciled-wrong-stream"),
            reconciled.clone(),
        )
        .expect_err("reconciliation must use the exact effect-owned stream"),
        ProcessServiceError::StreamRequestMismatch
    );

    let reconciliation_publication = decide_record_reconciled(&events, stream, reconciled)
        .expect("exact completed identity should reconcile");
    let (reconciled_events, _) = reconciliation_publication.into_events_and_consistency_streams();
    assert!(matches!(
        reconciled_events.first().expect("reconciled event").fact(),
        ProcessFact::Reconciled(recorded)
            if recorded.identity() == &identity
                && recorded.outcome()
                    == &ProcessReconciliationOutcome::Completed(Box::new(receipt))
    ));
    let serialized = parsed(serde_json::to_string(&reconciled_events));
    assert!(!serialized.contains("private-stdout-marker"));
    assert!(!serialized.contains("/nix/store/example/bin/cargo"));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn definitely_not_completed_is_a_closed_reconciliation_outcome() {
    let request = request("unit-test", "effect-process-definitely-not-completed");
    let catalog = catalog("unit-test");
    let stream = stream("effect-process-definitely-not-completed");
    let request_publication = decide_process_request(&[], stream.clone(), request, &catalog)
        .expect("configured request should prepare");
    let (mut events, _) = request_publication.into_events_and_consistency_streams();
    let identity = events
        .iter()
        .find_map(|event| match event.fact() {
            ProcessFact::Prepared(identity) => Some(identity.clone()),
            _ => None,
        })
        .expect("prepared identity");
    let unknown = decide_record_unknown(
        &events,
        stream.clone(),
        ProcessUnknown::new(identity.clone()),
    )
    .expect("exact prepared process should become unknown");
    events.extend(unknown.into_events_and_consistency_streams().0);
    let capability = recover_process_reconciliation(&events, &stream)
        .expect("unknown history should be valid")
        .expect("unknown history should mint reconciliation capability");
    let reconciled =
        capability.into_reconciled(ProcessReconciliationOutcome::DefinitelyNotCompleted);

    let reconciliation_publication = decide_record_reconciled(&events, stream, reconciled)
        .expect("definitive absence should append one closed reconciliation");
    let (reconciled_events, _) = reconciliation_publication.into_events_and_consistency_streams();
    assert!(matches!(
        reconciled_events.first().expect("reconciled event").fact(),
        ProcessFact::Reconciled(recorded)
            if recorded.identity() == &identity
                && recorded.outcome()
                    == &ProcessReconciliationOutcome::DefinitelyNotCompleted
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "public-boundary integration scenarios remain at crate scope"
)]
fn process_admission_rejects_only_a_distinct_stream_past_the_recovery_budget() {
    let effect = parsed(EffectId::parse("effect-process-admission-budget"));
    let streams = (0..MAX_PROCESS_INVOCATION_STREAMS)
        .map(|index| {
            ProcessStream::for_invocation(
                &effect,
                &parsed(ProcessInvocationId::parse(&format!("invocation-{index}"))),
            )
            .expect("bounded fixture invocation should form a stream")
        })
        .collect::<Vec<_>>();
    let verified_stream_ids = streams
        .iter()
        .map(|stream| stream.as_stream_id().clone())
        .collect::<Vec<_>>();

    admit_process_invocation(&effect, &verified_stream_ids, &streams[0])
        .expect("retrying an admitted invocation remains usable at the bound");

    let overflow = ProcessStream::for_invocation(
        &effect,
        &parsed(ProcessInvocationId::parse("invocation-overflow")),
    )
    .expect("overflow fixture invocation should form a stream");
    let error = admit_process_invocation(&effect, &verified_stream_ids, &overflow)
        .expect_err("a distinct 65th stream must be rejected before publication");
    assert_eq!(error, ProcessServiceError::InvocationLimitReached);
    assert_eq!(error.code(), "process_invocation_limit_reached");

    let other_effect = parsed(EffectId::parse("effect-process-admission-other"));
    let other = ProcessStream::for_invocation(
        &other_effect,
        &parsed(ProcessInvocationId::parse("invocation-other")),
    )
    .expect("other-effect invocation should form a stream");
    admit_process_invocation(&other_effect, &verified_stream_ids, &other)
        .expect("another effect owns an independent admission budget");
}
