#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use eventcore::model::{CheckStatus, check};
    use serde_json::{Value, json};
    use tiber_tasks_core::{TaskEvent, TaskId};
    use tiber_tasks_service::{
        TaskHistory,
        command::{AcceptanceIndex, CheckAcceptance, TaskCommandError, decide_check_acceptance},
    };

    const TASK_ID: &str = "20260810-hwcc-prove-tiber-can-run-an-isolated-codex-conversation";

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the modeled command fixture fails with checker diagnostics when provenance is incomplete"
    )]
    fn check_acceptance_has_complete_model_provenance() {
        let report = check().expect("complete check-acceptance model provenance");
        assert_eq!(report.status, CheckStatus::Verified);
        assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the durable-wire fixture fails fast and returns its parsed task fact directly"
    )]
    fn event(value: Value) -> TaskEvent {
        serde_json::from_value(value).expect("fixture matches retained TaskEvent wire vocabulary")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the mixed-stream fixture returns its canonical task history directly"
    )]
    fn legacy_replaced_acceptance_history() -> TaskHistory {
        TaskHistory::from_ordered_events(vec![
            event(json!({
                "event": "task_created",
                "stream_id": format!("tiber:task:{TASK_ID}"),
                "task": {
                    "acceptance": [],
                    "blocked_by": [], "blocks": [], "claim": null,
                    "committed_at": "2026-08-12T00:00:00Z", "context": "", "notes": [],
                    "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                    "stem": TASK_ID, "subtasks": [], "summary": "", "tags": [], "title": "HWCC"
                }
            })),
            event(json!({
                "event": "task_acceptance_added", "stream_id": format!("tiber:task:{TASK_ID}"), "stem": TASK_ID,
                "item": {"checked": false, "text": "A"}
            })),
            event(json!({
                "event": "task_acceptance_added", "stream_id": format!("tiber:task:{TASK_ID}"), "stem": TASK_ID,
                "item": {"checked": false, "text": "obsolete B"}
            })),
            event(json!({
                "event": "task_acceptance_added", "stream_id": format!("tiber:task:{TASK_ID}"), "stem": TASK_ID,
                "item": {"checked": false, "text": "C"}
            })),
            event(json!({
                "event": "task_acceptance_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "item": {"checked": false, "text": "D"}
            })),
            event(json!({
                "event": "task_acceptance_removed", "stream_id": "tiber:board", "stem": TASK_ID,
                "index": 1
            })),
            event(json!({
                "event": "task_acceptance_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "item": {"checked": false, "text": "E"}
            })),
            event(json!({
                "event": "task_subtask_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "subtask": {"after": [], "checked": false, "id": "s4", "title": "duplicate retained subtask"}
            })),
            event(json!({
                "event": "task_subtask_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "subtask": {"after": [], "checked": false, "id": "s4", "title": "duplicate retained subtask"}
            })),
        ])
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this pure public command scenario uses fail-fast typed fixture and decision assertions"
    )]
    fn checks_the_fourth_rendered_acceptance_item_across_legacy_and_board_streams() {
        let task = TaskId::parse(TASK_ID).expect("fixture task ID is valid");
        let request = CheckAcceptance::new(
            task.clone(),
            AcceptanceIndex::parse_one_based("4").expect("fourth item is a valid human index"),
        );

        let decided =
            decide_check_acceptance(legacy_replaced_acceptance_history().events(), &request)
                .expect("the fourth canonical checklist entry is addressable")
                .expect("unchecked entry requires one closed publication");

        let expected_index: usize = 3;
        assert_eq!(
            serde_json::to_value(TaskEvent::TaskAcceptanceChecked(
                decided.checked_fact().clone(),
            ))
            .expect("fact serializes"),
            json!({
                "event": "task_acceptance_checked", "stream_id": "tiber:board", "stem": TASK_ID,
                "index": expected_index, "checked": true
            })
        );
        assert_eq!(
            decided
                .consistency_streams()
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>(),
            vec!["tiber:board".to_owned(), format!("tiber:task:{TASK_ID}")]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the malformed request test uses a direct stable-code assertion"
    )]
    fn rejects_zero_as_a_human_facing_acceptance_position() {
        assert_eq!(
            AcceptanceIndex::parse_one_based("0")
                .expect_err("zero is not a one-based acceptance index")
                .code(),
            "tasks_invalid_acceptance_index"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the absent-position scenario uses a direct stable typed failure assertion"
    )]
    fn rejects_a_missing_current_acceptance_position() {
        let task = TaskId::parse(TASK_ID).expect("fixture task ID is valid");
        let request = CheckAcceptance::new(task, AcceptanceIndex::zero_based(4));
        assert_eq!(
            decide_check_acceptance(legacy_replaced_acceptance_history().events(), &request)
                .expect_err("only four current entries exist"),
            TaskCommandError::AcceptanceItemMissing {
                task: TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
                index: AcceptanceIndex::zero_based(4),
            }
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the public command boundary reports a stable failure for malformed retained stream ownership"
    )]
    fn rejects_target_acceptance_history_from_a_foreign_task_stream() {
        let task = TaskId::parse(TASK_ID).expect("fixture task ID is valid");
        let request = CheckAcceptance::new(task, AcceptanceIndex::zero_based(3));
        let mut events = legacy_replaced_acceptance_history().events().to_vec();
        let foreign_checked_index: usize = 0;
        events.push(event(json!({
            "event": "task_acceptance_checked",
            "stream_id": "tiber:task:another-task",
            "stem": TASK_ID,
            "index": foreign_checked_index,
            "checked": true
        })));

        assert_eq!(
            decide_check_acceptance(&events, &request)
                .expect_err("a target acceptance fact in another task stream is not authoritative")
                .code(),
            "tasks_command_target_task_fact_unexpected_stream"
        );
    }
}
