#![forbid(unsafe_code)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId};
use tiber_tasks_service::command::{AbandonTask, TaskCommandError, decide_abandon_task};

const TASK: &str = "20260816-test-abandonment";

#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    reason = "the bounded fixture fails loudly on invalid fixed wire facts and returns the parsed event directly"
)]
fn event(value: serde_json::Value) -> TaskEvent {
    serde_json::from_value(value).expect("valid abandonment fixture")
}

#[expect(
    clippy::needless_return,
    reason = "the bounded fixture uses an explicit return to satisfy the workspace's implicit-return policy"
)]
fn history() -> [TaskEvent; 2] {
    return [
        event(serde_json::json!({
            "event": "task_created", "stream_id": format!("tiber:task:{TASK}"),
            "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-16T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
                "stem": TASK, "subtasks": [], "summary": "", "tags": [], "title": "Test" }
        })),
        event(serde_json::json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": [TASK]
        })),
    ];
}

#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    reason = "the fixed request fixture fails loudly on invalid test identity and returns the semantic request directly"
)]
fn request() -> AbandonTask {
    AbandonTask::new(TaskId::parse(TASK).expect("valid task ID"))
}

#[expect(
    clippy::needless_return,
    reason = "the bounded fixture uses an explicit return to satisfy the workspace's implicit-return policy"
)]
fn transitioned(status: &str) -> TaskEvent {
    return event(serde_json::json!({
        "event": "task_transitioned", "stream_id": format!("tiber:task:{TASK}"),
        "stem": TASK, "status": status, "claim": null
    }));
}

#[expect(
    clippy::needless_return,
    reason = "the bounded fixture uses an explicit return to satisfy the workspace's implicit-return policy"
)]
fn historical(event_name: &str) -> TaskEvent {
    return event(serde_json::json!({
        "event": event_name, "stream_id": format!("tiber:task:{TASK}"), "stem": TASK
    }));
}

#[test]
#[expect(
    clippy::expect_used,
    clippy::tests_outside_test_module,
    reason = "the root integration test fails loudly while inspecting the closed abandonment publication's exact consistency fence"
)]
fn abandonment_publication_fences_board_and_the_addressed_task() {
    let publication = decide_abandon_task(&history(), &request())
        .expect("open task can be abandoned")
        .expect("open task requires publication");
    let (_events, streams) = publication.into_events_and_consistency_streams();
    assert_eq!(streams[0].as_ref(), "tiber:board");
    assert_eq!(streams[1].as_ref(), format!("tiber:task:{TASK}"));
}

#[test]
#[expect(
    clippy::expect_used,
    clippy::tests_outside_test_module,
    reason = "the root integration test fails loudly while requiring the owning model's complete provenance report"
)]
fn abandonment_model_consumes_all_provenance() {
    let _publication = decide_abandon_task(&history(), &request())
        .expect("fixture abandonment request links the modeled command");
    let report = check().expect("complete task-abandonment model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against malformed lifetime ordering"
)]
fn abandonment_rejects_a_transition_before_task_creation() {
    let [created, order] = history();
    let result = decide_abandon_task(&[transitioned("backlog"), created, order], &request());
    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against a commit closure preceding task creation"
)]
fn abandonment_rejects_commit_closure_before_task_creation() {
    let closure = event(serde_json::json!({
        "event": "tasks_closed_from_commit_trailers",
        "stream_id": "tiber:board",
        "stems": [TASK],
        "order": []
    }));
    let [created, order] = history();

    let result = decide_abandon_task(&[closure, created, order], &request());

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against duplicate task creation"
)]
fn abandonment_rejects_duplicate_task_creation() {
    let [created, order] = history();
    let duplicate = event(serde_json::json!({
        "event": "task_created", "stream_id": format!("tiber:task:{TASK}"),
        "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
            "committed_at": "2026-08-16T00:00:01Z", "context": "", "notes": [],
            "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
            "stem": TASK, "subtasks": [], "summary": "", "tags": [], "title": "Duplicate" }
    }));
    let result = decide_abandon_task(&[created, duplicate, order], &request());
    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against duplicate unrelated strict-order membership"
)]
fn abandonment_rejects_duplicate_unrelated_board_membership() {
    let [created, _order] = history();
    let order = event(serde_json::json!({
        "event": "board_reordered", "stream_id": "tiber:board",
        "order": [TASK, "20260816-test-other", "20260816-test-other"]
    }));

    let result = decide_abandon_task(&[created, order], &request());

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against an overwritten malformed order carrier"
)]
fn abandonment_rejects_an_overwritten_duplicate_order_carrier() {
    let [created, valid_order] = history();
    let duplicate_order = event(serde_json::json!({
        "event": "board_reordered", "stream_id": "tiber:board",
        "order": [TASK, "20260816-test-other", "20260816-test-other"]
    }));

    let result = decide_abandon_task(&[created, duplicate_order, valid_order], &request());

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against an incomplete terminal retry"
)]
fn abandonment_rejects_incomplete_terminal_retry() {
    let [created, order] = history();

    let result = decide_abandon_task(&[created, order, transitioned("abandoned")], &request());

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against terminal retry history lacking board-order authority"
)]
fn abandonment_rejects_terminal_retry_without_any_board_order() {
    let [created, _order] = history();

    let result = decide_abandon_task(&[created, transitioned("abandoned")], &request());

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against reopening one terminal task lifetime"
)]
fn abandonment_rejects_reopening_a_terminal_task_lifetime() {
    let [created, order] = history();

    let result = decide_abandon_task(
        &[
            created,
            transitioned("done"),
            transitioned("backlog"),
            order,
        ],
        &request(),
    );

    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentMalformedHistory)
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against a historical terminal closure"
)]
fn abandonment_treats_historical_closure_as_terminal() {
    let [created, order] = history();
    let result = decide_abandon_task(
        &[created, historical("task_closed_from_trailer"), order],
        &request(),
    );
    assert!(matches!(
        result,
        Err(TaskCommandError::TaskAbandonmentNotBacklog { .. })
    ));
}

#[test]
#[expect(
    clippy::tests_outside_test_module,
    reason = "the root integration test exercises the public command boundary against the end of one historically removed lifetime"
)]
fn abandonment_treats_historical_removal_as_the_end_of_the_task_lifetime() {
    let [created, order] = history();
    let result = decide_abandon_task(&[created, historical("task_removed"), order], &request());
    assert!(matches!(result, Err(TaskCommandError::TaskMissing { .. })));
}
