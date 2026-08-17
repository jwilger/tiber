#![forbid(unsafe_code)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId, TaskTitle};
use tiber_tasks_service::command::{UpdateTaskDetails, decide_update_task_details};

fn created() -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_created", "stream_id": "tiber:board",
        "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
            "committed_at": "2026-08-16T00:00:00Z", "context": "old", "notes": [],
            "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
            "stem": "20260816-test-details", "subtasks": [], "summary": "old",
            "tags": ["native"], "title": "Old" }
    })).expect("valid created fixture")
}

fn request() -> UpdateTaskDetails {
    UpdateTaskDetails::new(
        TaskId::parse("20260816-test-details").expect("valid task ID"),
        TaskTitle::parse("New").expect("valid title"),
        "new summary".to_owned(),
        "new context".to_owned(),
    )
}

fn updated(title: &str, summary: &str, context: &str) -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_details_updated", "stream_id": "tiber:task:20260816-test-details",
        "stem": "20260816-test-details", "title": title, "tags": ["native"],
        "summary": summary, "context": context
    })).expect("valid updated fixture")
}

#[test]
fn details_publication_fences_every_stream_accepted_by_its_fold() {
    let publication = decide_update_task_details(&[created()], &request())
        .expect("existing task requires details publication")
        .expect("changed details require publication");
    let (_event, streams) = publication.into_event_and_consistency_streams();
    assert_eq!(streams[0].as_ref(), "tiber:board");
    assert_eq!(streams[1].as_ref(), "tiber:task:20260816-test-details");
}

#[test]
fn task_details_model_consumes_all_provenance() {
    let _decision = decide_update_task_details(&[created()], &request())
        .expect("fixture update links the modeled command");
    let report = check().expect("complete native task-details model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty());
}

#[test]
fn exact_durable_details_reconcile_without_a_second_publication() {
    let decision = decide_update_task_details(
        &[created(), updated("New", "new summary", "new context")],
        &request(),
    ).expect("exact durable state is valid");
    assert!(decision.is_none());
}

#[test]
fn a_later_different_update_does_not_reconcile_an_old_ambiguous_attempt() {
    let decision = decide_update_task_details(
        &[
            created(),
            updated("New", "new summary", "new context"),
            updated("Changed later", "different", "different"),
        ],
        &request(),
    ).expect("changed durable state is valid");
    assert!(decision.is_some());
}
