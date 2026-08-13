#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use eventcore::model::{CheckStatus, check};
    use serde_json::{Value, json};
    use tiber_tasks_core::{Subtask, TaskEvent, TaskId};
    use tiber_tasks_service::command::{
        CheckSubtaskOccurrence, CompleteTask, SubtaskOccurrence, decide_check_subtask_occurrence,
        decide_complete_task,
    };
    use tiber_tasks_service::{TaskBoardProjection, TaskHistory};

    const TASK_ID: &str = "20260810-hwcc-prove-tiber-can-run-an-isolated-codex-conversation";

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the modeled command fixture fails with checker diagnostics when exact occurrence or completion provenance is incomplete"
    )]
    fn occurrence_check_and_completion_have_complete_model_provenance() {
        let report = check().expect("complete occurrence-check and completion model provenance");
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
        reason = "the focused occurrence-check fixture returns canonical task history directly"
    )]
    fn active_subtask_history() -> TaskHistory {
        TaskHistory::from_ordered_events(vec![event(json!({
            "event": "task_created",
            "stream_id": format!("tiber:task:{TASK_ID}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                "stem": TASK_ID,
                "subtasks": [
                    {"after": [], "checked": false, "id": "s4", "title": "protect review orchestration"}
                ],
                "summary": "", "tags": [], "title": "HWCC"
            }
        }))])
    }

    #[expect(
        clippy::implicit_return,
        reason = "the focused replay fixture appends one exact durable occurrence fact to its active base history"
    )]
    fn unchecked_subtask_history() -> TaskHistory {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_subtask_occurrence_checked", "stream_id": "tiber:board",
            "stem": TASK_ID, "index": usize::default(),
            "expected": {
                "after": [], "checked": false, "id": "s4", "title": "protect review orchestration"
            }
        })));
        TaskHistory::from_ordered_events(events)
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the exact current occurrence fixture uses a fail-fast durable-wire conversion"
    )]
    fn current_subtask() -> Subtask {
        serde_json::from_value(json!({
            "after": [], "checked": false, "id": "s4", "title": "protect review orchestration"
        }))
        .expect("fixture subtask is valid")
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the terminal retry fixture changes only the check state produced by the retained exact occurrence fact"
    )]
    fn checked_subtask() -> Subtask {
        let mut subtask = current_subtask();
        subtask.checked = true;
        subtask
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the public command fixture uses valid semantic boundary values"
    )]
    fn occurrence_check_request() -> CheckSubtaskOccurrence {
        CheckSubtaskOccurrence::new(
            TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
            SubtaskOccurrence::parse_one_based("1").expect("first occurrence is valid"),
            current_subtask(),
        )
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the public completion fixture uses a valid semantic task identity"
    )]
    fn completion_request() -> CompleteTask {
        CompleteTask::new(TaskId::parse(TASK_ID).expect("fixture task ID is valid"))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the completion fixture retains one exact occurrence check and one current board entry before terminal publication"
    )]
    fn completeable_task_history() -> TaskHistory {
        let mut events = unchecked_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": [TASK_ID]
        })));
        TaskHistory::from_ordered_events(events)
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "this public replay scenario proves an occurrence fact changes only its exact indexed subtask"
    )]
    fn replays_an_exact_subtask_occurrence_check() {
        let projection = TaskBoardProjection::replay(&unchecked_subtask_history())
            .expect("the exact durable occurrence fact replays");
        let task = projection
            .task(&TaskId::parse(TASK_ID).expect("fixture task ID is valid"))
            .expect("fixture task is projected");

        assert!(task.subtasks.first().is_some_and(|subtask| subtask.checked));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public command scenario uses a fail-fast typed fixture and closed-publication assertions"
    )]
    fn decides_one_exact_occurrence_check_on_the_board_and_task_fence() {
        let request = occurrence_check_request();
        let decided = decide_check_subtask_occurrence(active_subtask_history().events(), &request)
            .expect("the active unchecked occurrence is addressable")
            .expect("an unchecked exact occurrence requires one closed publication");

        assert_eq!(
            serde_json::to_value(TaskEvent::TaskSubtaskOccurrenceChecked(
                decided.checked_fact().clone(),
            ))
            .expect("fact serializes"),
            json!({
                "event": "task_subtask_occurrence_checked", "stream_id": "tiber:board",
                "stem": TASK_ID, "index": usize::default(),
                "expected": {
                    "after": [], "checked": false, "id": "s4", "title": "protect review orchestration"
                }
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
        reason = "the exact-idempotency scenario uses the direct durable occurrence fact fixture"
    )]
    fn returns_none_when_the_exact_occurrence_check_is_already_in_current_lifetime_history() {
        assert_eq!(
            decide_check_subtask_occurrence(
                unchecked_subtask_history().events(),
                &occurrence_check_request(),
            )
            .expect("the exact retained occurrence check remains valid"),
            None
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the terminal exact-idempotency scenario supplies the checked projection postimage reconstructed by the public CLI"
    )]
    fn returns_none_when_the_exact_checked_postimage_remains_current_after_completion() {
        let mut events = unchecked_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));
        let request = CheckSubtaskOccurrence::new(
            TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
            SubtaskOccurrence::zero_based(0),
            checked_subtask(),
        );

        assert_eq!(
            decide_check_subtask_occurrence(&events, &request)
                .expect("the exact checked postimage remains idempotent after completion"),
            None
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the ordered replay scenario uses a direct legacy uncheck after the exact fact and asserts that current unchecked state requires a new occurrence publication"
    )]
    fn republishes_an_exact_occurrence_check_after_a_later_legacy_uncheck() {
        let mut events = unchecked_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_subtask_checked", "stream_id": "tiber:board", "stem": TASK_ID,
            "subtask_id": "s4", "checked": false
        })));

        assert!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect("the later legacy uncheck restores the exact current preimage")
                .is_some(),
            "a historical exact check must not mask a later current unchecked state"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the stale-preimage scenario uses a direct changed immutable subtask fixture and stable error-code assertion"
    )]
    fn rejects_an_occurrence_check_with_a_stale_complete_preimage() {
        let stale: Subtask = serde_json::from_value(json!({
            "after": [], "checked": false, "id": "s4", "title": "stale review title"
        }))
        .expect("fixture subtask is valid");
        let request = CheckSubtaskOccurrence::new(
            TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
            SubtaskOccurrence::zero_based(0),
            stale,
        );

        assert_eq!(
            decide_check_subtask_occurrence(active_subtask_history().events(), &request)
                .expect_err("a changed complete preimage must not select by occurrence alone")
                .code(),
            "tasks_command_subtask_occurrence_check_preimage_mismatch"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the terminal-lifecycle scenario uses direct durable task history and a stable occurrence-check failure assertion"
    )]
    fn rejects_an_occurrence_check_after_the_target_task_leaves_in_progress() {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));

        assert_eq!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect_err("a terminal task cannot receive a new occurrence check")
                .code(),
            "tasks_command_task_not_in_progress"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the board-authoritative trailer-closure scenario uses a direct durable fact and proves it declines a new occurrence publication"
    )]
    fn refuses_an_occurrence_check_after_a_board_stream_trailer_closure() {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "tasks_closed_from_commit_trailers", "stream_id": "tiber:board",
            "stems": [TASK_ID], "order": []
        })));

        assert_eq!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect_err(
                    "a board-stream trailer closure ends the target lifecycle and cannot produce an occurrence publication"
                )
                .code(),
            "tasks_command_task_not_in_progress"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the board-authoritative trailer-fence scenario uses a direct foreign durable fact and stable command-fence assertion"
    )]
    fn rejects_a_target_trailer_closure_from_a_task_stream() {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "tasks_closed_from_commit_trailers", "stream_id": format!("tiber:task:{TASK_ID}"),
            "stems": [TASK_ID], "order": []
        })));

        assert_eq!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect_err(
                    "a trailer closure is board-authoritative even when its target list names this task"
                )
                .code(),
            "tasks_command_target_task_fact_unexpected_stream"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the foreign-stream scenario uses a direct durable occurrence fact and stable command-fence assertion"
    )]
    fn rejects_target_occurrence_history_from_a_foreign_task_stream() {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_subtask_occurrence_checked", "stream_id": "tiber:task:another-task",
            "stem": TASK_ID, "index": usize::default(),
            "expected": {
                "after": [], "checked": false, "id": "s4", "title": "protect review orchestration"
            }
        })));

        assert_eq!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect_err("a target occurrence fact in another task stream is not authoritative")
                .code(),
            "tasks_command_target_task_fact_unexpected_stream"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the removal-and-recreation scenario uses direct durable lifetime facts and an exact-publication assertion"
    )]
    fn permits_an_occurrence_check_again_after_task_removal_and_recreation() {
        let mut events = unchecked_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "task_removed", "stream_id": "tiber:board", "stem": TASK_ID
        })));
        events.push(event(json!({
            "event": "task_created",
            "stream_id": format!("tiber:task:{TASK_ID}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-14T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                "stem": TASK_ID,
                "subtasks": [
                    {"after": [], "checked": false, "id": "s4", "title": "protect review orchestration"}
                ],
                "summary": "", "tags": [], "title": "HWCC recreated"
            }
        })));

        assert!(
            decide_check_subtask_occurrence(&events, &occurrence_check_request())
                .expect("a replacement task lifetime resets old exact occurrence idempotency")
                .is_some()
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public completion scenario uses fail-fast typed fixtures and asserts the closed atomic terminal/order batch"
    )]
    fn completes_an_active_task_with_all_requirements_checked_and_removes_its_board_entry() {
        let decided =
            decide_complete_task(completeable_task_history().events(), &completion_request())
                .expect("every current completion requirement is checked")
                .expect("an active task needs the closed terminal/order publication");
        let (events, consistency_streams) = decided.into_events_and_consistency_streams();

        assert_eq!(
            serde_json::to_value(events).expect("closed batch serializes"),
            json!([
                {
                    "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
                    "status": "done", "claim": null
                },
                {"event": "board_reordered", "stream_id": "tiber:board", "order": []}
            ])
        );
        assert_eq!(
            consistency_streams
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>(),
            vec!["tiber:board".to_owned(), format!("tiber:task:{TASK_ID}")]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the stale-board scenario uses direct durable lifecycle/order facts and asserts the closed order-only repair"
    )]
    fn repairs_a_done_tasks_stale_board_entry_without_reemitting_its_transition() {
        let mut events = completeable_task_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));
        let decided = decide_complete_task(&events, &completion_request())
            .expect("a done task with a stale entry is repairable")
            .expect("the stale board entry requires one closed order repair");
        let (published_events, _consistency_streams) =
            decided.into_events_and_consistency_streams();

        assert_eq!(
            serde_json::to_value(published_events).expect("closed repair batch serializes"),
            json!([
                {"event": "board_reordered", "stream_id": "tiber:board", "order": []}
            ])
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the duplicate stale-entry scenario uses direct durable board facts and proves an order-only repair preserves every non-target entry's relative order"
    )]
    fn repairs_all_duplicate_stale_target_entries_without_reordering_other_tasks() {
        let first = "20260811-a111-first-open";
        let second = "20260812-b222-second-open";
        let mut events = completeable_task_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board",
            "order": [first, TASK_ID, second, TASK_ID]
        })));
        let decided = decide_complete_task(&events, &completion_request())
            .expect("a done task with duplicate stale entries is repairable")
            .expect("stale duplicate target entries require one closed order-only repair");
        assert!(
            decided.transitioned_fact().is_none(),
            "a stale-order repair must not re-emit the terminal transition"
        );
        let (published_events, _consistency_streams) =
            decided.into_events_and_consistency_streams();

        assert_eq!(
            serde_json::to_value(published_events).expect("closed order-only repair serializes"),
            json!([
                {
                    "event": "board_reordered", "stream_id": "tiber:board",
                    "order": [first, second]
                }
            ])
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the complete idempotency scenario uses direct durable completion facts and a no-publication assertion"
    )]
    fn returns_none_when_a_done_task_is_already_absent_from_the_board_order() {
        let mut events = completeable_task_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "done", "claim": null
        })));
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": []
        })));

        assert_eq!(
            decide_complete_task(&events, &completion_request())
                .expect("the completed exact state is valid"),
            None
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the unchecked-subtask scenario uses a direct active task fixture and stable completion error assertion"
    )]
    fn refuses_completion_while_a_current_subtask_is_unchecked() {
        let mut events = active_subtask_history().events().to_vec();
        events.push(event(json!({
            "event": "board_reordered", "stream_id": "tiber:board", "order": [TASK_ID]
        })));

        assert_eq!(
            decide_complete_task(&events, &completion_request())
                .expect_err("an unchecked current subtask blocks completion")
                .code(),
            "tasks_command_subtask_occurrence_unchecked"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the unchecked-acceptance scenario uses direct durable facts and stable completion error assertion"
    )]
    fn refuses_completion_while_a_current_acceptance_item_is_unchecked() {
        let mut events = completeable_task_history().events().to_vec();
        events.insert(
            1,
            event(json!({
                "event": "task_acceptance_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "item": {"checked": false, "text": "verify completion boundary"}
            })),
        );

        assert_eq!(
            decide_complete_task(&events, &completion_request())
                .expect_err("an unchecked current acceptance item blocks completion")
                .code(),
            "tasks_command_acceptance_item_unchecked"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the terminal-status scenario uses direct durable lifecycle facts and a stable completion error assertion"
    )]
    fn refuses_completion_for_a_non_active_non_done_task() {
        let mut events = completeable_task_history().events().to_vec();
        events.push(event(json!({
            "event": "task_transitioned", "stream_id": "tiber:board", "stem": TASK_ID,
            "status": "abandoned", "claim": null
        })));

        assert_eq!(
            decide_complete_task(&events, &completion_request())
                .expect_err("an abandoned task is not eligible for completion")
                .code(),
            "tasks_command_task_not_in_progress"
        );
    }
}
