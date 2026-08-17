#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::tests_outside_test_module,
    reason = "the bounded EventCore contract fixture fails loudly and directly inspects its exact two-fact publication"
)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId, TaskTitle};
use tiber_tasks_service::command::{CreateTask, TaskCreationDecision, decide_create_task};

#[test]
fn task_creation_model_consumes_all_provenance() {
    let request = CreateTask::new(
        TaskId::parse("20260816-abcd").expect("fixture prefix is valid"),
        "2026-08-16T00:00:00Z".to_owned(),
        TaskTitle::parse("Modeled creation").expect("fixture title is valid"),
    );
    let _decision =
        decide_create_task(&[], &request).expect("fixture creation links the modeled command");
    let report = check().expect("complete native task-creation model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty());
}

#[test]
fn task_creation_order_excludes_a_validly_removed_task() {
    let events = [
        serde_json::from_value::<TaskEvent>(serde_json::json!({
            "event": "task_created",
            "stream_id": "tiber:board",
            "task": {
                "acceptance": [],
                "blocked_by": [],
                "blocks": [],
                "claim": null,
                "committed_at": "2026-08-16T00:00:00Z",
                "context": "",
                "notes": [],
                "pr_mr_status": null,
                "pr_mr_url": null,
                "stem": "20260816-old-removed",
                "subtasks": [],
                "summary": "",
                "tags": [],
                "title": "Removed task",
                "status": "backlog"
            }
        }))
        .expect("created fixture should decode"),
        serde_json::from_value::<TaskEvent>(serde_json::json!({
            "event": "board_reordered",
            "stream_id": "tiber:board",
            "order": ["20260816-old-removed"]
        }))
        .expect("order fixture should decode"),
        serde_json::from_value::<TaskEvent>(serde_json::json!({
            "event": "task_removed",
            "stream_id": "tiber:board",
            "stem": "20260816-old-removed"
        }))
        .expect("removal fixture should decode"),
    ];
    let request = CreateTask::new_implicit(
        TaskId::parse("20260816-new").expect("fixture prefix is valid"),
        "2026-08-16T00:00:00Z".to_owned(),
        TaskTitle::parse("New task").expect("fixture title is valid"),
    );

    let TaskCreationDecision::Publish(publication) =
        decide_create_task(&events, &request).expect("valid history should decide publication")
    else {
        panic!("distinct implicit request should publish");
    };
    let (facts, _) = publication.into_events_and_consistency_streams();
    let TaskEvent::BoardReordered(order) = &facts[1] else {
        panic!("second modeled fact should be the resulting strict order");
    };
    assert_eq!(
        order.order,
        [TaskId::parse("20260816-new-new-task").expect("expected task ID is valid")]
    );
}
