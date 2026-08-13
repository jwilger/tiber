#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use eventcore::model::{CheckStatus, check};
    use serde_json::{Value, json};
    use tiber_tasks_core::{TaskEvent, TaskId, TaskStatus};
    use tiber_tasks_service::{
        TaskHistory,
        command::{StartTask, TaskCommandError, decide_start_task},
    };

    const BLOCKER_ID: &str = "20260810-hwcc-prove-tiber-can-run-an-isolated-codex-conversation";
    const TASK_ID: &str = "20260810-3tb3-make-tiber-own-sessions-agents-tasks-and-workflow";

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
        reason = "the activation fixture returns the exact canonical history required by the public command boundary"
    )]
    fn eligible_backlog_history() -> TaskHistory {
        TaskHistory::from_ordered_events(vec![
            event(json!({
                "event": "task_created",
                "stream_id": format!("tiber:task:{BLOCKER_ID}"),
                "task": {
                    "acceptance": [], "blocked_by": [], "blocks": [TASK_ID], "claim": null,
                    "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                    "pr_mr_status": null, "pr_mr_url": null, "status": "done",
                    "stem": BLOCKER_ID, "subtasks": [], "summary": "", "tags": [], "title": "HWCC"
                }
            })),
            event(json!({
                "event": "task_created",
                "stream_id": format!("tiber:task:{TASK_ID}"),
                "task": {
                    "acceptance": [], "blocked_by": [BLOCKER_ID], "blocks": [], "claim": null,
                    "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                    "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
                    "stem": TASK_ID, "subtasks": [], "summary": "", "tags": [], "title": "Own workflow"
                }
            })),
            event(json!({
                "event": "board_reordered", "stream_id": "tiber:board", "order": [TASK_ID]
            })),
        ])
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the public activation request fixture parses one valid durable task identity"
    )]
    fn start_request() -> StartTask {
        StartTask::new(TaskId::parse(TASK_ID).expect("fixture task ID is valid"))
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the modeled command fixture fails with checker diagnostics when activation provenance is incomplete"
    )]
    fn task_activation_has_complete_model_provenance() {
        let report = check().expect("complete task-activation model provenance");
        assert_eq!(report.status, CheckStatus::Verified);
        assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public command scenario uses fail-fast fixtures and verifies the closed activation fact plus its exact consistency fence"
    )]
    fn activates_the_first_unblocked_backlog_task_on_the_board_and_task_fence() {
        let decided = decide_start_task(eligible_backlog_history().events(), &start_request())
            .expect("the strict next eligible backlog task is startable")
            .expect("a backlog task requires one closed activation publication");

        assert_eq!(
            serde_json::to_value(TaskEvent::TaskTransitioned(
                decided.transitioned_fact().clone(),
            ))
            .expect("activation fact serializes"),
            json!({
                "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
                "status": "in-progress", "claim": null
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
        reason = "the retry scenario uses the exact retained activation fact and a no-publication assertion"
    )]
    fn returns_none_when_the_target_is_already_the_sole_active_task() {
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "in-progress", "claim": null
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect("the exact sole active task remains an idempotent activation retry"),
            None
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the retained terminal-state fixture parses the exact stable command refusal"
    )]
    fn rejects_a_done_task_retained_in_board_order_without_an_activation_publication() {
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("a done task must refuse activation rather than emit a publication"),
            TaskCommandError::TaskActivationNotBacklog {
                task: TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
                status: TaskStatus::Done,
            }
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the unresolved-blocker scenario uses a direct current lifecycle fact and stable activation refusal"
    )]
    fn rejects_a_backlog_target_with_an_unfinished_blocker() {
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": BLOCKER_ID,
            "status": "backlog", "claim": null
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("an unfinished prerequisite must block native activation")
                .code(),
            "tasks_command_task_activation_blocked"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the strict-priority scenario uses one earlier eligible task and a stable bypass refusal"
    )]
    fn rejects_a_target_that_would_bypass_the_first_eligible_backlog_task() {
        let earlier = "20260810-aaaa-earlier-eligible";
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_created", "stream_id": format!("tiber:task:{earlier}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "backlog",
                "stem": earlier, "subtasks": [], "summary": "", "tags": [], "title": "Earlier"
            }
        })));
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": [earlier, TASK_ID]
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("a strict-next command cannot bypass an earlier eligible task")
                .code(),
            "tasks_command_task_activation_not_next_eligible"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the duplicate-order scenario uses a direct strict board fact and stable order-drift refusal"
    )]
    fn rejects_a_target_that_appears_more_than_once_in_the_strict_board_order() {
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": [TASK_ID, TASK_ID]
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("the addressed task must have exactly one strict board entry")
                .code(),
            "tasks_command_task_activation_order_drift"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the one-active-task scenario uses one current lifecycle fact and stable active-task refusal"
    )]
    fn rejects_a_new_activation_while_another_task_is_active() {
        let active = "20260810-aaaa-other-active";
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_created", "stream_id": format!("tiber:task:{active}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                "stem": active, "subtasks": [], "summary": "", "tags": [], "title": "Active"
            }
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("a current active task must be continued before another starts")
                .code(),
            "tasks_command_task_activation_active_task"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the corrupt-active-state scenario supplies two current lifecycle facts and stable one-active-task diagnostics"
    )]
    fn rejects_history_with_multiple_active_tasks() {
        let active = "20260810-aaaa-other-active";
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "task_created", "stream_id": format!("tiber:task:{active}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                "stem": active, "subtasks": [], "summary": "", "tags": [], "title": "Active"
            }
        })));
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "in-progress", "claim": null
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("more than one active task is ambiguous rather than startable")
                .code(),
            "tasks_command_multiple_active_tasks"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the malformed-board scenario uses a direct non-board order fact and stable activation-history diagnostics"
    )]
    fn rejects_an_order_fact_that_is_not_board_authoritative() {
        let mut events = eligible_backlog_history().events().to_vec();
        events.push(event(json!({
            "event": "board_reordered", "stream_id": format!("tiber:task:{TASK_ID}"), "order": [TASK_ID]
        })));

        assert_eq!(
            decide_start_task(&events, &start_request())
                .expect_err("a strict order fact must come from the board stream")
                .code(),
            "tasks_command_task_activation_malformed_history"
        );
    }
}
