#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::tests_outside_test_module,
    reason = "the bounded EventCore fixture fails loudly and directly inspects modeled acceptance-add publications"
)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId};
use tiber_tasks_service::command::{AddAcceptance, decide_add_acceptance};

const TASK: &str = "20260816-test-acceptance-add";
const CRITERION: &str = "The public result survives restart.";

fn event(value: serde_json::Value) -> TaskEvent {
    serde_json::from_value(value).expect("valid task fixture")
}

fn created() -> TaskEvent {
    event(serde_json::json!({
        "event": "task_created", "stream_id": "tiber:board",
        "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
            "committed_at": "2026-08-16T00:00:00Z", "context": "", "notes": [],
            "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
            "stem": TASK, "subtasks": [], "summary": "", "tags": [], "title": "Test" }
    }))
}

fn added() -> TaskEvent {
    event(serde_json::json!({
        "event": "task_acceptance_added", "stream_id": format!("tiber:task:{TASK}"),
        "stem": TASK, "item": { "checked": false, "text": CRITERION }
    }))
}

#[expect(
    clippy::single_call_fn,
    reason = "the named removal fixture keeps the current-state replay scenario explicit"
)]
fn removed() -> TaskEvent {
    event(serde_json::json!({
        "event": "task_acceptance_removed", "stream_id": format!("tiber:task:{TASK}"),
        "stem": TASK, "index": 0
    }))
}

fn request() -> AddAcceptance {
    AddAcceptance::new(
        TaskId::parse(TASK).expect("valid task ID"),
        CRITERION.to_owned(),
    )
}

#[test]
fn acceptance_add_fences_every_stream_accepted_by_its_fold() {
    let publication = decide_add_acceptance(&[created()], &request())
        .expect("existing task is valid")
        .expect("missing criterion requires publication");
    let (_event, streams) = publication.into_event_and_consistency_streams();
    assert_eq!(streams[0].as_ref(), "tiber:board");
    assert_eq!(streams[1].as_ref(), format!("tiber:task:{TASK}"));
}

#[test]
fn acceptance_add_model_consumes_all_provenance() {
    let _decision = decide_add_acceptance(&[created()], &request())
        .expect("fixture addition links the modeled command");
    let report = check().expect("complete acceptance-add model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty());
}

#[test]
fn exact_durable_criterion_reconciles_without_a_second_publication() {
    let decision = decide_add_acceptance(&[created(), added()], &request())
        .expect("exact durable state is valid");
    assert!(decision.is_none());
}

#[test]
fn a_removed_matching_criterion_can_be_added_again() {
    let decision = decide_add_acceptance(&[created(), added(), removed()], &request())
        .expect("current history is valid");
    assert!(decision.is_some());
}
