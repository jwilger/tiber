#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use eventcore::model::{CheckStatus, check};
    use serde_json::{Value, json};
    use tiber_tasks_core::{Subtask, TaskEvent, TaskId};
    use tiber_tasks_service::command::{
        RepairDuplicateSubtaskId, SubtaskOccurrence, SubtaskReplacementId,
        decide_repair_duplicate_subtask_id,
    };

    const TASK_ID: &str = "20260810-hwcc-prove-tiber-can-run-an-isolated-codex-conversation";
    const FIRST_OCCURRENCE: usize = 0;
    const THIRD_OCCURRENCE: usize = 2;

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the modeled command fixture fails with checker diagnostics when provenance is incomplete"
    )]
    fn correction_has_complete_model_provenance() {
        let report = check().expect("complete duplicate-subtask correction model provenance");
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
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the complete preimage fixture fails fast and returns the exact retained subtask directly"
    )]
    fn second_duplicate_preimage() -> Subtask {
        serde_json::from_value(json!({
            "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
        }))
        .expect("fixture subtask is valid")
    }

    #[expect(
        clippy::implicit_return,
        reason = "the mixed-stream fixture returns its canonical task history directly"
    )]
    fn duplicate_subtask_history() -> Vec<TaskEvent> {
        vec![
            event(json!({
                "event": "task_created",
                "stream_id": format!("tiber:task:{TASK_ID}"),
                "task": {
                    "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                    "committed_at": "2026-08-12T00:00:00Z", "context": "", "notes": [],
                    "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                    "stem": TASK_ID,
                    "subtasks": [
                        {"after": [], "checked": true, "id": "s1", "title": "already complete"},
                        {"after": ["s3"], "checked": false, "id": "s4", "title": "protect native review orchestration"}
                    ],
                    "summary": "", "tags": [], "title": "HWCC"
                }
            })),
            event(json!({
                "event": "task_subtask_added", "stream_id": "tiber:board", "stem": TASK_ID,
                "subtask": {
                    "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
                }
            })),
            event(json!({
                "event": "task_subtask_checked", "stream_id": "tiber:board", "stem": TASK_ID,
                "subtask_id": "s4", "checked": true
            })),
        ]
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the public command fixture uses valid semantic boundary values"
    )]
    fn correction_request(replacement_id: &str) -> RepairDuplicateSubtaskId {
        RepairDuplicateSubtaskId::new(
            TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
            SubtaskOccurrence::parse_one_based("3").expect("third occurrence is valid"),
            second_duplicate_preimage(),
            SubtaskReplacementId::parse(replacement_id).expect("fixture replacement ID is valid"),
        )
    }

    #[expect(
        clippy::implicit_return,
        reason = "the checked-preimage fixture changes only the exact current occurrence state required after the durable occurrence-check fact"
    )]
    fn checked_second_duplicate_preimage() -> Subtask {
        let mut checked = second_duplicate_preimage();
        checked.checked = true;
        checked
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this pure public command scenario uses fail-fast typed fixture and decision assertions"
    )]
    fn corrects_the_exact_duplicate_occurrence_with_a_full_preimage_and_two_stream_fence() {
        let request = correction_request("s5");
        let decided = decide_repair_duplicate_subtask_id(&duplicate_subtask_history(), &request)
            .expect("the duplicate occurrence is correctable")
            .expect("a missing exact correction requires one closed publication");

        assert_eq!(
            serde_json::to_value(TaskEvent::TaskSubtaskIdCorrected(
                decided.corrected_fact().clone(),
            ))
            .expect("fact serializes"),
            json!({
                "event": "task_subtask_id_corrected", "stream_id": "tiber:board", "stem": TASK_ID,
                "index": THIRD_OCCURRENCE,
                "expected": {
                    "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
                },
                "replacement_id": "s5"
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
        reason = "the idempotency scenario uses a direct exact-publication fixture"
    )]
    fn returns_none_when_the_exact_correction_is_already_in_canonical_history() {
        let request = correction_request("s5");
        let mut history = duplicate_subtask_history();
        history.push(event(json!({
            "event": "task_subtask_id_corrected", "stream_id": "tiber:board", "stem": TASK_ID,
            "index": THIRD_OCCURRENCE,
            "expected": {
                "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
            },
            "replacement_id": "s5"
        })));

        assert_eq!(
            decide_repair_duplicate_subtask_id(&history, &request)
                .expect("the exact retained correction remains valid"),
            None
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the ordered regression uses the prior exact occurrence fact and asserts that a subsequent correction carries its current checked preimage"
    )]
    fn repairs_a_duplicate_occurrence_after_an_exact_occurrence_check() {
        let mut history = duplicate_subtask_history();
        history.push(event(json!({
            "event": "task_subtask_occurrence_checked", "stream_id": "tiber:board",
            "stem": TASK_ID, "index": THIRD_OCCURRENCE,
            "expected": {
                "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
            }
        })));
        let request = RepairDuplicateSubtaskId::new(
            TaskId::parse(TASK_ID).expect("fixture task ID is valid"),
            SubtaskOccurrence::zero_based(THIRD_OCCURRENCE),
            checked_second_duplicate_preimage(),
            SubtaskReplacementId::parse("s5").expect("fixture replacement ID is valid"),
        );
        let decided = decide_repair_duplicate_subtask_id(&history, &request)
            .expect("the preceding exact occurrence check remains replayable by repair")
            .expect("the checked duplicate occurrence still needs its unique identity correction");

        assert_eq!(
            decided.corrected_fact().expected,
            checked_second_duplicate_preimage()
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the removal-and-recreation scenario uses direct durable facts and a closed correction assertion"
    )]
    fn clears_a_prior_exact_correction_when_the_task_is_removed_before_recreation() {
        let request = correction_request("s5");
        let mut history = duplicate_subtask_history();
        history.push(event(json!({
            "event": "task_subtask_id_corrected", "stream_id": "tiber:board", "stem": TASK_ID,
            "index": THIRD_OCCURRENCE,
            "expected": {
                "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
            },
            "replacement_id": "s5"
        })));
        history.push(event(json!({
            "event": "task_removed", "stream_id": "tiber:board", "stem": TASK_ID
        })));
        history.push(event(json!({
            "event": "task_created",
            "stream_id": format!("tiber:task:{TASK_ID}"),
            "task": {
                "acceptance": [], "blocked_by": [], "blocks": [], "claim": null,
                "committed_at": "2026-08-13T00:00:00Z", "context": "", "notes": [],
                "pr_mr_status": null, "pr_mr_url": null, "status": "in-progress",
                "stem": TASK_ID,
                "subtasks": [
                    {"after": [], "checked": true, "id": "s1", "title": "already complete"},
                    {"after": ["s3"], "checked": true, "id": "s4", "title": "protect native review orchestration"}
                ],
                "summary": "", "tags": [], "title": "HWCC recreated"
            }
        })));
        history.push(event(json!({
            "event": "task_subtask_added", "stream_id": "tiber:board", "stem": TASK_ID,
            "subtask": {
                "after": ["s4"], "checked": false, "id": "s4", "title": "disable retained legacy eval paths"
            }
        })));

        assert!(
            decide_repair_duplicate_subtask_id(&history, &request)
                .expect("a removal resets the old correction and permits a recreated task")
                .is_some()
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the malformed retained-correction scenario asserts the stable history error directly"
    )]
    fn rejects_a_retained_correction_of_an_identifier_that_was_not_duplicate() {
        let request = correction_request("s5");
        let mut history = duplicate_subtask_history();
        history.push(event(json!({
            "event": "task_subtask_id_corrected", "stream_id": "tiber:board", "stem": TASK_ID,
            "index": FIRST_OCCURRENCE,
            "expected": {
                "after": [], "checked": true, "id": "s1", "title": "already complete"
            },
            "replacement_id": "s2"
        })));

        assert_eq!(
            decide_repair_duplicate_subtask_id(&history, &request)
                .expect_err(
                    "a retained correction may repair only an actually duplicated identifier"
                )
                .code(),
            "tasks_command_history_subtask_id_not_duplicate"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the stale-preimage scenario uses a direct stable-code assertion"
    )]
    fn rejects_a_stale_full_preimage() {
        let stale: Subtask = serde_json::from_value(json!({
            "after": ["s4"], "checked": false, "id": "s4", "title": "stale title"
        }))
        .expect("fixture subtask is valid");
        let task = TaskId::parse(TASK_ID).expect("fixture task ID is valid");
        let request = RepairDuplicateSubtaskId::new(
            task,
            SubtaskOccurrence::zero_based(2),
            stale,
            SubtaskReplacementId::parse("s5").expect("fixture replacement is valid"),
        );

        assert_eq!(
            decide_repair_duplicate_subtask_id(&duplicate_subtask_history(), &request)
                .expect_err("a stale full preimage must not select by identifier alone")
                .code(),
            "tasks_command_subtask_correction_preimage_mismatch"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the occupied-replacement scenario uses a direct stable-code assertion"
    )]
    fn rejects_a_replacement_identifier_already_used_by_another_occurrence() {
        let request = correction_request("s1");

        assert_eq!(
            decide_repair_duplicate_subtask_id(&duplicate_subtask_history(), &request)
                .expect_err("the replacement must be unique in the current task")
                .code(),
            "tasks_command_subtask_replacement_id_already_exists"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the malformed-stream scenario uses a direct stable-code assertion"
    )]
    fn rejects_target_subtask_history_from_a_foreign_task_stream() {
        let request = correction_request("s5");
        let mut history = duplicate_subtask_history();
        history.push(event(json!({
            "event": "task_subtask_added", "stream_id": "tiber:task:another-task", "stem": TASK_ID,
            "subtask": {"after": [], "checked": false, "id": "untrusted", "title": "foreign"}
        })));

        assert_eq!(
            decide_repair_duplicate_subtask_id(&history, &request)
                .expect_err("a target task fact in another task stream is not authoritative")
                .code(),
            "tasks_command_target_task_fact_unexpected_stream"
        );
    }
}
