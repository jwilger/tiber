#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::tests_outside_test_module,
    reason = "the bounded EventCore fixture fails loudly and directly inspects the modeled priority publication"
)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId};
use tiber_tasks_service::command::{PrioritizeTask, decide_prioritize_task};

const FIRST: &str = "20260816-test-priority-first";
const SECOND: &str = "20260816-test-priority-second";

fn event(value: serde_json::Value) -> TaskEvent {
    serde_json::from_value(value).expect("valid task fixture")
}

#[expect(
    clippy::single_call_fn,
    reason = "the named single-purpose fixture represents the authoritative board-order fact for these bounded command tests"
)]
fn order() -> TaskEvent {
    event(serde_json::json!({
        "event": "board_reordered",
        "stream_id": "tiber:board",
        "order": [FIRST, SECOND]
    }))
}

fn created(task: &str) -> TaskEvent {
    event(serde_json::json!({
        "event": "task_created", "stream_id": format!("tiber:task:{task}"),
        "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
            "committed_at": "2026-08-16T00:00:00Z", "context": "", "notes": [],
            "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
            "stem": task, "subtasks": [], "summary": "", "tags": [], "title": "Test" }
    }))
}

fn history() -> [TaskEvent; 3] {
    [created(FIRST), created(SECOND), order()]
}

fn request() -> PrioritizeTask {
    PrioritizeTask::new(
        TaskId::parse(SECOND).expect("valid moved task ID"),
        TaskId::parse(FIRST).expect("valid target task ID"),
    )
}

#[test]
fn priority_publication_fences_board_and_both_addressed_tasks() {
    let publication = decide_prioritize_task(&history(), &request())
        .expect("strict order is valid")
        .expect("different order requires publication");
    let (_event, streams) = publication.into_event_and_consistency_streams();
    assert_eq!(streams[0].as_ref(), "tiber:board");
    assert_eq!(streams[1].as_ref(), format!("tiber:task:{SECOND}"));
    assert_eq!(streams[2].as_ref(), format!("tiber:task:{FIRST}"));
}

#[test]
fn priority_model_consumes_all_provenance() {
    let _decision = decide_prioritize_task(&history(), &request())
        .expect("fixture priority request links the modeled command");
    let report = check().expect("complete task-priority model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty());
}
