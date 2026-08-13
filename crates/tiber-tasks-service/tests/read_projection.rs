#[cfg(test)]
mod tests {
    use core::mem::size_of;

    use serde_json::{Value, json};
    use tiber_tasks_core::{TaskEvent, TaskId, TaskStatus};
    use tiber_tasks_service::{TaskBoardProjection, TaskHistory, TaskProjectionError};

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixture decoder fails fast with a descriptive message and returns its parsed event directly"
    )]
    fn event(value: Value) -> TaskEvent {
        serde_json::from_value(value).expect("fixture event matches the retained task wire format")
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "this one-purpose fixture builder keeps the retained task wire shape readable beside the event builder"
    )]
    fn task(id: &str, title: &str, status: &str) -> Value {
        json!({
            "acceptance": [],
            "blocked_by": [],
            "blocks": [],
            "claim": null,
            "committed_at": "2026-08-12T00:00:00Z",
            "context": "",
            "notes": [],
            "pr_mr_status": null,
            "pr_mr_url": null,
            "status": status,
            "stem": id,
            "subtasks": [],
            "summary": "",
            "tags": [],
            "title": title
        })
    }

    #[expect(
        clippy::implicit_return,
        reason = "the compact fixture event builder returns its decoded creation fact directly"
    )]
    fn created(id: &str, title: &str, status: &str) -> TaskEvent {
        event(json!({
            "event": "task_created",
            "stream_id": format!("tiber:task:{id}"),
            "task": task(id, title, status)
        }))
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "this one-use fixture names the repository initialization fact used in the full replay scenario"
    )]
    fn repository_initialized() -> TaskEvent {
        event(json!({
            "event": "repository_initialized",
            "stream_id": "tiber:repository"
        }))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the compact fixture event builder returns its decoded board-order fact directly"
    )]
    fn board_order(order: &[&str]) -> TaskEvent {
        event(json!({
            "event": "board_reordered",
            "stream_id": "tiber:board",
            "order": order
        }))
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "this public-boundary replay scenario uses fail-fast fixture assertions and an iterator projection assertion"
    )]
    fn replays_mixed_board_and_dynamic_task_stream_history() {
        let foundation = "20260810-a111-foundation";
        let dependent = "20260810-b222-dependent-service";
        let history = TaskHistory::from_ordered_events(vec![
            repository_initialized(),
            created(foundation, "Lay the foundation", "backlog"),
            created(dependent, "Read-only task service", "backlog"),
            event(json!({
                "event": "task_links_changed",
                "stream_id": format!("tiber:task:{dependent}"),
                "stem": dependent,
                "blocks": [],
                "blocked_by": [foundation]
            })),
            board_order(&[dependent, foundation]),
            event(json!({
                "event": "task_details_updated",
                "stream_id": format!("tiber:task:{foundation}"),
                "stem": foundation,
                "title": "Foundation is delivered",
                "tags": ["native"],
                "summary": "A native harness foundation.",
                "context": "The task store can now be ported safely."
            })),
            event(json!({
                "event": "task_transitioned",
                "stream_id": format!("tiber:task:{foundation}"),
                "stem": foundation,
                "status": "done",
                "claim": null
            })),
        ]);

        let projection = TaskBoardProjection::replay(&history).expect("history replays");

        assert!(projection.is_initialized());
        assert_eq!(projection.tasks().count(), 2);
        assert_eq!(
            projection
                .ordered_tasks()
                .into_iter()
                .map(|task| task.stem.as_str())
                .collect::<Vec<_>>(),
            vec![dependent]
        );
        assert_eq!(
            projection
                .task(&TaskId::parse(foundation).expect("fixture task id is valid"))
                .expect("foundation is projected")
                .title
                .as_str(),
            "Foundation is delivered"
        );
        assert_eq!(
            projection
                .next_eligible_task()
                .expect("completed prerequisite makes dependent eligible")
                .stem
                .as_str(),
            dependent
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "this public-boundary terminal-state scenario uses a fail-fast replay assertion and iterator projection assertion"
    )]
    fn ordered_tasks_omits_terminal_tasks_transitioned_after_the_latest_board_order() {
        let done = "20260810-a111-done-after-order";
        let abandoned = "20260810-b222-abandoned-after-order";
        let open = "20260810-c333-open-after-order";
        let projection = TaskBoardProjection::replay(&TaskHistory::from_ordered_events(vec![
            created(done, "Done after board order", "backlog"),
            created(abandoned, "Abandoned after board order", "backlog"),
            created(open, "Still open", "backlog"),
            board_order(&[done, abandoned, open]),
            event(json!({
                "event": "task_transitioned",
                "stream_id": format!("tiber:task:{done}"),
                "stem": done,
                "status": "done",
                "claim": null
            })),
            event(json!({
                "event": "task_transitioned",
                "stream_id": format!("tiber:task:{abandoned}"),
                "stem": abandoned,
                "status": "abandoned",
                "claim": null
            })),
        ]))
        .expect("history replays");

        assert_eq!(
            projection
                .ordered_tasks()
                .into_iter()
                .map(|task| task.stem.as_str())
                .collect::<Vec<_>>(),
            vec![open],
            "the default board view is open-only even when terminal transitions follow its latest order fact"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the malformed durable correction scenario uses a direct public replay error assertion"
    )]
    fn rejects_a_correction_that_would_rename_a_unique_subtask_identity() {
        let id = "20260810-a111-duplicate-correction-fence";
        let history = TaskHistory::from_ordered_events(vec![
            created(id, "Duplicate correction fence", "backlog"),
            event(json!({
                "event": "task_subtask_added", "stream_id": "tiber:board", "stem": id,
                "subtask": {"after": [], "checked": false, "id": "s1", "title": "first"}
            })),
            event(json!({
                "event": "task_subtask_added", "stream_id": "tiber:board", "stem": id,
                "subtask": {"after": [], "checked": false, "id": "s2", "title": "unique"}
            })),
            event(json!({
                "event": "task_subtask_id_corrected", "stream_id": "tiber:board", "stem": id,
                "index": size_of::<u8>(),
                "expected": {"after": [], "checked": false, "id": "s2", "title": "unique"},
                "replacement_id": "s3"
            })),
        ]);

        assert_eq!(
            TaskBoardProjection::replay(&history)
                .expect_err("a correction is valid only for a currently duplicated identity")
                .code(),
            "tasks_projection_subtask_correction_id_not_duplicate"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary reference-resolution scenario uses descriptive fail-fast fixture assertions"
    )]
    fn resolves_full_short_and_nickname_references_without_guessing_ambiguous_ones() {
        let primary = "20260810-hwcc-native-task-service";
        let projection = TaskBoardProjection::replay(&TaskHistory::from_ordered_events(vec![
            created(primary, "Native task service", "backlog"),
            created(
                "20260811-a111-shared-nickname",
                "First shared nickname",
                "backlog",
            ),
            created(
                "20260812-b222-shared-nickname",
                "Second shared nickname",
                "backlog",
            ),
        ]))
        .expect("history replays");

        assert_eq!(
            projection
                .resolve_task_ref(primary)
                .expect("full task reference resolves")
                .as_str(),
            primary
        );
        assert_eq!(
            projection
                .resolve_task_ref("20260810-hwcc")
                .expect("short task reference resolves")
                .as_str(),
            primary
        );
        assert_eq!(
            projection
                .resolve_task_ref("native-task-service")
                .expect("nickname reference resolves")
                .as_str(),
            primary
        );
        assert_eq!(
            projection
                .resolve_task_ref("shared-nickname")
                .expect_err("ambiguous nickname is rejected")
                .code(),
            "tasks_task_reference_ambiguous"
        );
        assert_eq!(
            projection
                .resolve_task_ref("../outside")
                .expect_err("path-like reference is rejected at the boundary")
                .code(),
            "tasks_invalid_task_reference"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary task-selection scenario uses descriptive fail-fast fixture assertions"
    )]
    fn selects_the_first_unblocked_backlog_task_in_strict_priority_order() {
        let prerequisite = "20260810-a111-prerequisite";
        let blocked = "20260810-b222-blocked";
        let eligible = "20260810-c333-eligible";
        let history = TaskHistory::from_ordered_events(vec![
            created(prerequisite, "Prerequisite", "backlog"),
            created(blocked, "Blocked", "backlog"),
            created(eligible, "Eligible", "backlog"),
            event(json!({
                "event": "task_links_changed",
                "stream_id": format!("tiber:task:{blocked}"),
                "stem": blocked,
                "blocks": [],
                "blocked_by": [prerequisite]
            })),
            board_order(&[blocked, eligible]),
        ]);
        let mut projection = TaskBoardProjection::replay(&history).expect("history replays");

        assert_eq!(
            projection
                .next_eligible_task()
                .expect("later unblocked task is eligible")
                .stem
                .as_str(),
            eligible
        );
        assert_eq!(
            projection
                .next_actionable_task()
                .expect("no active task permits the eligible backlog selection")
                .expect("later unblocked task is actionable")
                .stem
                .as_str(),
            eligible
        );

        projection
            .apply(&event(json!({
                "event": "task_transitioned",
                "stream_id": format!("tiber:task:{eligible}"),
                "stem": eligible,
                "status": "in-progress",
                "claim": null
            })))
            .expect("transition folds");
        assert!(projection.next_eligible_task().is_none());
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary active-task precedence scenario uses descriptive fail-fast fixture assertions"
    )]
    fn next_actionable_task_returns_the_sole_active_task_before_an_eligible_backlog_task() {
        let active = "20260810-a111-active";
        let eligible = "20260810-b222-eligible";
        let projection = TaskBoardProjection::replay(&TaskHistory::from_ordered_events(vec![
            created(active, "Continue active work", "in-progress"),
            created(eligible, "Eligible after active work", "backlog"),
            board_order(&[eligible, active]),
        ]))
        .expect("history replays");

        assert_eq!(
            projection
                .next_actionable_task()
                .expect("one active task is a valid board")
                .expect("the active task is actionable")
                .stem
                .as_str(),
            active
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary corrupt-history scenario uses descriptive fail-fast fixture assertions"
    )]
    fn next_actionable_task_rejects_multiple_active_tasks_with_stable_diagnostics() {
        let first_active = "20260810-a111-first-active";
        let second_active = "20260810-b222-second-active";
        let projection = TaskBoardProjection::replay(&TaskHistory::from_ordered_events(vec![
            created(second_active, "Second active task", "in-progress"),
            created(first_active, "First active task", "in-progress"),
        ]))
        .expect("history replays");

        let error = projection
            .next_actionable_task()
            .expect_err("multiple active tasks require explicit repair");

        assert_eq!(error.code(), "tasks_projection_multiple_active_tasks");
        assert_eq!(
            error,
            TaskProjectionError::MultipleActiveTasks {
                active_tasks: vec![
                    TaskId::parse(first_active).expect("fixture task id is valid"),
                    TaskId::parse(second_active).expect("fixture task id is valid"),
                ],
            }
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::too_many_lines,
        reason = "this one public-boundary scenario demonstrates a complete preserved historical-fact sequence with fail-fast fixture assertions and compact predicate checks"
    )]
    fn projects_legacy_facts_and_validation_repairs_without_treating_them_as_commands() {
        let current = "20260810-a111-current";
        let blocker = "20260810-b222-blocker";
        let mut projection = TaskBoardProjection::default();
        let events = vec![
            created(current, "Current task", "backlog"),
            created(blocker, "Blocker", "done"),
            event(json!({
                "event": "task_claim_changed",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "claim": {"host": "owner-host", "session": "session-1"}
            })),
            event(json!({
                "event": "task_pull_request_changed",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "url": "https://example.test/pull/1",
                "status": "open"
            })),
            event(json!({
                "event": "task_acceptance_added",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "item": {"checked": false, "text": "Project legacy facts"}
            })),
            event(json!({
                "event": "task_acceptance_checked",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "index": usize::MIN,
                "checked": true
            })),
            event(json!({
                "event": "task_subtask_added",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "subtask": {
                    "id": "projection",
                    "checked": false,
                    "title": "Replay existing facts",
                    "after": []
                }
            })),
            event(json!({
                "event": "task_subtask_checked",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "subtask_id": "projection",
                "checked": true
            })),
            event(json!({
                "event": "task_note_added",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "note": {"date": "2026-08-12", "text": "Historical event replayed."}
            })),
            event(json!({
                "event": "task_validation_repaired",
                "stream_id": "tiber:board",
                "link_changes": [{
                    "stream_id": format!("tiber:task:{current}"),
                    "stem": current,
                    "blocks": [],
                    "blocked_by": [blocker]
                }],
                "order_change": {"stream_id": "tiber:board", "order": [current]},
                "repairs": [{"repair": "board_entry_added", "task": current}]
            })),
            event(json!({"event": "task_state_published", "stream_id": "tiber:board"})),
        ];

        for event in events {
            projection.apply(&event).expect("legacy fact folds");
        }
        let current_id = TaskId::parse(current).expect("fixture task id is valid");
        let projected = projection.task(&current_id).expect("task remains present");
        assert_eq!(
            projected.claim.as_ref().map(|claim| claim.session.as_str()),
            Some("session-1")
        );
        assert_eq!(projected.pr_mr_status.as_deref(), Some("open"));
        assert!(
            projected
                .acceptance
                .first()
                .is_some_and(|item| item.checked)
        );
        assert!(projected.subtasks.first().is_some_and(|item| item.checked));
        assert_eq!(projected.notes.len(), 1);
        assert_eq!(
            projection
                .next_eligible_task()
                .expect("done blocker makes repaired task eligible")
                .stem
                .as_str(),
            current
        );

        projection
            .apply(&event(json!({
                "event": "task_acceptance_removed",
                "stream_id": format!("tiber:task:{current}"),
                "stem": current,
                "index": usize::MIN
            })))
            .expect("historical acceptance removal folds");
        projection
            .apply(&event(json!({
                "event": "task_closed_from_trailer",
                "stream_id": "tiber:board",
                "stem": current
            })))
            .expect("historical closure folds");
        assert_eq!(
            projection
                .task(&current_id)
                .expect("closed task remains queryable")
                .status,
            TaskStatus::Done
        );
        projection
            .apply(&event(json!({
                "event": "task_removed",
                "stream_id": "tiber:board",
                "stem": current
            })))
            .expect("historical removal folds");
        assert!(projection.task(&current_id).is_none());
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary malformed-history scenario uses a descriptive fail-fast assertion"
    )]
    fn rejects_a_task_mutation_that_has_no_prior_creation_fact() {
        let unknown = "20260810-a111-unknown";
        let history = TaskHistory::from_ordered_events(vec![event(json!({
            "event": "task_transitioned",
            "stream_id": format!("tiber:task:{unknown}"),
            "stem": unknown,
            "status": "done",
            "claim": null
        }))]);

        assert_eq!(
            TaskBoardProjection::replay(&history)
                .expect_err("invalid historical sequence is rejected")
                .code(),
            "tasks_projection_task_missing"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "this public-boundary retained-history scenario uses a descriptive fail-fast replay assertion"
    )]
    fn ignores_the_historical_task_state_published_notification() {
        let history = TaskHistory::from_ordered_events(vec![event(json!({
            "event": "task_state_published",
            "stream_id": "tiber:board"
        }))]);

        let projection = TaskBoardProjection::replay(&history)
            .expect("historical publication notification is intentionally a no-op");

        assert!(!projection.is_initialized());
        assert_eq!(projection.tasks().count(), 0);
    }
}
