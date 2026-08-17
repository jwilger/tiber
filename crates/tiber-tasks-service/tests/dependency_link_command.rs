#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    clippy::tests_outside_test_module,
    reason = "the bounded EventCore fixture fails loudly, deliberately destructures the exact two-event opaque publication, and uses single-purpose scenario builders"
)]

use eventcore::model::{CheckStatus, check};
use tiber_tasks_core::{TaskEvent, TaskId};
use tiber_tasks_service::command::{LinkBlockedBy, decide_link_blocked_by};

const TASK: &str = "20260816-test-dependent";
const BLOCKER: &str = "20260816-test-blocker";
const TARGET_BLOCKS: &str = "20260816-test-target-blocks";
const TARGET_BLOCKED_BY: &str = "20260816-test-target-blocked-by";
const BLOCKER_BLOCKS: &str = "20260816-test-blocker-blocks";
const BLOCKER_BLOCKED_BY: &str = "20260816-test-blocker-blocked-by";

fn created(task: &str) -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_created", "stream_id": format!("tiber:task:{task}"),
        "task": { "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
            "committed_at": "2026-08-16T00:00:00Z", "context": "", "notes": [],
            "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
            "stem": task, "subtasks": [], "summary": "", "tags": [], "title": "Test" }
    }))
    .expect("valid task fixture")
}

fn target_side_only() -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_links_changed", "stream_id": "tiber:board",
        "stem": TASK, "blocks": [], "blocked_by": [BLOCKER]
    }))
    .expect("valid one-sided dependency fixture")
}

fn blocker_side_only() -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_links_changed", "stream_id": "tiber:board",
        "stem": BLOCKER, "blocks": [TASK], "blocked_by": []
    }))
    .expect("valid one-sided dependency fixture")
}

fn validation_repaired_links() -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_validation_repaired", "stream_id": "tiber:board",
        "link_changes": [
            { "stream_id": "tiber:board", "stem": TASK,
              "blocks": [TARGET_BLOCKS], "blocked_by": [TARGET_BLOCKED_BY] },
            { "stream_id": "tiber:board", "stem": BLOCKER,
              "blocks": [BLOCKER_BLOCKS], "blocked_by": [BLOCKER_BLOCKED_BY] }
        ],
        "repairs": []
    }))
    .expect("valid validation-repair fixture")
}

fn validation_repaired_links_on(outer_stream: &str, embedded_stream: &str) -> TaskEvent {
    serde_json::from_value(serde_json::json!({
        "event": "task_validation_repaired", "stream_id": outer_stream,
        "link_changes": [{ "stream_id": embedded_stream, "stem": TASK,
            "blocks": [TARGET_BLOCKS], "blocked_by": [TARGET_BLOCKED_BY] }],
        "repairs": []
    }))
    .expect("valid malformed validation-repair fixture")
}

fn request() -> LinkBlockedBy {
    LinkBlockedBy::new(
        TaskId::parse(TASK).expect("valid task ID"),
        TaskId::parse(BLOCKER).expect("valid blocker ID"),
    )
}

#[test]
fn dependency_link_fences_board_and_both_endpoint_streams() {
    let publication = decide_link_blocked_by(&[created(TASK), created(BLOCKER)], &request())
        .expect("two existing tasks can be linked")
        .expect("missing dependency requires publication");
    let (_events, streams) = publication.into_events_and_consistency_streams();
    assert_eq!(streams[0].as_ref(), "tiber:board");
    assert_eq!(streams[1].as_ref(), format!("tiber:task:{TASK}"));
    assert_eq!(streams[2].as_ref(), format!("tiber:task:{BLOCKER}"));
}

#[test]
fn dependency_link_model_consumes_all_provenance() {
    let _publication = decide_link_blocked_by(&[created(TASK), created(BLOCKER)], &request())
        .expect("fixture link registers the modeled command");
    let report = check().expect("complete dependency-link model provenance");
    assert_eq!(report.status, CheckStatus::Verified);
    assert!(report.warnings.is_empty());
}

#[test]
fn one_sided_dependency_history_requires_reciprocal_repair() {
    for one_sided_fact in [target_side_only(), blocker_side_only()] {
        let publication = decide_link_blocked_by(
            &[created(TASK), created(BLOCKER), one_sided_fact],
            &request(),
        )
        .expect("one-sided durable history is readable");
        assert!(publication.is_some());
    }
}

#[test]
fn validation_repair_link_replacements_are_preserved_by_a_new_dependency() {
    let publication = decide_link_blocked_by(
        &[created(TASK), created(BLOCKER), validation_repaired_links()],
        &request(),
    )
    .expect("validation-repaired link history is readable")
    .expect("new reciprocal dependency requires publication");
    let (events, _streams) = publication.into_events_and_consistency_streams();
    let TaskEvent::TaskLinksChanged(target) = &events[0] else {
        panic!("first publication fact must replace target links");
    };
    assert_eq!(
        target.blocks.iter().map(TaskId::as_str).collect::<Vec<_>>(),
        [TARGET_BLOCKS]
    );
    assert_eq!(
        target
            .blocked_by
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>(),
        [TARGET_BLOCKED_BY, BLOCKER]
    );
    let TaskEvent::TaskLinksChanged(blocker) = &events[1] else {
        panic!("second publication fact must replace blocker links");
    };
    assert_eq!(
        blocker
            .blocks
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>(),
        [BLOCKER_BLOCKS, TASK]
    );
    assert_eq!(
        blocker
            .blocked_by
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>(),
        [BLOCKER_BLOCKED_BY]
    );
}

#[test]
fn validation_repair_link_history_enforces_outer_and_embedded_stream_ownership() {
    let cases = [
        validation_repaired_links_on(&format!("tiber:task:{TASK}"), "tiber:board"),
        validation_repaired_links_on("tiber:board", &format!("tiber:task:{BLOCKER}")),
    ];
    for malformed in cases {
        let error =
            decide_link_blocked_by(&[created(TASK), created(BLOCKER), malformed], &request())
                .expect_err("foreign validation-repair ownership must be rejected");
        assert_eq!(
            error.code(),
            "tasks_command_target_task_fact_unexpected_stream"
        );
    }
}
