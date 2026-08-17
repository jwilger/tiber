#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::default_numeric_fallback,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::single_call_fn,
    clippy::std_instead_of_core,
    clippy::string_slice,
    clippy::too_many_lines,
    clippy::used_underscore_binding,
    reason = "the black-box CLI fixture fails loudly and inspects exact bounded output values"
)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

    use chrono::{DateTime, Utc};
    use eventcore_fs::FileEventStore;
    use eventcore_types::{EventStore as _, StreamId, StreamVersion, StreamWrites};
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tiber_tasks_core::TaskEvent;

    const TASK_ID: &str = "20260812-a111-paged-task-history";
    const FIRST_PRIORITY_TASK_ID: &str = "20260812-b222-first-priority";
    const PAGE_SPANNING_UPDATES: usize = 65;
    const SECOND_PRIORITY_TASK_ID: &str = "20260812-c333-first-priority";
    const SHELL_SENSITIVE_TASK_ID: &str = "20260812-e555-owner's-task";
    const CLOSED_TASK_ID: &str = "20260812-d444-closed-task";
    const BLOCKER_TASK_ID: &str = "20260812-e555-blocking-task";
    const BLOCKED_TASK_ID: &str = "20260812-f666-blocked-task";
    const UNRELATED_TARGET_BLOCKS: &str = "20260812-g777-target-blocks";
    const UNRELATED_TARGET_BLOCKED_BY: &str = "20260812-h888-target-blocked-by";
    const UNRELATED_BLOCKER_BLOCKS: &str = "20260812-i999-blocker-blocks";
    const UNRELATED_BLOCKER_BLOCKED_BY: &str = "20260812-j000-blocker-blocked-by";
    const TIBER_REF: &str = "refs/heads/tiber";

    struct TaskFixture {
        _directory: TempDir,
        repository: PathBuf,
    }

    /// A legacy workflow fact that intentionally shares the historical event
    /// type while remaining outside the native task stream boundary.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct LegacyCiRecoveryEvent {
        /// A workflow-specific identity that is not part of `TaskEvent`.
        recovery_id: String,
        /// The legacy workflow stream that owns this fact.
        stream_id: StreamId,
    }

    #[expect(
        clippy::implicit_return,
        reason = "the tiny test-only EventCore implementation reads clearly as direct trait-method expressions"
    )]
    impl eventcore_types::Event for LegacyCiRecoveryEvent {
        fn event_type_name() -> &'static str {
            TaskEvent::event_type_name()
        }

        fn stream_id(&self) -> &StreamId {
            &self.stream_id
        }
    }

    #[expect(
        clippy::arbitrary_source_item_ordering,
        reason = "the signed fixture helpers are arranged by shared base, distinct scenario, then public CLI behavior rather than alphabetically"
    )]
    impl TaskFixture {
        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the dedicated write fixture extends the signed paged base fixture while proving native signed idempotent publication"
        )]
        async fn signed_acceptance_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(task_acceptance_added(
                    &board_stream,
                    TASK_ID,
                    "first native acceptance",
                ))
                .expect("first acceptance fact should append")
                .append(task_acceptance_added(
                    &board_stream,
                    TASK_ID,
                    "second native acceptance",
                ))
                .expect("second acceptance fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture acceptance fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the active completion fixture needs one unchecked acceptance item to exercise the public recovery diagnostic"
        )]
        async fn signed_active_acceptance_history() -> Self {
            let fixture = Self::signed_active_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(task_acceptance_added(
                    &board_stream,
                    TASK_ID,
                    "first native acceptance",
                ))
                .expect("fixture acceptance fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture acceptance fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::implicit_return,
            reason = "the duplicate-identity fixture deliberately fails fast while constructing the signed authority state exercised by the public repair command"
        )]
        async fn signed_duplicate_subtask_history() -> Self {
            Self::signed_duplicate_subtask_history_with_status("backlog").await
        }

        #[expect(
            clippy::implicit_return,
            reason = "the active duplicate scenario names its distinct lifecycle state at its public completion-boundary use"
        )]
        async fn signed_active_duplicate_subtask_history() -> Self {
            Self::signed_duplicate_subtask_history_with_status("in-progress").await
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the completion-ready fixture deliberately fails fast while retaining historical unique-ID checks before the public occurrence-safe completion boundary"
        )]
        async fn signed_active_completion_ready_duplicate_subtask_history() -> Self {
            let fixture = Self::signed_active_duplicate_subtask_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(6))
                .expect("fixture board stream should register after five subtasks")
                .append(task_subtask_checked(&board_stream, TASK_ID, "s1"))
                .expect("first unique historical check should append")
                .append(task_subtask_checked(&board_stream, TASK_ID, "s2"))
                .expect("second unique historical check should append")
                .append(task_subtask_checked(&board_stream, TASK_ID, "s3"))
                .expect("third unique historical check should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture historical checks should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the parameterized duplicate-identity fixture deliberately fails fast while constructing the signed authority state exercised by distinct public task commands"
        )]
        async fn signed_duplicate_subtask_history_with_status(status: &str) -> Self {
            let fixture = Self::signed_paged_history_with_status(status).await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s1",
                    "Bootstrap",
                    &[],
                ))
                .expect("first fixture subtask should append")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s2",
                    "Authenticate",
                    &["s1"],
                ))
                .expect("second fixture subtask should append")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s3",
                    "Present",
                    &["s2"],
                ))
                .expect("third fixture subtask should append")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s4",
                    "Protect native review orchestration",
                    &["s3"],
                ))
                .expect("first duplicate fixture subtask should append")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s4",
                    "Disable legacy eval execution",
                    &["s4"],
                ))
                .expect("second duplicate fixture subtask should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture duplicate subtasks should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the recreated-task fixture deliberately retains a prior valid correction while making the current duplicate's complete preimage distinct"
        )]
        async fn signed_recreated_duplicate_subtask_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(
                    task_stream.clone(),
                    StreamVersion::new(PAGE_SPANNING_UPDATES + 1),
                )
                .expect("fixture task stream should register at its current version")
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s4",
                    "Original first duplicate",
                    &[],
                ))
                .expect("first original duplicate should append")
                .append(task_subtask_added(
                    &board_stream,
                    TASK_ID,
                    "s4",
                    "Original second duplicate",
                    &[],
                ))
                .expect("second original duplicate should append")
                .append(task_subtask_id_corrected(
                    &board_stream,
                    TASK_ID,
                    1,
                    "s4",
                    "Original second duplicate",
                    "s5",
                ))
                .expect("original correction should append")
                .append(task_removed(&board_stream, TASK_ID))
                .expect("historical task removal should append")
                .append(event(json!({
                    "event": "task_created",
                    "stream_id": task_stream.as_ref(),
                    "task": {
                        "acceptance": [],
                        "blocked_by": [],
                        "blocks": [],
                        "claim": null,
                        "committed_at": "2026-08-13T00:00:00Z",
                        "context": "Recreated task context.",
                        "notes": [],
                        "pr_mr_status": null,
                        "pr_mr_url": null,
                        "status": "backlog",
                        "stem": TASK_ID,
                        "subtasks": [
                            {"after": [], "checked": false, "id": "s4", "title": "Recreated first duplicate"},
                            {"after": [], "checked": false, "id": "s4", "title": "Recreated second duplicate"}
                        ],
                        "summary": "Recreated task summary.",
                        "tags": ["native"],
                        "title": "Recreated duplicate task"
                    }
                })))
                .expect("recreated task should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture recreated task history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::implicit_return,
            reason = "the shared active-task scenario keeps its distinct lifecycle state explicit for public completion-boundary fixtures"
        )]
        async fn signed_active_paged_history() -> Self {
            Self::signed_paged_history_with_status("in-progress").await
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "this dedicated public-CLI scenario needs fail-fast EventCore fixture construction and names the distinct board-order state"
        )]
        async fn signed_ordered_history() -> Self {
            let (directory, repository) = signed_repository();
            let first = task_stream(FIRST_PRIORITY_TASK_ID);
            let second = task_stream(SECOND_PRIORITY_TASK_ID);
            let closed = task_stream(CLOSED_TASK_ID);
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(first.clone(), StreamVersion::new(0))
                .expect("first task stream should register")
                .register_stream(second.clone(), StreamVersion::new(0))
                .expect("second task stream should register")
                .register_stream(closed.clone(), StreamVersion::new(0))
                .expect("closed task stream should register")
                .register_stream(board_stream, StreamVersion::new(0))
                .expect("fixture board stream should register")
                .append(task_created(
                    &first,
                    FIRST_PRIORITY_TASK_ID,
                    "First priority task",
                    "backlog",
                ))
                .expect("first task should append")
                .append(task_created(
                    &second,
                    SECOND_PRIORITY_TASK_ID,
                    "Second priority task",
                    "backlog",
                ))
                .expect("second task should append")
                .append(task_created(
                    &closed,
                    CLOSED_TASK_ID,
                    "Closed task must not be listed",
                    "done",
                ))
                .expect("closed task should append")
                .append(board_reordered(&[
                    SECOND_PRIORITY_TASK_ID,
                    FIRST_PRIORITY_TASK_ID,
                ]))
                .expect("board order should append")
                .append(board_task_details_updated(
                    FIRST_PRIORITY_TASK_ID,
                    "First priority task revised by board",
                ))
                .expect("board-side task update should append");
            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("fixture event store should initialize");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture task history should append");
            drop(store);
            commit_signed_tiber_history_with_shuffled_ingestion_cursor(&repository);

            Self {
                _directory: directory,
                repository,
            }
        }

        async fn signed_shell_sensitive_abandonment_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let task = task_stream(SHELL_SENSITIVE_TASK_ID);
            let board = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task.clone(), StreamVersion::new(0))
                .expect("shell-sensitive task stream should register")
                .register_stream(board.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after order and details")
                .append(task_created(
                    &task,
                    SHELL_SENSITIVE_TASK_ID,
                    "Owner's task",
                    "backlog",
                ))
                .expect("shell-sensitive task should append")
                .append(board_reordered(&[
                    SECOND_PRIORITY_TASK_ID,
                    FIRST_PRIORITY_TASK_ID,
                    SHELL_SENSITIVE_TASK_ID,
                ]))
                .expect("shell-sensitive board order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("shell-sensitive history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_abandoned_priority_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after order and details")
                .append(event(json!({
                    "event": "task_transitioned", "stream_id": board_stream.as_ref(),
                    "stem": FIRST_PRIORITY_TASK_ID, "status": "abandoned", "claim": null
                })))
                .expect("abandonment fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture abandonment should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_foreign_stream_abandonment_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let foreign_stream = task_stream(SECOND_PRIORITY_TASK_ID);
            let writes = StreamWrites::new()
                .register_stream(foreign_stream.clone(), StreamVersion::new(1))
                .expect("foreign task stream should register after creation")
                .append(event(json!({
                    "event": "task_transitioned", "stream_id": foreign_stream.as_ref(),
                    "stem": FIRST_PRIORITY_TASK_ID, "status": "abandoned", "claim": null
                })))
                .expect("foreign-stream abandonment fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream abandonment should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_commit_closed_priority_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after order and details")
                .append(event(json!({
                    "event": "tasks_closed_from_commit_trailers",
                    "stream_id": board_stream.as_ref(),
                    "stems": [FIRST_PRIORITY_TASK_ID],
                    "order": [SECOND_PRIORITY_TASK_ID]
                })))
                .expect("commit closure fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture commit closure should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_malformed_priority_repair_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let task_stream = task_stream(FIRST_PRIORITY_TASK_ID);
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(1))
                .expect("fixture task stream should register after creation")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": task_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": "tiber:board",
                        "order": [FIRST_PRIORITY_TASK_ID, SECOND_PRIORITY_TASK_ID]
                    },
                    "repairs": []
                })))
                .expect("malformed repair envelope should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("malformed repair history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_dependency_preservation_history() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after order and details")
                .append(event(json!({
                    "event": "task_links_changed", "stream_id": board_stream.as_ref(),
                    "stem": FIRST_PRIORITY_TASK_ID,
                    "blocks": [UNRELATED_TARGET_BLOCKS],
                    "blocked_by": [UNRELATED_TARGET_BLOCKED_BY]
                })))
                .expect("target links should append")
                .append(event(json!({
                    "event": "task_links_changed", "stream_id": board_stream.as_ref(),
                    "stem": SECOND_PRIORITY_TASK_ID,
                    "blocks": [UNRELATED_BLOCKER_BLOCKS],
                    "blocked_by": [UNRELATED_BLOCKER_BLOCKED_BY]
                })))
                .expect("blocker links should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture dependency links should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the bounded activation refusal fixture must fail fast while constructing one signed task with one unresolved durable blocker"
        )]
        async fn signed_blocked_history() -> Self {
            let (directory, repository) = signed_repository();
            let prerequisite_stream = task_stream(BLOCKER_TASK_ID);
            let target_stream = task_stream(BLOCKED_TASK_ID);
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(prerequisite_stream.clone(), StreamVersion::new(0))
                .expect("blocker task stream should register")
                .register_stream(target_stream.clone(), StreamVersion::new(0))
                .expect("blocked task stream should register")
                .register_stream(board_stream, StreamVersion::new(0))
                .expect("fixture board stream should register")
                .append(task_created(
                    &prerequisite_stream,
                    BLOCKER_TASK_ID,
                    "Blocking task",
                    "backlog",
                ))
                .expect("blocker task should append")
                .append(event(json!({
                    "event": "task_created",
                    "stream_id": target_stream.as_ref(),
                    "task": {
                        "acceptance": [],
                        "blocked_by": [BLOCKER_TASK_ID],
                        "blocks": [],
                        "claim": null,
                        "committed_at": "2026-08-13T00:00:00Z",
                        "context": "A blocker must be completed first.",
                        "notes": [],
                        "pr_mr_status": null,
                        "pr_mr_url": null,
                        "status": "backlog",
                        "stem": BLOCKED_TASK_ID,
                        "subtasks": [],
                        "summary": "Blocked activation fixture.",
                        "tags": ["native"],
                        "title": "Blocked task"
                    }
                })))
                .expect("blocked task should append")
                .append(board_reordered(&[BLOCKED_TASK_ID]))
                .expect("strict blocked-task order should append");
            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("fixture event store should initialize");
            let _slice = store
                .append_events(writes)
                .await
                .expect("blocked task history should append");
            drop(store);
            commit_signed_tiber_history(&repository);

            Self {
                _directory: directory,
                repository,
            }
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the stale-order fixture deliberately extends a signed multi-task board so the public completion adapter can prove its order-only reconciliation behavior; its one named caller keeps that scenario isolated"
        )]
        async fn signed_done_task_with_duplicate_stale_board_entries() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream, StreamVersion::new(2))
                .expect("fixture board stream should register after its retained order and detail facts")
                .append(board_reordered(&[
                    FIRST_PRIORITY_TASK_ID,
                    CLOSED_TASK_ID,
                    CLOSED_TASK_ID,
                    SECOND_PRIORITY_TASK_ID,
                ]))
                .expect("fixture stale board order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture stale board order should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::single_call_fn,
            reason = "the abandonment-only repaired-order fixture isolates one public lifecycle scenario from creation's established fixture contract"
        )]
        async fn signed_abandonment_validation_repaired_order() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let third_stream = task_stream(SHELL_SENSITIVE_TASK_ID);
            let writes = StreamWrites::new()
                .register_stream(third_stream.clone(), StreamVersion::new(0))
                .expect("third task stream should register")
                .register_stream(board_stream.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after retained order facts")
                .append(task_created(
                    &third_stream,
                    SHELL_SENSITIVE_TASK_ID,
                    "Owner's task",
                    "backlog",
                ))
                .expect("third task should append")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": board_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": board_stream.as_ref(),
                        "order": [SECOND_PRIORITY_TASK_ID, SHELL_SENSITIVE_TASK_ID, FIRST_PRIORITY_TASK_ID]
                    },
                    "repairs": [{
                        "repair": "board_entry_added",
                        "task": FIRST_PRIORITY_TASK_ID
                    }]
                })))
                .expect("validation-repair order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture validation repair should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::single_call_fn,
            reason = "the creation repaired-order fixture preserves one established public append-after-repair scenario"
        )]
        async fn signed_validation_repaired_order() -> Self {
            let fixture = Self::signed_ordered_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(2))
                .expect("fixture board stream should register after retained order facts")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": board_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": board_stream.as_ref(),
                        "order": [FIRST_PRIORITY_TASK_ID, SECOND_PRIORITY_TASK_ID]
                    },
                    "repairs": [{
                        "repair": "board_entry_added",
                        "task": FIRST_PRIORITY_TASK_ID
                    }]
                })))
                .expect("validation-repair order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture validation repair should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::implicit_return,
            reason = "the named backlog fixture returns its constructed scenario directly"
        )]
        async fn signed_paged_history() -> Self {
            Self::signed_paged_history_with_status("backlog").await
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-history fixture must fail fast while constructing one signed duplicate creation fact for the public write boundary"
        )]
        async fn signed_duplicate_creation_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(66))
                .expect("fixture task stream should register at its current version")
                .append(task_created(
                    &task_stream,
                    TASK_ID,
                    "Duplicate task creation",
                    "backlog",
                ))
                .expect("duplicate task creation fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("duplicate task creation fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-history fixture must fail fast while constructing one task creation fact on a foreign stream"
        )]
        async fn signed_foreign_stream_creation_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let foreign_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture existing task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(foreign_stream.clone(), StreamVersion::new(66))
                .expect("fixture existing task stream should register at its current version")
                .append(task_created(
                    &foreign_stream,
                    "20260816-wrng-foreign-stream",
                    "Foreign stream task",
                    "backlog",
                ))
                .expect("foreign-stream task creation fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream task creation fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-history fixture must fail fast while constructing one removal for an absent task"
        )]
        async fn signed_absent_task_removal_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(historical_task_removed(
                    &board_stream,
                    "20260816-gone-never-created",
                ))
                .expect("absent-task removal fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("absent-task removal fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_foreign_stream_removal_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let foreign_stream = StreamId::try_new("tiber:task:foreign-owner".to_owned())
                .expect("fixture foreign task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(foreign_stream.clone(), StreamVersion::new(0))
                .expect("fixture foreign task stream should register")
                .append(historical_task_removed(&foreign_stream, TASK_ID))
                .expect("foreign-stream removal should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream removal should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_removed_task_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(historical_task_removed(&board_stream, TASK_ID))
                .expect("valid removal should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("valid removal should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the malformed-order fixture must fail fast while constructing one signed duplicate board occurrence"
        )]
        async fn signed_duplicate_board_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream, StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(board_reordered(&[TASK_ID, TASK_ID]))
                .expect("duplicate board order fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("duplicate board order fact should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        async fn signed_missing_target_board_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream, StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(board_reordered(&[]))
                .expect("missing-target board order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("missing-target board order should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the retry-identity fixture must fail fast while retaining one noncanonical numeric suffix"
        )]
        async fn signed_noncanonical_retry_suffix_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let noncanonical_id = "20260816-rtry-exact-stable-prefix-02";
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(task_created(
                    &board_stream,
                    noncanonical_id,
                    "Exact stable prefix",
                    "backlog",
                ))
                .expect("noncanonical retry-suffix task should append")
                .append(board_reordered(&[TASK_ID, noncanonical_id]))
                .expect("fixture board order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("noncanonical retry-suffix history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the malformed-order fixture must fail fast while carrying one board order fact on a retained task stream"
        )]
        async fn signed_foreign_stream_board_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(66))
                .expect("fixture task stream should register at its current version")
                .append(event(json!({
                    "event": "board_reordered",
                    "stream_id": task_stream.as_ref(),
                    "order": [TASK_ID]
                })))
                .expect("foreign-stream board order fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream board order history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-order fixture must fail fast while carrying one priority fact on a retained task stream"
        )]
        async fn signed_foreign_stream_priority_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(66))
                .expect("fixture task stream should register at its current version")
                .append(event(json!({
                    "event": "task_priority_changed",
                    "stream_id": task_stream.as_ref(),
                    "order": [TASK_ID]
                })))
                .expect("foreign-stream priority order fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream priority history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the malformed-order fixture must fail fast while carrying one commit-closure order on a retained task stream"
        )]
        async fn signed_foreign_stream_commit_closure_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(66))
                .expect("fixture task stream should register at its current version")
                .append(event(json!({
                    "event": "tasks_closed_from_commit_trailers",
                    "stream_id": task_stream.as_ref(),
                    "stems": [],
                    "order": [TASK_ID]
                })))
                .expect("foreign-stream commit-closure fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream commit-closure history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-repair fixture must fail fast while carrying one validation repair on a retained task stream"
        )]
        async fn signed_foreign_stream_validation_repair_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(66))
                .expect("fixture task stream should register at its current version")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": task_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": board_stream.as_ref(),
                        "order": [TASK_ID]
                    },
                    "repairs": []
                })))
                .expect("foreign-stream validation repair should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("foreign-stream validation repair should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-repair fixture must fail fast while retaining a board repair whose embedded order names a task stream"
        )]
        async fn signed_mismatched_validation_repair_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": board_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": task_stream.as_ref(),
                        "order": [TASK_ID]
                    },
                    "repairs": []
                })))
                .expect("mismatched validation repair should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("mismatched validation repair should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-order fixture must fail fast while retaining duplicate priority membership"
        )]
        async fn signed_duplicate_priority_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(event(json!({
                    "event": "task_priority_changed",
                    "stream_id": board_stream.as_ref(),
                    "order": [TASK_ID, TASK_ID]
                })))
                .expect("duplicate priority order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("duplicate priority history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-order fixture must fail fast while retaining duplicate commit-closure order membership"
        )]
        async fn signed_duplicate_commit_closure_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(event(json!({
                    "event": "tasks_closed_from_commit_trailers",
                    "stream_id": board_stream.as_ref(),
                    "stems": [],
                    "order": [TASK_ID, TASK_ID]
                })))
                .expect("duplicate commit-closure order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("duplicate commit-closure history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the malformed-repair fixture must fail fast while retaining duplicate repaired-order membership"
        )]
        async fn signed_duplicate_validation_repair_order_history() -> Self {
            let fixture = Self::signed_paged_history().await;
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(board_stream.clone(), StreamVersion::new(1))
                .expect("fixture board stream should register at its current version")
                .append(event(json!({
                    "event": "task_validation_repaired",
                    "stream_id": board_stream.as_ref(),
                    "link_changes": [],
                    "order_change": {
                        "stream_id": board_stream.as_ref(),
                        "order": [TASK_ID, TASK_ID]
                    },
                    "repairs": []
                })))
                .expect("duplicate validation-repair order should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("duplicate validation-repair history should persist");
            drop(store);
            commit_signed_tiber_history(&fixture.repository);
            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "the signed public-CLI fixture deliberately fails fast when its deterministic EventCore setup cannot be created"
        )]
        async fn signed_paged_history_with_status(status: &str) -> Self {
            let (directory, repository) = signed_repository();

            let task_stream = StreamId::try_new(format!("tiber:task:{TASK_ID}"))
                .expect("fixture task stream should be valid");
            let board_stream = StreamId::try_new("tiber:board".to_owned())
                .expect("fixture board stream should be valid");
            let mut writes = StreamWrites::new()
                .register_stream(task_stream.clone(), StreamVersion::new(0))
                .expect("fixture task stream should register")
                .register_stream(board_stream, StreamVersion::new(0))
                .expect("fixture board stream should register")
                .append(task_created(
                    &task_stream,
                    TASK_ID,
                    "Paged task revision zero",
                    status,
                ))
                .expect("fixture task creation should append")
                .append(board_reordered(&[TASK_ID]))
                .expect("fixture board order should append");
            for revision in 0..PAGE_SPANNING_UPDATES {
                writes = writes
                    .append(task_details_updated(&task_stream, revision))
                    .expect("fixture detail update should append");
            }
            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("fixture event store should initialize");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture task history should append");
            drop(store);

            commit_signed_tiber_history(&repository);

            Self {
                _directory: directory,
                repository,
            }
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the dedicated shared-event-type scenario deliberately uses fail-fast EventCore fixture construction to exercise one public CLI boundary"
        )]
        async fn signed_paged_history_with_unrelated_shared_event_type() -> Self {
            let fixture = Self::signed_paged_history().await;
            let ci_recovery_stream = StreamId::try_new("tiber:ci-recovery".to_owned())
                .expect("fixture CI-recovery stream should be valid");
            let writes = StreamWrites::new()
                .register_stream(ci_recovery_stream.clone(), StreamVersion::new(0))
                .expect("fixture CI-recovery stream should register")
                .append(LegacyCiRecoveryEvent {
                    recovery_id: "recovery-fixture-1".to_owned(),
                    stream_id: ci_recovery_stream,
                })
                .expect("fixture CI-recovery fact should append");
            let store = FileEventStore::open(fixture.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture CI-recovery fact should persist");
            commit_signed_tiber_history(&fixture.repository);

            fixture
        }

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            reason = "a fixture process-start failure cannot yield meaningful CLI behavior evidence"
        )]
        fn tiber(&self, arguments: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(arguments)
                .current_dir(&self.repository)
                .output()
                .expect("Tiber CLI should execute")
        }
    }

    #[tokio::test]
    async fn tasks_list_replays_every_page_of_signed_tiber_history() {
        let fixture = TaskFixture::signed_paged_history().await;

        let output = fixture.tiber(&["tasks", "list"]);

        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stderr).is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!(
                "{TASK_ID}\tbacklog\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_show_displays_the_original_durable_creation_timestamp() {
        let fixture = TaskFixture::signed_paged_history().await;

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);

        assert_success(&shown);
        assert!(
            String::from_utf8_lossy(&shown.stdout).contains("committed-at: 2026-08-12T00:00:00Z\n"),
            "task show must expose the retained original creation timestamp"
        );
    }

    #[tokio::test]
    async fn tasks_list_ignores_non_task_streams_that_share_the_legacy_event_type() {
        let fixture = TaskFixture::signed_paged_history_with_unrelated_shared_event_type().await;

        let output = fixture.tiber(&["tasks", "list"]);

        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!(
                "{TASK_ID}\tbacklog\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_read_commands_query_the_native_projection() {
        let fixture = TaskFixture::signed_paged_history().await;

        let listed = fixture.tiber(&["tasks", "list", "--status", "backlog"]);
        assert_success(&listed);
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout),
            format!(
                "{TASK_ID}\tbacklog\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        assert_eq!(
            String::from_utf8_lossy(&shown.stdout),
            format!(
                "id: {TASK_ID}\nstatus: backlog\ntitle: Paged task revision {}\ncommitted-at: 2026-08-12T00:00:00Z\nsummary: History page {}.\ncontext: Read the complete EventCore history.\ntags: native\n",
                PAGE_SPANNING_UPDATES - 1,
                PAGE_SPANNING_UPDATES - 1
            )
        );

        let searched = fixture.tiber(&["tasks", "search", "complete", "EventCore"]);
        assert_success(&searched);
        assert_eq!(
            String::from_utf8_lossy(&searched.stdout),
            format!(
                "{TASK_ID}\tbacklog\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );

        let next = fixture.tiber(&["tasks", "next"]);
        assert_success(&next);
        assert_eq!(
            String::from_utf8_lossy(&next.stdout),
            format!(
                "{TASK_ID}\tbacklog\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_create_publishes_a_backlog_task_visible_through_native_queries() {
        let fixture = TaskFixture::signed_paged_history().await;

        let created = fixture.tiber(&["tasks", "create", "Native task administration"]);

        assert_success(&created);
        let created_text = String::from_utf8_lossy(&created.stdout);
        let task_id = created_text
            .strip_prefix("created ")
            .and_then(|text| text.split_once(" at ").map(|(id, _revision)| id))
            .expect("successful creation names the durable task ID and authority revision");

        let listed = fixture.tiber(&["tasks", "list", "--status", "backlog"]);
        assert_success(&listed);
        assert!(
            String::from_utf8_lossy(&listed.stdout)
                .contains(&format!("{task_id}\tbacklog\tNative task administration\n")),
            "a created task must immediately appear in the native board projection"
        );
    }

    #[tokio::test]
    async fn tasks_update_replaces_the_title_summary_and_context_visible_through_native_queries() {
        let fixture = TaskFixture::signed_paged_history().await;

        let updated = fixture.tiber(&[
            "tasks",
            "update",
            TASK_ID,
            "--title",
            "Resume a durable task conversation",
            "--summary",
            "Persist the owner-visible conversation and its exact next action.",
            "--context",
            "Drive the packaged Tiber binary through exit and relaunch.",
        ]);

        assert_success(&updated);
        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        let shown = String::from_utf8_lossy(&shown.stdout);
        assert!(shown.contains("title: Resume a durable task conversation\n"));
        assert!(shown.contains(
            "summary: Persist the owner-visible conversation and its exact next action.\n"
        ));
        assert!(
            shown.contains("context: Drive the packaged Tiber binary through exit and relaunch.\n")
        );
        assert!(shown.contains("tags: native\n"));
    }

    #[tokio::test]
    async fn tasks_acceptance_add_appends_a_criterion_visible_through_native_queries() {
        let fixture = TaskFixture::signed_acceptance_history().await;

        let added = fixture.tiber(&[
            "tasks",
            "acceptance",
            "add",
            TASK_ID,
            "The packaged binary restores the exact next action after restart.",
        ]);

        assert_success(&added);
        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        assert!(
            shown_text.contains(
                "3. [ ] The packaged binary restores the exact next action after restart.\n"
            ),
            "unexpected task projection:\n{shown_text}"
        );
    }

    #[tokio::test]
    async fn tasks_link_blocked_by_exposes_the_reciprocal_dependency_through_native_queries() {
        let fixture = TaskFixture::signed_ordered_history().await;

        let linked = fixture.tiber(&[
            "tasks",
            "link",
            "blocked-by",
            FIRST_PRIORITY_TASK_ID,
            SECOND_PRIORITY_TASK_ID,
        ]);

        assert_success(&linked);
        let revision = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        assert_eq!(
            String::from_utf8_lossy(&linked.stdout),
            format!(
                "linked {FIRST_PRIORITY_TASK_ID} blocked by {SECOND_PRIORITY_TASK_ID} at {revision}\n"
            )
        );
        assert!(String::from_utf8_lossy(&linked.stderr).is_empty());
        let target = fixture.tiber(&["tasks", "show", FIRST_PRIORITY_TASK_ID]);
        assert_success(&target);
        assert!(
            String::from_utf8_lossy(&target.stdout)
                .contains(&format!("blocked-by: {SECOND_PRIORITY_TASK_ID}\n"))
        );
        let blocker = fixture.tiber(&["tasks", "show", SECOND_PRIORITY_TASK_ID]);
        assert_success(&blocker);
        assert!(
            String::from_utf8_lossy(&blocker.stdout)
                .contains(&format!("blocks: {FIRST_PRIORITY_TASK_ID}\n"))
        );
    }

    #[tokio::test]
    async fn tasks_prioritize_moves_one_task_before_another_without_disturbing_other_order() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "20260816-k111",
            "Third priority task",
        ]);
        assert_success(&created);
        let third_task_id = created_task_id(&created);

        let prioritized = fixture.tiber(&[
            "tasks",
            "prioritize",
            third_task_id.as_str(),
            "--before",
            FIRST_PRIORITY_TASK_ID,
        ]);

        assert_success(&prioritized);
        assert!(
            String::from_utf8_lossy(&prioritized.stdout).starts_with(&format!(
                "prioritized {third_task_id} before {FIRST_PRIORITY_TASK_ID} at "
            )),
            "the successful reorder must name both tasks and its confirmed authority revision"
        );
        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout),
            format!(
                "{SECOND_PRIORITY_TASK_ID}\tbacklog\tSecond priority task\n{third_task_id}\tbacklog\tThird priority task\n{FIRST_PRIORITY_TASK_ID}\tbacklog\tFirst priority task revised by board\n"
            ),
            "prioritization must move only the requested task and preserve every other relative position"
        );
    }

    #[tokio::test]
    async fn tasks_prioritize_rejects_self_reference_without_advancing_authority() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&[
            "tasks",
            "prioritize",
            FIRST_PRIORITY_TASK_ID,
            "--before",
            FIRST_PRIORITY_TASK_ID,
        ]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_priority_self_reference: task `{FIRST_PRIORITY_TASK_ID}` cannot be prioritized before itself\n"
            )
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected self-reference must not advance task authority"
        );
    }

    #[tokio::test]
    async fn tasks_prioritize_rejects_a_done_task_in_either_endpoint_role() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        for invocation in [
            [
                "tasks",
                "prioritize",
                CLOSED_TASK_ID,
                "--before",
                FIRST_PRIORITY_TASK_ID,
            ],
            [
                "tasks",
                "prioritize",
                FIRST_PRIORITY_TASK_ID,
                "--before",
                CLOSED_TASK_ID,
            ],
        ] {
            let output = fixture.tiber(&invocation);

            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                format!(
                    "tasks_command_task_priority_endpoint_not_open: task `{CLOSED_TASK_ID}` is currently `done`, so it cannot define open-board priority\n"
                )
            );
            assert_eq!(
                git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
                before,
                "a terminal priority endpoint must not advance task authority"
            );
        }
    }

    #[tokio::test]
    async fn tasks_prioritize_rejects_abandoned_and_commit_closed_endpoints() {
        for (fixture, status) in [
            (
                TaskFixture::signed_abandoned_priority_history().await,
                "abandoned",
            ),
            (
                TaskFixture::signed_commit_closed_priority_history().await,
                "done",
            ),
        ] {
            let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
            let output = fixture.tiber(&[
                "tasks",
                "prioritize",
                FIRST_PRIORITY_TASK_ID,
                "--before",
                SECOND_PRIORITY_TASK_ID,
            ]);

            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                format!(
                    "tasks_command_task_priority_endpoint_not_open: task `{FIRST_PRIORITY_TASK_ID}` is currently `{status}`, so it cannot define open-board priority\n"
                )
            );
            assert_eq!(
                git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
                before,
                "a terminal priority endpoint must not advance task authority"
            );
        }
    }

    #[tokio::test]
    async fn tasks_prioritize_rejects_a_repair_order_from_a_non_board_envelope() {
        let fixture = TaskFixture::signed_malformed_priority_repair_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&[
            "tasks",
            "prioritize",
            SECOND_PRIORITY_TASK_ID,
            "--before",
            FIRST_PRIORITY_TASK_ID,
        ]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tasks_command_task_priority_malformed_history: the authoritative Tiber task history could not decide that task change\n"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a malformed repair envelope must not authorize priority publication"
        );
    }

    #[tokio::test]
    async fn tasks_prioritize_reconciles_an_exact_retry_without_advancing_authority_twice() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let first = fixture.tiber(&[
            "tasks",
            "prioritize",
            FIRST_PRIORITY_TASK_ID,
            "--before",
            SECOND_PRIORITY_TASK_ID,
        ]);
        assert_success(&first);
        let after_first = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&[
            "tasks",
            "prioritize",
            FIRST_PRIORITY_TASK_ID,
            "--before",
            SECOND_PRIORITY_TASK_ID,
        ]);

        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!(
                "{FIRST_PRIORITY_TASK_ID} already prioritized before {SECOND_PRIORITY_TASK_ID}\n"
            )
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first,
            "an exact durable retry must not publish a second priority revision"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_removes_one_backlog_task_from_the_open_board() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let abandoned =
            fixture.tiber(&["tasks", "transition", FIRST_PRIORITY_TASK_ID, "abandoned"]);

        assert_success(&abandoned);
        let after = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        assert_ne!(after, before);
        assert_eq!(
            String::from_utf8_lossy(&abandoned.stdout),
            format!("abandoned {FIRST_PRIORITY_TASK_ID} at {after}\n")
        );
        let open = fixture.tiber(&["tasks", "list"]);
        assert_success(&open);
        assert_eq!(
            String::from_utf8_lossy(&open.stdout),
            format!("{SECOND_PRIORITY_TASK_ID}\tbacklog\tSecond priority task\n")
        );
        let terminal = fixture.tiber(&["tasks", "list", "--status", "abandoned"]);
        assert_success(&terminal);
        assert_eq!(
            String::from_utf8_lossy(&terminal.stdout),
            format!("{FIRST_PRIORITY_TASK_ID}\tabandoned\tFirst priority task revised by board\n")
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_uses_the_latest_repaired_board_order() {
        let fixture = TaskFixture::signed_abandonment_validation_repaired_order().await;

        let abandoned =
            fixture.tiber(&["tasks", "transition", FIRST_PRIORITY_TASK_ID, "abandoned"]);

        assert_success(&abandoned);
        let open = fixture.tiber(&["tasks", "list"]);
        assert_success(&open);
        assert_eq!(
            String::from_utf8_lossy(&open.stdout),
            format!(
                "{SECOND_PRIORITY_TASK_ID}\tbacklog\tSecond priority task\n{SHELL_SENSITIVE_TASK_ID}\tbacklog\tOwner's task\n"
            )
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_removes_one_active_task_from_the_open_board() {
        let fixture = TaskFixture::signed_active_paged_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "abandoned"]);

        assert_success(&output);
        let after = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        assert_ne!(after, before);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("abandoned {TASK_ID} at {after}\n")
        );
        let open = fixture.tiber(&["tasks", "list"]);
        assert_success(&open);
        assert!(open.stdout.is_empty());
        let terminal = fixture.tiber(&["tasks", "list", "--status", "abandoned"]);
        assert_success(&terminal);
        assert_eq!(
            String::from_utf8_lossy(&terminal.stdout),
            format!(
                "{TASK_ID}\tabandoned\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_a_done_task_without_advancing_authority() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", CLOSED_TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_abandonment_not_backlog: task `{CLOSED_TASK_ID}` is currently `done`; abandonment requires `backlog` or `in-progress`\n"
            )
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a done task refusal must not advance task authority"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_reconciles_an_exact_durable_retry() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let first = fixture.tiber(&["tasks", "transition", FIRST_PRIORITY_TASK_ID, "abandoned"]);
        assert_success(&first);
        let after_first = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&["tasks", "transition", FIRST_PRIORITY_TASK_ID, "abandoned"]);

        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("{FIRST_PRIORITY_TASK_ID} already abandoned\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first,
            "an exact durable retry must not publish a second abandonment revision"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_a_matching_fact_from_a_foreign_stream() {
        let fixture = TaskFixture::signed_foreign_stream_abandonment_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", FIRST_PRIORITY_TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with("tasks_command_target_task_fact_unexpected_stream:")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream transition must not authorize abandonment reconciliation"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_board_order_from_a_foreign_stream() {
        let fixture = TaskFixture::signed_foreign_stream_board_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tasks_command_task_abandonment_malformed_history: the authoritative Tiber task history could not decide that task change\n"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream board order must not authorize abandonment"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_an_order_missing_the_target() {
        let fixture = TaskFixture::signed_missing_target_board_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tasks_command_task_abandonment_malformed_history: the authoritative Tiber task history could not decide that task change\n"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_duplicate_target_order_membership() {
        let fixture = TaskFixture::signed_duplicate_board_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with("tasks_command_task_abandonment_malformed_history:")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_rejects_commit_closure_from_a_foreign_stream() {
        let fixture = TaskFixture::signed_foreign_stream_commit_closure_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "abandoned"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tasks_command_task_abandonment_malformed_history: the authoritative Tiber task history could not decide that task change\n"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream commit closure must not authorize abandonment"
        );
    }

    #[tokio::test]
    async fn tasks_transition_abandoned_reconciles_a_shell_safe_retry_after_ambiguous_publication()
    {
        let fixture = TaskFixture::signed_shell_sensitive_abandonment_history().await;
        let remote = fixture._directory.path().join("abandonment-remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture
            ._directory
            .path()
            .join("abandonment-ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("wrapper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("wrapper executable");
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .expect("Git discovery")
                .stdout,
        )
        .expect("Git path UTF-8")
        .trim()
        .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture._directory.path().join("abandonment-push-completed");
        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "transition", "20260812-e555", "abandoned"])
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", &base)
            .output()
            .expect("Tiber CLI");

        assert!(!ambiguous.status.success());
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        assert_ne!(
            remote_after_ambiguous,
            base,
            "ambiguous command stderr: {}",
            String::from_utf8_lossy(&ambiguous.stderr)
        );
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let retry_command = diagnostic
            .strip_prefix("tiber_store_publication_ambiguous: task abandonment may already be durable; retry exactly: `")
            .and_then(|text| text.strip_suffix("`\n"))
            .expect("exact abandonment retry diagnostic");
        assert_eq!(
            retry_command, "tiber tasks transition '20260812-e555-owner'\\''s-task' abandoned",
            "the displayed retry must quote the resolved semantic task argument"
        );
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("binary parent");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("retry command");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!("{SHELL_SENSITIVE_TASK_ID} already abandoned\n")
        );
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous,
            "reconciliation must not publish a second abandonment revision"
        );
    }

    #[tokio::test]
    async fn tasks_prioritize_reconciles_a_shell_safe_retry_after_ambiguous_publication() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let remote = fixture._directory.path().join("priority-remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture
            ._directory
            .path()
            .join("priority-ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("wrapper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("wrapper executable");
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .expect("Git discovery")
                .stdout,
        )
        .expect("Git path UTF-8")
        .trim()
        .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture._directory.path().join("priority-push-completed");
        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args([
                "tasks",
                "prioritize",
                FIRST_PRIORITY_TASK_ID,
                "--before",
                SECOND_PRIORITY_TASK_ID,
            ])
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", &base)
            .output()
            .expect("Tiber CLI");

        assert!(!ambiguous.status.success());
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        assert_ne!(remote_after_ambiguous, base);
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let retry_command = diagnostic
            .strip_prefix("tiber_store_publication_ambiguous: priority change may already be durable; retry exactly: `")
            .and_then(|text| text.strip_suffix("`\n"))
            .expect("exact priority retry diagnostic");
        assert_eq!(
            retry_command,
            format!(
                "tiber tasks prioritize '{FIRST_PRIORITY_TASK_ID}' --before '{SECOND_PRIORITY_TASK_ID}'"
            ),
            "the displayed retry must quote each resolved semantic argument"
        );
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("binary parent");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("retry command");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!(
                "{FIRST_PRIORITY_TASK_ID} already prioritized before {SECOND_PRIORITY_TASK_ID}\n"
            )
        );
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous,
            "reconciliation must not publish a second priority revision"
        );
    }

    #[tokio::test]
    async fn tasks_link_blocked_by_rejects_a_self_link_without_advancing_authority() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let linked = fixture.tiber(&[
            "tasks",
            "link",
            "blocked-by",
            FIRST_PRIORITY_TASK_ID,
            FIRST_PRIORITY_TASK_ID,
        ]);

        assert!(!linked.status.success());
        assert_eq!(
            String::from_utf8_lossy(&linked.stderr),
            format!(
                "tasks_command_dependency_self_link: task `{FIRST_PRIORITY_TASK_ID}` cannot be blocked by itself\n"
            )
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before
        );
    }

    #[tokio::test]
    async fn tasks_link_blocked_by_preserves_unrelated_links_on_both_tasks() {
        let fixture = TaskFixture::signed_dependency_preservation_history().await;

        let linked = fixture.tiber(&[
            "tasks",
            "link",
            "blocked-by",
            FIRST_PRIORITY_TASK_ID,
            SECOND_PRIORITY_TASK_ID,
        ]);

        assert_success(&linked);
        let target = fixture.tiber(&["tasks", "show", FIRST_PRIORITY_TASK_ID]);
        assert_success(&target);
        let target = String::from_utf8_lossy(&target.stdout);
        assert!(target.contains(&format!("blocks: {UNRELATED_TARGET_BLOCKS}\n")));
        assert!(target.contains(&format!(
            "blocked-by: {UNRELATED_TARGET_BLOCKED_BY}, {SECOND_PRIORITY_TASK_ID}\n"
        )));
        let blocker = fixture.tiber(&["tasks", "show", SECOND_PRIORITY_TASK_ID]);
        assert_success(&blocker);
        let blocker = String::from_utf8_lossy(&blocker.stdout);
        assert!(blocker.contains(&format!(
            "blocks: {UNRELATED_BLOCKER_BLOCKS}, {FIRST_PRIORITY_TASK_ID}\n"
        )));
        assert!(blocker.contains(&format!("blocked-by: {UNRELATED_BLOCKER_BLOCKED_BY}\n")));
    }

    #[tokio::test]
    async fn tasks_link_blocked_by_reconciles_an_exact_durable_retry() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let arguments = [
            "tasks",
            "link",
            "blocked-by",
            FIRST_PRIORITY_TASK_ID,
            SECOND_PRIORITY_TASK_ID,
        ];

        let linked = fixture.tiber(&arguments);
        assert_success(&linked);
        let linked_revision = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let retried = fixture.tiber(&arguments);

        assert_success(&retried);
        assert_eq!(
            String::from_utf8_lossy(&retried.stdout),
            format!("{FIRST_PRIORITY_TASK_ID} already blocked by {SECOND_PRIORITY_TASK_ID}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            linked_revision
        );
    }

    #[tokio::test]
    async fn tasks_link_blocked_by_reconciles_an_exact_retry_after_ambiguous_publication() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let remote = fixture._directory.path().join("dependency-link-remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture
            ._directory
            .path()
            .join("dependency-link-ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("wrapper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("wrapper executable");
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .expect("Git discovery")
                .stdout,
        )
        .expect("Git path UTF-8")
        .trim()
        .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture
            ._directory
            .path()
            .join("dependency-link-push-completed");
        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args([
                "tasks",
                "link",
                "blocked-by",
                FIRST_PRIORITY_TASK_ID,
                SECOND_PRIORITY_TASK_ID,
            ])
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", &base)
            .output()
            .expect("Tiber CLI");
        assert!(!ambiguous.status.success());
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        assert_ne!(remote_after_ambiguous, base);
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let retry_command = diagnostic
            .strip_prefix("tiber_store_publication_ambiguous: dependency link may already be durable; retry exactly: `")
            .and_then(|text| text.strip_suffix("`\n"))
            .expect("exact retry diagnostic");
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("binary parent");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("retry command");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!("{FIRST_PRIORITY_TASK_ID} already blocked by {SECOND_PRIORITY_TASK_ID}\n")
        );
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous
        );
    }

    #[tokio::test]
    async fn tasks_acceptance_add_reconciles_a_shell_safe_retry_after_ambiguous_publication() {
        let fixture = TaskFixture::signed_paged_history().await;
        let remote = fixture._directory.path().join("acceptance-add-remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture
            ._directory
            .path()
            .join("acceptance-add-ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("wrapper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("wrapper executable");
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .expect("Git discovery")
                .stdout,
        )
        .expect("Git path UTF-8")
        .trim()
        .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture
            ._directory
            .path()
            .join("acceptance-add-push-completed");
        let sentinel = fixture._directory.path().join("must-not-exist");
        let criterion = format!("Owner's exact criterion; touch {}", sentinel.display());
        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "acceptance", "add", TASK_ID, criterion.as_str()])
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", &base)
            .output()
            .expect("Tiber CLI");
        assert!(!ambiguous.status.success());
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        assert_ne!(remote_after_ambiguous, base);
        let retry_command = diagnostic
            .strip_prefix("tiber_store_publication_ambiguous: acceptance addition may already be durable; retry exactly: `")
            .and_then(|text| text.strip_suffix("`\n"))
            .expect("exact retry diagnostic");
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("binary parent");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("retry command");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!("criterion already added for {TASK_ID}\n")
        );
        assert!(!sentinel.exists());
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous
        );
        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        assert_eq!(
            String::from_utf8_lossy(&shown.stdout)
                .matches(&criterion)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn tasks_update_reconciles_the_exact_retry_after_ambiguous_publication() {
        let fixture = TaskFixture::signed_paged_history().await;
        let remote = fixture._directory.path().join("update-remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture._directory.path().join("update-ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("wrapper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("wrapper executable");
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .expect("Git discovery")
                .stdout,
        )
        .expect("Git path UTF-8")
        .trim()
        .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture._directory.path().join("update-push-completed");
        let invocation = [
            "tasks",
            "update",
            TASK_ID,
            "--title",
            "Ambiguous title",
            "--summary",
            "Ambiguous summary",
            "--context",
            "Ambiguous context",
        ];
        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(invocation)
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", base)
            .output()
            .expect("Tiber CLI");
        assert!(!ambiguous.status.success());
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        let retry_command = diagnostic.strip_prefix("tiber_store_publication_ambiguous: task update may already be durable; retry exactly: `")
            .and_then(|text| text.strip_suffix("`\n")).expect("exact retry diagnostic");
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("binary parent");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("retry command");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!("{TASK_ID} already updated\n")
        );
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous
        );
    }

    #[tokio::test]
    async fn tasks_create_initializes_backlog_status_and_empty_optional_details() {
        let fixture = TaskFixture::signed_paged_history().await;

        let created = fixture.tiber(&["tasks", "create", "Initial task fields"]);

        assert_success(&created);
        let task_id = created_task_id(&created);
        let shown = fixture.tiber(&["tasks", "show", task_id.as_str()]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        assert!(
            shown_text.starts_with(&format!(
                "id: {task_id}\nstatus: backlog\ntitle: Initial task fields\ncommitted-at: "
            )),
            "a created task must begin in backlog with its requested title"
        );
        assert!(
            shown_text.ends_with("summary: \ncontext: \n"),
            "a created task must begin with empty optional details"
        );
    }

    #[tokio::test]
    async fn tasks_create_records_the_invocation_time_as_a_utc_timestamp() {
        let fixture = TaskFixture::signed_paged_history().await;
        let instant_before = Utc::now();

        let created = fixture.tiber(&["tasks", "create", "Timestamped task"]);

        let instant_after = Utc::now();
        assert_success(&created);
        let task_id = created_task_id(&created);
        let shown = fixture.tiber(&["tasks", "show", task_id.as_str()]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        let committed_at = shown_text
            .lines()
            .find_map(|line| line.strip_prefix("committed-at: "))
            .expect("a newly created task exposes its durable creation timestamp");
        assert_eq!(committed_at.len(), 20);
        assert_eq!(&committed_at[4..5], "-");
        assert_eq!(&committed_at[7..8], "-");
        assert_eq!(&committed_at[10..11], "T");
        assert_eq!(&committed_at[13..14], ":");
        assert_eq!(&committed_at[16..17], ":");
        assert!(committed_at.ends_with('Z'));
        assert!(
            committed_at
                .chars()
                .enumerate()
                .all(
                    |(index, character)| matches!(index, 4 | 7 | 10 | 13 | 16 | 19)
                        || character.is_ascii_digit(),
                ),
            "the creation timestamp must contain only UTC date-time digits and separators"
        );
        let committed_instant = DateTime::parse_from_rfc3339(committed_at)
            .expect("the durable creation timestamp must parse as RFC 3339")
            .with_timezone(&Utc);
        assert!(
            committed_instant.timestamp() >= instant_before.timestamp()
                && committed_instant.timestamp() <= instant_after.timestamp(),
            "the durable creation timestamp must fall within the invocation interval"
        );
    }

    #[tokio::test]
    async fn tasks_create_disambiguates_repeated_title_nicknames() {
        let fixture = TaskFixture::signed_paged_history().await;

        let first = fixture.tiber(&["tasks", "create", "Repeated native title"]);
        assert_success(&first);
        let second = fixture.tiber(&["tasks", "create", "Repeated native title"]);
        assert_success(&second);

        let first_id = created_task_id(&first);
        let second_id = created_task_id(&second);
        assert!(first_id.ends_with("-repeated-native-title"));
        assert!(second_id.ends_with("-repeated-native-title-2"));

        let first_shown = fixture.tiber(&["tasks", "show", "repeated-native-title"]);
        assert_success(&first_shown);
        assert!(
            String::from_utf8_lossy(&first_shown.stdout).starts_with(&format!("id: {first_id}\n"))
        );
        let second_shown = fixture.tiber(&["tasks", "show", "repeated-native-title-2"]);
        assert_success(&second_shown);
        assert!(
            String::from_utf8_lossy(&second_shown.stdout)
                .starts_with(&format!("id: {second_id}\n"))
        );
    }

    #[tokio::test]
    async fn tasks_create_preserves_the_latest_validation_repaired_order() {
        let fixture = TaskFixture::signed_validation_repaired_order().await;

        let before = fixture.tiber(&["tasks", "list"]);
        assert_success(&before);
        assert_eq!(
            task_row_ids(&before),
            [FIRST_PRIORITY_TASK_ID, SECOND_PRIORITY_TASK_ID],
            "the public projection must expose the repaired order before creation"
        );

        let created = fixture.tiber(&["tasks", "create", "After repaired order"]);
        assert_success(&created);
        let created_id = created_task_id(&created);

        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert_eq!(
            task_row_ids(&listed),
            [
                FIRST_PRIORITY_TASK_ID,
                SECOND_PRIORITY_TASK_ID,
                created_id.as_str()
            ],
            "creation must append after the complete latest repaired strict order"
        );
    }

    #[tokio::test]
    async fn tasks_create_retries_one_stable_creation_identity_without_duplication() {
        let fixture = TaskFixture::signed_paged_history().await;
        let invocation = [
            "tasks",
            "create",
            "--id",
            "20260816-rtry",
            "Retry-safe creation",
        ];

        let first = fixture.tiber(&invocation);
        assert_success(&first);
        let task_id = created_task_id(&first);
        assert_eq!(task_id, "20260816-rtry-retry-safe-creation");
        let after_first = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&invocation);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("already created {task_id}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first,
            "retrying one stable creation identity must not publish a duplicate task"
        );
    }

    #[tokio::test]
    async fn tasks_create_retries_the_disambiguated_task_for_one_stable_identity() {
        let fixture = TaskFixture::signed_paged_history().await;
        let occupied = fixture.tiber(&["tasks", "create", "Retry-safe creation"]);
        assert_success(&occupied);
        let invocation = [
            "tasks",
            "create",
            "--id",
            "20260816-rtry",
            "Retry-safe creation",
        ];

        let first = fixture.tiber(&invocation);
        assert_success(&first);
        let task_id = created_task_id(&first);
        assert_eq!(task_id, "20260816-rtry-retry-safe-creation-2");
        let after_first = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&invocation);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("already created {task_id}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first,
            "retrying a disambiguated stable creation identity must not publish another task"
        );
    }

    #[tokio::test]
    async fn tasks_create_does_not_alias_a_longer_stable_prefix_to_a_shorter_one() {
        let fixture = TaskFixture::signed_paged_history().await;
        let title = "Exact stable prefix";
        let longer = fixture.tiber(&["tasks", "create", "--id", "20260816-rtry-long", title]);
        assert_success(&longer);
        let longer_id = created_task_id(&longer);
        assert_eq!(longer_id, "20260816-rtry-long-exact-stable-prefix");

        let shorter_invocation = ["tasks", "create", "--id", "20260816-rtry", title];
        let shorter = fixture.tiber(&shorter_invocation);
        assert_success(&shorter);
        let shorter_id = created_task_id(&shorter);
        assert_eq!(shorter_id, "20260816-rtry-exact-stable-prefix-2");
        let after_shorter = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&shorter_invocation);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("already created {shorter_id}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_shorter,
            "only the exact shorter stable identity may reconcile without publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_does_not_reconcile_a_noncanonical_numeric_suffix() {
        let fixture = TaskFixture::signed_noncanonical_retry_suffix_history().await;

        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "20260816-rtry",
            "Exact stable prefix",
        ]);

        assert_success(&created);
        assert_eq!(
            created_task_id(&created),
            "20260816-rtry-exact-stable-prefix"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_board_order_from_a_task_stream() {
        let fixture = TaskFixture::signed_foreign_stream_board_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "a board order on a task stream must report the stable stream-authority code"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream board order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_priority_order_from_a_task_stream() {
        let fixture = TaskFixture::signed_foreign_stream_priority_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "a priority order on a task stream must report the stable creation-history code"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream priority order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_commit_closure_order_from_a_task_stream() {
        let fixture = TaskFixture::signed_foreign_stream_commit_closure_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "a commit-closure order on a task stream must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream commit-closure order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_validation_repair_from_a_task_stream() {
        let fixture = TaskFixture::signed_foreign_stream_validation_repair_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "a validation repair on a task stream must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a foreign-stream validation repair must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_a_validation_repair_with_mismatched_order_stream() {
        let fixture = TaskFixture::signed_mismatched_validation_repair_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "a mismatched embedded order stream must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a mismatched validation-repair order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_duplicate_priority_order_without_publication() {
        let fixture = TaskFixture::signed_duplicate_priority_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "duplicate priority order must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "duplicate priority order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_duplicate_commit_closure_order_without_publication() {
        let fixture = TaskFixture::signed_duplicate_commit_closure_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "duplicate commit-closure order must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "duplicate commit-closure order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_duplicate_validation_repair_order_without_publication() {
        let fixture = TaskFixture::signed_duplicate_validation_repair_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "duplicate repaired order must report malformed creation history"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "duplicate repaired order must not authorize publication"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_duplicate_creation_history_without_publication() {
        let fixture = TaskFixture::signed_duplicate_creation_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert_eq!(
            String::from_utf8_lossy(&created.stderr),
            "tasks_command_duplicate_task_creation: the authoritative Tiber task history could not decide that task change\n"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "malformed creation history must not authorize another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_a_creation_fact_from_a_foreign_stream() {
        let fixture = TaskFixture::signed_foreign_stream_creation_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_target_task_fact_unexpected_stream:"),
            "a foreign-stream task fact must report the stable stream-authority error code"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "foreign-stream task history must not authorize another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_a_removal_for_an_absent_task() {
        let fixture = TaskFixture::signed_absent_task_removal_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr).starts_with("tasks_command_task_missing:"),
            "an invalid removal must report the stable missing-task error code"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "an invalid removal must not authorize another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_a_removal_from_a_foreign_task_stream() {
        let fixture = TaskFixture::signed_foreign_stream_removal_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_target_task_fact_unexpected_stream:")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "foreign-stream removal must not authorize another revision"
        );
    }

    #[tokio::test]
    async fn tasks_create_does_not_republish_a_removed_task_in_the_board_order() {
        let fixture = TaskFixture::signed_removed_task_history().await;

        let created = fixture.tiber(&["tasks", "create", "Replacement board member"]);

        assert_success(&created);
        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert!(!task_row_ids(&listed).contains(&TASK_ID));
        assert_eq!(task_row_ids(&listed), vec![created_task_id(&created)]);
    }

    #[tokio::test]
    async fn tasks_create_rejects_an_id_that_cannot_form_a_task_stream() {
        let fixture = TaskFixture::signed_paged_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "--id", "*", "Poisoned task"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_invalid_task_stream:")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before
        );
    }

    #[tokio::test]
    async fn tasks_create_accepts_valid_remove_then_recreate_history() {
        let fixture = TaskFixture::signed_recreated_duplicate_subtask_history().await;

        let created = fixture.tiber(&["tasks", "create", "After valid recreation"]);

        assert_success(&created);
        let created_id = created_task_id(&created);
        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert!(
            task_row_ids(&listed).contains(&created_id.as_str()),
            "valid remove-and-recreate history must retain creation authority for a new task"
        );
    }

    #[tokio::test]
    async fn tasks_create_rejects_duplicate_strict_order_without_publication() {
        let fixture = TaskFixture::signed_duplicate_board_order_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let created = fixture.tiber(&["tasks", "create", "Must not be created"]);

        assert!(!created.status.success());
        assert!(
            String::from_utf8_lossy(&created.stderr)
                .starts_with("tasks_command_task_creation_malformed_history:"),
            "duplicate strict order must report the stable creation-history error code"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "duplicate strict order must not authorize another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_create_reports_an_exact_retry_after_ambiguous_remote_confirmation() {
        let fixture = TaskFixture::signed_paged_history().await;
        let remote = fixture._directory.path().join("remote.git");
        git(
            fixture._directory.path(),
            [
                "clone",
                "--bare",
                fixture.repository.to_str().expect("fixture path is UTF-8"),
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        git(
            &fixture.repository,
            [
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path is UTF-8"),
            ],
        );
        let shell_sentinel = fixture._directory.path().join("unsafe-retry-ran");
        let title = format!("Fix'; touch {}", shell_sentinel.display());
        let first = fixture.tiber(&["tasks", "create", title.as_str()]);
        assert_success(&first);
        let first_task_id = created_task_id(&first);
        let base = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        let commands = fixture._directory.path().join("ambiguous-commands");
        fs::create_dir_all(&commands).expect("wrapper directory should be created");
        let wrapper = commands.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
operation=
for argument in "$@"; do
  if [ "$argument" = "push" ]; then operation=push; fi
  if [ "$argument" = "ls-remote" ]; then operation=ls-remote; fi
done
if [ "$operation" = "ls-remote" ] && [ -f "$TIBER_PUSH_MARKER" ]; then
  printf '%s\trefs/heads/tiber\n' "$TIBER_STALE_HEAD"
  exit 0
fi
if [ "$operation" = "push" ]; then
  "$TIBER_REAL_GIT" "$@"
  status=$?
  if [ "$status" -eq 0 ]; then : > "$TIBER_PUSH_MARKER"; fi
  exit "$status"
fi
exec "$TIBER_REAL_GIT" "$@"
"#,
        )
        .expect("Git wrapper should be written");
        let mut permissions = fs::metadata(&wrapper)
            .expect("Git wrapper metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).expect("Git wrapper should be executable");
        let real_git = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("Git discovery should execute");
        assert!(real_git.status.success());
        let real_git = String::from_utf8(real_git.stdout)
            .expect("Git path is UTF-8")
            .trim()
            .to_owned();
        let path = format!(
            "{}:{}",
            commands.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let marker = fixture._directory.path().join("push-completed");
        let invocation = ["tasks", "create", title.as_str()];

        let ambiguous = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(invocation)
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .env("TIBER_REAL_GIT", real_git)
            .env("TIBER_PUSH_MARKER", &marker)
            .env("TIBER_STALE_HEAD", base)
            .output()
            .expect("Tiber CLI should execute");

        assert!(!ambiguous.status.success());
        let diagnostic = String::from_utf8_lossy(&ambiguous.stderr);
        let remote_after_ambiguous = git_output(&remote, ["rev-parse", TIBER_REF]);
        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        let durable_task_id = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .find_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let task_id = fields.next()?;
                let _status = fields.next()?;
                let listed_title = fields.next()?;
                (listed_title == title && task_id != first_task_id).then(|| task_id.to_owned())
            })
            .expect("the ambiguous publication must be visible through the public task list");

        let retry_command = diagnostic
            .strip_prefix(
                "tiber_store_publication_ambiguous: task creation may already be durable; retry exactly: `",
            )
            .and_then(|text| text.strip_suffix("`\n"))
            .expect("the diagnostic must expose one exact retry command");
        let tiber_directory = Path::new(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("the test binary has a parent directory");
        let retry_path = format!(
            "{}:{}",
            tiber_directory.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let reconciled = Command::new("sh")
            .args(["-c", retry_command])
            .current_dir(&fixture.repository)
            .env("PATH", retry_path)
            .output()
            .expect("the displayed retry command should execute");
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout),
            format!("already created {durable_task_id}\n")
        );
        assert_ne!(durable_task_id, first_task_id);
        assert!(
            !shell_sentinel.exists(),
            "the displayed retry must preserve the title as one inert shell argument"
        );
        assert_eq!(
            git_output(&remote, ["rev-parse", TIBER_REF]),
            remote_after_ambiguous,
            "reconciliation must not publish a second task"
        );
    }

    #[tokio::test]
    async fn tasks_start_activates_the_next_task_and_is_idempotent() {
        let fixture = TaskFixture::signed_ordered_history().await;

        let activated = fixture.tiber(&["tasks", "start", SECOND_PRIORITY_TASK_ID]);

        assert_success(&activated);
        let after_activation = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        assert_eq!(
            String::from_utf8_lossy(&activated.stdout),
            format!("activated {SECOND_PRIORITY_TASK_ID} at {after_activation}\n"),
            "a successful bounded activation must name the exact confirmed signed authority revision"
        );

        let repeated = fixture.tiber(&["tasks", "start", SECOND_PRIORITY_TASK_ID]);

        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("{SECOND_PRIORITY_TASK_ID} already in progress\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_activation,
            "a retry of the sole active target must not publish another authority revision"
        );

        let shown = fixture.tiber(&["tasks", "show", SECOND_PRIORITY_TASK_ID]);
        assert_success(&shown);
        assert!(
            String::from_utf8_lossy(&shown.stdout).contains("status: in-progress\n"),
            "the native activation must be visible through the public task projection"
        );
    }

    #[tokio::test]
    async fn tasks_start_rejects_non_next_and_another_active_task_without_publishing() {
        let fixture = TaskFixture::signed_ordered_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let non_next = fixture.tiber(&["tasks", "start", FIRST_PRIORITY_TASK_ID]);

        assert!(!non_next.status.success());
        assert!(non_next.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&non_next.stderr),
            format!(
                "tasks_command_task_activation_not_next_eligible: task `{FIRST_PRIORITY_TASK_ID}` is not the next eligible task; run `tiber tasks start {SECOND_PRIORITY_TASK_ID}` or inspect `tiber tasks next` before retrying\n"
            ),
            "a priority refusal must name the strict next task and a safe native recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a non-next activation refusal must not publish another authority revision"
        );

        let activated = fixture.tiber(&["tasks", "start", SECOND_PRIORITY_TASK_ID]);
        assert_success(&activated);
        let after_activation = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let another = fixture.tiber(&["tasks", "start", FIRST_PRIORITY_TASK_ID]);

        assert!(!another.status.success());
        assert!(another.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&another.stderr),
            format!(
                "tasks_command_task_activation_active_task: task `{SECOND_PRIORITY_TASK_ID}` is already active; continue it or inspect `tiber tasks next` before starting another task\n"
            ),
            "an active-task refusal must name the sole active task and a safe native recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_activation,
            "a second activation refusal must not publish another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_start_rejects_a_blocked_task_without_publishing() {
        let fixture = TaskFixture::signed_blocked_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "start", BLOCKED_TASK_ID]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_activation_blocked: task `{BLOCKED_TASK_ID}` is blocked by `{BLOCKER_TASK_ID}`; reload with `tiber tasks show {BLOCKED_TASK_ID}` before retrying\n"
            ),
            "a blocked activation refusal must name the first unresolved blocker and a safe native recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a blocked activation refusal must not publish another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_start_rejects_a_done_task_retained_in_board_order_without_publishing() {
        let fixture = TaskFixture::signed_paged_history_with_status("done").await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "start", TASK_ID]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_activation_not_backlog: task `{TASK_ID}` is currently `done`, not `backlog`; reload with `tiber tasks show {TASK_ID}` before retrying\n"
            ),
            "a non-backlog activation refusal must name the exact terminal status and safe recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a non-backlog activation refusal must not publish another authority revision"
        );
    }

    #[tokio::test]
    async fn tasks_acceptance_check_publishes_once_and_is_idempotent() {
        let fixture = TaskFixture::signed_acceptance_history().await;

        let checked = fixture.tiber(&["tasks", "acceptance", "check", TASK_ID, "1"]);
        assert_success(&checked);
        assert!(
            String::from_utf8_lossy(&checked.stdout)
                .starts_with(&format!("checked acceptance 1 for {TASK_ID} at "))
        );

        let repeated = fixture.tiber(&["tasks", "acceptance", "check", TASK_ID, "1"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("acceptance 1 already checked for {TASK_ID}\n")
        );
    }

    #[tokio::test]
    async fn tasks_subtask_repair_corrects_only_the_selected_duplicate_occurrence() {
        let fixture = TaskFixture::signed_duplicate_subtask_history().await;

        let before = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&before);
        let before_text = String::from_utf8_lossy(&before.stdout);
        assert!(
            before_text
                .contains("4. [ ] s4 Protect native review orchestration \u{2014} after: s3")
        );
        assert!(before_text.contains("5. [ ] s4 Disable legacy eval execution \u{2014} after: s4"));

        let repaired = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", "s5"]);
        assert_success(&repaired);
        assert!(
            String::from_utf8_lossy(&repaired.stdout).starts_with(&format!(
                "corrected duplicate subtask 5 for {TASK_ID}: s4 -> s5 at "
            ))
        );

        let repeated = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", "s5"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("duplicate subtask 5 already corrected for {TASK_ID}\n")
        );

        let after = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&after);
        let after_text = String::from_utf8_lossy(&after.stdout);
        assert!(
            after_text.contains("4. [ ] s4 Protect native review orchestration \u{2014} after: s3")
        );
        assert!(after_text.contains("5. [ ] s5 Disable legacy eval execution \u{2014} after: s4"));
    }

    #[tokio::test]
    async fn tasks_subtask_check_addresses_the_duplicate_id_by_occurrence_before_repair() {
        let fixture = TaskFixture::signed_active_duplicate_subtask_history().await;

        let before = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&before);
        let before_text = String::from_utf8_lossy(&before.stdout);
        assert!(
            before_text
                .contains("4. [ ] s4 Protect native review orchestration \u{2014} after: s3")
        );
        assert!(before_text.contains("5. [ ] s4 Disable legacy eval execution \u{2014} after: s4"));

        let checked = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);
        assert_success(&checked);
        assert!(
            String::from_utf8_lossy(&checked.stdout)
                .starts_with(&format!("checked subtask 5 for {TASK_ID} at "))
        );
        let after_first_check = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("subtask 5 already checked for {TASK_ID}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first_check,
            "an idempotent duplicate-occurrence retry must not publish another board update"
        );

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        assert!(
            shown_text.contains("4. [ ] s4 Protect native review orchestration \u{2014} after: s3")
        );
        assert!(shown_text.contains("5. [x] s4 Disable legacy eval execution \u{2014} after: s4"));
    }

    #[tokio::test]
    async fn tasks_subtask_check_addresses_the_repaired_occurrence_and_is_idempotent() {
        let fixture = TaskFixture::signed_active_duplicate_subtask_history().await;

        let repaired = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", "s5"]);
        assert_success(&repaired);

        let checked = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);
        assert_success(&checked);
        assert!(
            String::from_utf8_lossy(&checked.stdout)
                .starts_with(&format!("checked subtask 5 for {TASK_ID} at "))
        );
        let after_first_check = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("subtask 5 already checked for {TASK_ID}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_first_check,
            "an idempotent occurrence-check retry must not publish another board update"
        );

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        assert!(
            shown_text.contains("4. [ ] s4 Protect native review orchestration \u{2014} after: s3")
        );
        assert!(shown_text.contains("5. [x] s5 Disable legacy eval execution \u{2014} after: s4"));
    }

    #[tokio::test]
    async fn tasks_subtask_check_rejects_a_missing_occurrence_without_publishing() {
        let fixture = TaskFixture::signed_active_duplicate_subtask_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "6"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_subtask_occurrence_missing: subtask occurrence 6 does not exist for task `{TASK_ID}`; reload with `tiber tasks show {TASK_ID}` before choosing an occurrence\n"
            ),
            "a valid but absent occurrence must name its one-based position and the safe recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected occurrence check must not publish a board update"
        );
    }

    #[tokio::test]
    async fn tasks_subtask_check_rejects_a_non_active_task_with_a_safe_status_diagnostic() {
        let fixture = TaskFixture::signed_duplicate_subtask_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "1"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_not_in_progress: task `{TASK_ID}` is currently `backlog`, not `in-progress`; reload with `tiber tasks show {TASK_ID}` before retrying\n"
            ),
            "a lifecycle rejection must name the durable status and a safe task-specific recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected occurrence check must not publish a board update"
        );
    }

    #[tokio::test]
    async fn tasks_done_transition_rejects_a_non_active_task_with_a_safe_status_diagnostic() {
        let fixture = TaskFixture::signed_duplicate_subtask_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_task_not_in_progress: task `{TASK_ID}` is currently `backlog`, not `in-progress`; reload with `tiber tasks show {TASK_ID}` before retrying\n"
            ),
            "a lifecycle rejection must name the durable status and a safe task-specific recovery command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected terminal transition must not publish a board update"
        );
    }

    #[tokio::test]
    async fn tasks_done_transition_completes_the_ready_active_task_and_is_idempotent() {
        let fixture = TaskFixture::signed_active_completion_ready_duplicate_subtask_history().await;

        let repaired = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", "s5"]);
        assert_success(&repaired);
        for occurrence in ["4", "5"] {
            let checked = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, occurrence]);
            assert_success(&checked);
        }

        let completed = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);
        assert_success(&completed);
        assert!(
            String::from_utf8_lossy(&completed.stdout)
                .starts_with(&format!("transitioned {TASK_ID} to done at "))
        );
        let after_completion = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("{TASK_ID} already done\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_completion,
            "an idempotent completion retry must not publish another board update"
        );

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        assert!(String::from_utf8_lossy(&shown.stdout).contains("status: done\n"));

        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert!(
            listed.stdout.is_empty(),
            "completed work must leave the open board"
        );
    }

    #[tokio::test]
    async fn tasks_subtask_check_remains_idempotent_after_task_completion() {
        let fixture = TaskFixture::signed_active_completion_ready_duplicate_subtask_history().await;

        let repaired = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", "s5"]);
        assert_success(&repaired);
        let prerequisite = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "4"]);
        assert_success(&prerequisite);
        let checked = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);
        assert_success(&checked);

        let completed = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);
        assert_success(&completed);
        let after_completion = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let repeated = fixture.tiber(&["tasks", "subtask", "check", TASK_ID, "5"]);

        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("subtask 5 already checked for {TASK_ID}\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_completion,
            "an idempotent terminal occurrence-check retry must not publish another board update"
        );
    }

    #[tokio::test]
    async fn tasks_done_transition_rejects_unchecked_requirements_without_publishing() {
        let fixture = TaskFixture::signed_active_duplicate_subtask_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_subtask_occurrence_unchecked: task `{TASK_ID}` cannot transition to done because subtask 1 is unchecked; run `tiber tasks subtask check {TASK_ID} 1` before retrying\n"
            ),
            "terminal completion must name the first unchecked one-based subtask and its safe check command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected terminal completion must not publish a board update"
        );
        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        assert!(String::from_utf8_lossy(&shown.stdout).contains("status: in-progress\n"));
    }

    #[tokio::test]
    async fn tasks_done_transition_names_an_unchecked_acceptance_item_without_publishing() {
        let fixture = TaskFixture::signed_active_acceptance_history().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let output = fixture.tiber(&["tasks", "transition", TASK_ID, "done"]);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_command_acceptance_item_unchecked: task `{TASK_ID}` cannot transition to done because acceptance item 1 is unchecked; run `tiber tasks acceptance check {TASK_ID} 1` before retrying\n"
            ),
            "terminal completion must name the first unchecked one-based acceptance item and its safe check command"
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            before,
            "a rejected terminal completion must not publish a board update"
        );
    }

    #[tokio::test]
    async fn tasks_done_transition_repairs_all_stale_board_entries_without_retransitioning() {
        let fixture = TaskFixture::signed_done_task_with_duplicate_stale_board_entries().await;
        let before = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);

        let reconciled = fixture.tiber(&["tasks", "transition", CLOSED_TASK_ID, "done"]);

        assert_success(&reconciled);
        assert!(
            String::from_utf8_lossy(&reconciled.stdout).starts_with(&format!(
                "reconciled completed task {CLOSED_TASK_ID} board entries at "
            )),
            "a done task with stale order entries must not be reported as a new transition"
        );
        let after_reconciliation = git_output(&fixture.repository, ["rev-parse", TIBER_REF]);
        assert_ne!(
            after_reconciliation, before,
            "the order-only repair must publish one exact new authority revision"
        );

        let listed = fixture.tiber(&["tasks", "list"]);
        assert_success(&listed);
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout),
            format!(
                "{FIRST_PRIORITY_TASK_ID}\tbacklog\tFirst priority task revised by board\n{SECOND_PRIORITY_TASK_ID}\tbacklog\tSecond priority task\n"
            ),
            "all target entries must be removed while non-target board order is retained"
        );

        let repeated = fixture.tiber(&["tasks", "transition", CLOSED_TASK_ID, "done"]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("{CLOSED_TASK_ID} already done\n")
        );
        assert_eq!(
            git_output(&fixture.repository, ["rev-parse", TIBER_REF]),
            after_reconciliation,
            "a reconciled completion retry must not publish another board update"
        );
    }

    #[tokio::test]
    async fn tasks_subtask_repair_canonicalizes_a_whitespace_padded_replacement_for_retry() {
        let fixture = TaskFixture::signed_duplicate_subtask_history().await;

        let repaired =
            fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", " s5 "]);
        assert_success(&repaired);
        assert!(
            String::from_utf8_lossy(&repaired.stdout).starts_with(&format!(
                "corrected duplicate subtask 5 for {TASK_ID}: s4 -> s5 at "
            ))
        );

        let repeated =
            fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "5", " s5 "]);
        assert_success(&repeated);
        assert_eq!(
            String::from_utf8_lossy(&repeated.stdout),
            format!("duplicate subtask 5 already corrected for {TASK_ID}\n")
        );
    }

    #[tokio::test]
    async fn tasks_subtask_repair_uses_only_the_current_task_lifetime_preimage() {
        let fixture = TaskFixture::signed_recreated_duplicate_subtask_history().await;

        let repaired = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "2", "s5"]);

        assert_success(&repaired);
        assert!(
            String::from_utf8_lossy(&repaired.stdout).starts_with(&format!(
                "corrected duplicate subtask 2 for {TASK_ID}: s4 -> s5 at "
            ))
        );

        let shown = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&shown);
        let shown_text = String::from_utf8_lossy(&shown.stdout);
        assert!(shown_text.contains("1. [ ] s4 Recreated first duplicate"));
        assert!(shown_text.contains("2. [ ] s5 Recreated second duplicate"));
    }

    #[tokio::test]
    async fn tasks_subtask_repair_failure_uses_neutral_task_change_language() {
        let fixture = TaskFixture::signed_paged_history().await;

        let output = fixture.tiber(&["tasks", "subtask", "repair-duplicate", TASK_ID, "1", "s5"]);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with(
            "tasks_command_subtask_occurrence_missing: subtask occurrence 1 does not exist for task `20260812-a111-paged-task-history`; reload with `tiber tasks show 20260812-a111-paged-task-history` before choosing an occurrence"
        ));
        assert!(
            !stderr.contains("acceptance change"),
            "a duplicate-subtask error must not name the unrelated acceptance command: {stderr}"
        );
    }

    #[tokio::test]
    async fn tasks_show_displays_ordered_acceptance_with_current_checked_state() {
        let fixture = TaskFixture::signed_acceptance_history().await;

        let before_check = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&before_check);
        assert_eq!(
            String::from_utf8_lossy(&before_check.stdout),
            format!(
                "id: {TASK_ID}\nstatus: backlog\ntitle: Paged task revision {}\ncommitted-at: 2026-08-12T00:00:00Z\nsummary: History page {}.\ncontext: Read the complete EventCore history.\ntags: native\nacceptance:\n1. [ ] first native acceptance\n2. [ ] second native acceptance\n",
                PAGE_SPANNING_UPDATES - 1,
                PAGE_SPANNING_UPDATES - 1
            )
        );

        let checked = fixture.tiber(&["tasks", "acceptance", "check", TASK_ID, "1"]);
        assert_success(&checked);

        let after_check = fixture.tiber(&["tasks", "show", TASK_ID]);
        assert_success(&after_check);
        assert_eq!(
            String::from_utf8_lossy(&after_check.stdout),
            format!(
                "id: {TASK_ID}\nstatus: backlog\ntitle: Paged task revision {}\ncommitted-at: 2026-08-12T00:00:00Z\nsummary: History page {}.\ncontext: Read the complete EventCore history.\ntags: native\nacceptance:\n1. [x] first native acceptance\n2. [ ] second native acceptance\n",
                PAGE_SPANNING_UPDATES - 1,
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_next_continues_the_sole_active_task() {
        let fixture = TaskFixture::signed_active_paged_history().await;

        let output = fixture.tiber(&["tasks", "next"]);

        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!(
                "{TASK_ID}\tin-progress\tPaged task revision {}\n",
                PAGE_SPANNING_UPDATES - 1
            )
        );
    }

    #[tokio::test]
    async fn tasks_list_replays_cross_stream_facts_in_transaction_order_despite_shuffled_cursor() {
        let fixture = TaskFixture::signed_ordered_history().await;

        let output = fixture.tiber(&["tasks", "list"]);

        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!(
                "{SECOND_PRIORITY_TASK_ID}\tbacklog\tSecond priority task\n{FIRST_PRIORITY_TASK_ID}\tbacklog\tFirst priority task revised by board\n"
            )
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains(CLOSED_TASK_ID),
            "the default open-board view must omit closed task identities"
        );
    }

    #[tokio::test]
    async fn tasks_show_reports_an_actionable_missing_task_reference() {
        let fixture = TaskFixture::signed_ordered_history().await;

        let output = fixture.tiber(&["tasks", "show", "not-a-task"]);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tasks_task_reference_missing: no task matches reference `not-a-task`\n"
        );
    }

    #[tokio::test]
    async fn tasks_show_reports_matching_full_ids_for_an_ambiguous_reference() {
        let fixture = TaskFixture::signed_ordered_history().await;

        let output = fixture.tiber(&["tasks", "show", "first-priority"]);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!(
                "tasks_task_reference_ambiguous: task reference `first-priority` is ambiguous; matching task IDs: {FIRST_PRIORITY_TASK_ID}, {SECOND_PRIORITY_TASK_ID}\n"
            )
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the isolated non-Git fixture must fail fast before it can assert the stable CLI error boundary"
    )]
    fn tasks_store_failure_keeps_git_stderr_sanitized() {
        let directory = TempDir::new().expect("non-Git fixture directory should be created");
        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "list"])
            .current_dir(directory.path())
            .output()
            .expect("Tiber CLI should execute against a non-Git directory");

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with("tiber_git_resolve_tiber_ref_failed:"));
        assert!(
            !stderr.contains("fatal:"),
            "Git implementation stderr must not cross the CLI boundary: {stderr}"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while preparing the environment that proves invalid input skips Git"
    )]
    fn invalid_task_references_fail_before_any_git_command() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");

        for (index, (operation, reference)) in [
            ("show", "../x"),
            ("show", ""),
            ("show", "task.md"),
            ("start", "../x"),
            ("start", ""),
            ("start", "task.md"),
        ]
        .into_iter()
        .enumerate()
        {
            let marker = directory.path().join(format!("git-was-run-{index}"));
            let usage_exit_code: i32 = 2;
            let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(["tasks", operation, reference])
                .current_dir(directory.path())
                .env("PATH", &command_directory)
                .env("TIBER_GIT_SENTINEL", &marker)
                .output()
                .expect("Tiber CLI should execute");

            assert_eq!(output.status.code(), Some(usage_exit_code));
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .starts_with("tasks_invalid_task_reference:"),
                "invalid task references must report the stable usage code"
            );
            assert!(
                !marker.exists(),
                "invalid {operation} reference {reference:?} must not invoke Git"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while proving malformed native acceptance input cannot trigger a Git authority read"
    )]
    fn invalid_acceptance_commands_fail_before_any_git_command() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");

        for (index, (reference, acceptance_index, code)) in [
            ("../x", "1", "tasks_invalid_task_reference"),
            (TASK_ID, "0", "tasks_invalid_acceptance_index"),
        ]
        .into_iter()
        .enumerate()
        {
            let marker = directory.path().join(format!("git-was-run-{index}"));
            let usage_exit_code: i32 = 2;
            let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(["tasks", "acceptance", "check", reference, acceptance_index])
                .current_dir(directory.path())
                .env("PATH", &command_directory)
                .env("TIBER_GIT_SENTINEL", &marker)
                .output()
                .expect("Tiber CLI should execute");

            assert_eq!(output.status.code(), Some(usage_exit_code));
            assert!(
                String::from_utf8_lossy(&output.stderr).starts_with(&format!("{code}:")),
                "invalid acceptance input must report the stable usage code"
            );
            assert!(
                !marker.exists(),
                "invalid acceptance command must not invoke Git"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while proving malformed duplicate-repair input cannot trigger a Git authority read"
    )]
    fn invalid_subtask_repair_replacement_ids_fail_before_any_git_command() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");

        for (index, replacement_id) in ["", "s5\u{0007}"].into_iter().enumerate() {
            let marker = directory.path().join(format!("git-was-run-{index}"));
            let usage_exit_code: i32 = 2;
            let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args([
                    "tasks",
                    "subtask",
                    "repair-duplicate",
                    TASK_ID,
                    "5",
                    replacement_id,
                ])
                .current_dir(directory.path())
                .env("PATH", &command_directory)
                .env("TIBER_GIT_SENTINEL", &marker)
                .output()
                .expect("Tiber CLI should execute");

            assert_eq!(output.status.code(), Some(usage_exit_code));
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .starts_with("tasks_invalid_subtask_replacement_id:"),
                "invalid replacement input must report the stable usage code"
            );
            assert!(
                !marker.exists(),
                "invalid replacement ID {replacement_id:?} must not invoke Git"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while proving malformed bounded-completion input cannot trigger a Git authority read"
    )]
    fn invalid_bounded_completion_commands_fail_before_any_git_command() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");
        let usage_exit_code: i32 = 2;

        let invalid_occurrence = directory.path().join("git-was-run-invalid-occurrence");
        let occurrence_output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "subtask", "check", TASK_ID, "0"])
            .current_dir(directory.path())
            .env("PATH", &command_directory)
            .env("TIBER_GIT_SENTINEL", &invalid_occurrence)
            .output()
            .expect("Tiber CLI should execute");
        assert_eq!(occurrence_output.status.code(), Some(usage_exit_code));
        assert!(
            String::from_utf8_lossy(&occurrence_output.stderr)
                .starts_with("tasks_invalid_subtask_occurrence:"),
            "the malformed occurrence must report its semantic usage error"
        );
        assert!(
            !invalid_occurrence.exists(),
            "a malformed occurrence must not invoke Git"
        );

        let invalid_status = directory.path().join("git-was-run-invalid-transition");
        let transition_output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "transition", TASK_ID, "backlog"])
            .current_dir(directory.path())
            .env("PATH", &command_directory)
            .env("TIBER_GIT_SENTINEL", &invalid_status)
            .output()
            .expect("Tiber CLI should execute");
        assert_eq!(transition_output.status.code(), Some(usage_exit_code));
        assert!(
            String::from_utf8_lossy(&transition_output.stderr)
                .starts_with("tiber_tasks_invalid_arguments:"),
            "only the literal done transition is accepted"
        );
        assert!(
            !invalid_status.exists(),
            "a non-done transition must not invoke Git"
        );

        let invalid_reference = directory
            .path()
            .join("git-was-run-invalid-transition-reference");
        let reference_output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "transition", "../not-a-task", "done"])
            .current_dir(directory.path())
            .env("PATH", &command_directory)
            .env("TIBER_GIT_SENTINEL", &invalid_reference)
            .output()
            .expect("Tiber CLI should execute");
        assert_eq!(reference_output.status.code(), Some(usage_exit_code));
        assert!(
            String::from_utf8_lossy(&reference_output.stderr)
                .starts_with("tasks_invalid_task_reference:"),
            "a malformed transition reference must report its semantic usage error"
        );
        assert!(
            !invalid_reference.exists(),
            "a malformed transition reference must not invoke Git"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while proving incomplete stable-creation syntax cannot trigger a Git authority read"
    )]
    fn tasks_create_rejects_a_bare_reserved_id_flag_before_git() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");
        let sentinel = directory.path().join("git-was-run");

        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "create", "--id"])
            .current_dir(directory.path())
            .env("PATH", &command_directory)
            .env("TIBER_GIT_SENTINEL", &sentinel)
            .output()
            .expect("Tiber CLI should execute");

        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).starts_with("tiber_tasks_invalid_arguments:"),
            "the reserved stable-ID flag requires both its prefix and title operands"
        );
        assert!(
            !sentinel.exists(),
            "incomplete stable-creation syntax must not invoke Git"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the sentinel fixture must fail fast while proving a stable prefix without a title cannot trigger a Git authority read"
    )]
    fn tasks_create_rejects_a_stable_id_prefix_without_a_title_before_git() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let command_directory = directory.path().join("commands");
        fs::create_dir_all(&command_directory)
            .expect("sentinel command directory should be created");
        let git_sentinel = command_directory.join("git");
        fs::write(
            &git_sentinel,
            "#!/bin/sh\n: > \"$TIBER_GIT_SENTINEL\"\nexit 99\n",
        )
        .expect("sentinel Git command should be written");
        let mut permissions = fs::metadata(&git_sentinel)
            .expect("sentinel Git command metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_sentinel, permissions)
            .expect("sentinel Git command should be executable");
        let sentinel = directory.path().join("git-was-run");

        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "create", "--id", "20260816-rtry"])
            .current_dir(directory.path())
            .env("PATH", &command_directory)
            .env("TIBER_GIT_SENTINEL", &sentinel)
            .output()
            .expect("Tiber CLI should execute");

        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).starts_with("tiber_tasks_invalid_arguments:"),
            "the reserved stable-ID prefix requires a title operand"
        );
        assert!(
            !sentinel.exists(),
            "a stable prefix without a title must not invoke Git"
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the command shell must start successfully before its public usage output can be asserted"
    )]
    fn task_help_advertises_native_creation_and_bounded_lifecycle_grammar() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let usage_exit_code: i32 = 2;
        let nested = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["tasks", "subtask"])
            .current_dir(directory.path())
            .output()
            .expect("Tiber CLI should execute");
        assert_eq!(nested.status.code(), Some(usage_exit_code));
        assert!(
            String::from_utf8_lossy(&nested.stderr)
                .contains("create [--id <stable-prefix>] <title>")
        );
        assert!(
            String::from_utf8_lossy(&nested.stderr)
                .contains("subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id>")
        );
        assert!(
            String::from_utf8_lossy(&nested.stderr)
                .contains("subtask check <ref> <one-based-occurrence>")
        );
        assert!(String::from_utf8_lossy(&nested.stderr).contains("start <ref>"));
        assert!(
            String::from_utf8_lossy(&nested.stderr).contains("transition <ref> <done|abandoned>")
        );

        let top_level = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .arg("unsupported")
            .current_dir(directory.path())
            .output()
            .expect("Tiber CLI should execute");
        assert_eq!(top_level.status.code(), Some(usage_exit_code));
        assert!(
            String::from_utf8_lossy(&top_level.stderr)
                .contains("subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id>")
        );
        assert!(
            String::from_utf8_lossy(&top_level.stderr)
                .contains("subtask check <ref> <one-based-occurrence>")
        );
        assert!(String::from_utf8_lossy(&top_level.stderr).contains("start <ref>"));
        assert!(
            String::from_utf8_lossy(&top_level.stderr)
                .contains("transition <ref> <done|abandoned>")
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the command shell must start successfully before its public help boundary can be asserted"
    )]
    fn explicit_help_flags_succeed_without_an_error_diagnostic() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let root_usage = "usage: tiber [app-server-probe <authority-surface.json> | auth <status|login|login-api-key|logout> | converse <prompt> | tasks <create [--id <stable-prefix>] <title> | update <ref> --title <title> --summary <summary> --context <context> | prioritize <ref> --before <ref> | link blocked-by <ref> <blocker-ref> | list [--status <backlog|in-progress|done|abandoned>] | show <ref> | search <query> | next | start <ref> | acceptance add <ref> <criterion> | acceptance check <ref> <one-based-index> | subtask check <ref> <one-based-occurrence> | subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id> | transition <ref> <done|abandoned>>]\n";
        let tasks_usage = "usage: tiber tasks <create [--id <stable-prefix>] <title> | update <ref> --title <title> --summary <summary> --context <context> | prioritize <ref> --before <ref> | link blocked-by <ref> <blocker-ref> | list [--status <backlog|in-progress|done|abandoned>] | show <ref> | search <query> | next | start <ref> | acceptance add <ref> <criterion> | acceptance check <ref> <one-based-index> | subtask check <ref> <one-based-occurrence> | subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id> | transition <ref> <done|abandoned>>\n";
        let usage_exit_code: i32 = 2;
        let cases: &[(&[&str], &str)] = &[
            (&["--help"], root_usage),
            (&["-h"], root_usage),
            (&["tasks", "--help"], tasks_usage),
            (&["tasks", "-h"], tasks_usage),
        ];

        for &(arguments, expected_stdout) in cases {
            let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(arguments)
                .current_dir(directory.path())
                .output()
                .expect("Tiber CLI should execute");

            assert!(
                output.status.success(),
                "explicit help must be successful for {arguments:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).is_empty(),
                "explicit help must not emit an error diagnostic for {arguments:?}"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                stdout, *expected_stdout,
                "unexpected help for {arguments:?}"
            );
        }

        let invalid_cases: &[(&[&str], &str)] = &[
            (
                &["--help", "unexpected"],
                "explicit help accepts no further arguments\nusage: tiber [",
            ),
            (
                &["tasks", "--help", "unexpected"],
                "tiber_tasks_unknown_subcommand: ",
            ),
            (
                &["tasks", "-h", "unexpected"],
                "tiber_tasks_unknown_subcommand: ",
            ),
        ];
        for &(arguments, expected_stderr_prefix) in invalid_cases {
            let invalid = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(arguments)
                .current_dir(directory.path())
                .output()
                .expect("Tiber CLI should execute");
            assert_eq!(invalid.status.code(), Some(usage_exit_code));
            assert!(String::from_utf8_lossy(&invalid.stdout).is_empty());
            assert!(
                String::from_utf8_lossy(&invalid.stderr).starts_with(expected_stderr_prefix),
                "unexpected invalid-help diagnostic for {arguments:?}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the process-level fixture must fail fast if its deliberately removed working directory cannot be prepared"
    )]
    fn nested_help_does_not_require_a_resolvable_current_directory() {
        let directory = TempDir::new().expect("fixture directory should be created");
        let removed_current_directory = directory.path().join("removed-current-directory");
        fs::create_dir_all(&removed_current_directory)
            .expect("removed-current-directory fixture should be created");
        let output = Command::new("sh")
            .args([
                "-c",
                "cd \"$1\" && rmdir \"$1\" && exec \"$2\" tasks --help",
                "tiber-help-fixture",
            ])
            .arg(&removed_current_directory)
            .arg(env!("CARGO_BIN_EXE_tiber"))
            .current_dir(directory.path())
            .output()
            .expect("Tiber CLI should execute from its removed current directory");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "usage: tiber tasks <create [--id <stable-prefix>] <title> | update <ref> --title <title> --summary <summary> --context <context> | prioritize <ref> --before <ref> | link blocked-by <ref> <blocker-ref> | list [--status <backlog|in-progress|done|abandoned>] | show <ref> | search <query> | next | start <ref> | acceptance add <ref> <criterion> | acceptance check <ref> <one-based-index> | subtask check <ref> <one-based-occurrence> | subtask repair-duplicate <ref> <one-based-occurrence> <replacement-id> | transition <ref> <done|abandoned>>\n"
        );
    }

    #[expect(
        clippy::implicit_return,
        reason = "the static event fixture returns its decoded wire representation directly"
    )]
    fn task_created(stream_id: &StreamId, task_id: &str, title: &str, status: &str) -> TaskEvent {
        event(json!({
            "event": "task_created",
            "stream_id": stream_id.as_ref(),
            "task": {
                "acceptance": [],
                "blocked_by": [],
                "blocks": [],
                "claim": null,
                "committed_at": "2026-08-12T00:00:00Z",
                "context": "Read the complete EventCore history.",
                "notes": [],
                "pr_mr_status": null,
                "pr_mr_url": null,
                "status": status,
                "stem": task_id,
                "subtasks": [],
                "summary": "Initial task text.",
                "tags": ["native"],
                "title": title
            }
        }))
    }

    fn historical_task_removed(stream_id: &StreamId, task_id: &str) -> TaskEvent {
        event(json!({
            "event": "task_removed",
            "stream_id": stream_id.as_ref(),
            "stem": task_id
        }))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the acceptance fixture's one stable board-side mutation returns its decoded durable task fact directly"
    )]
    fn task_acceptance_added(stream_id: &StreamId, task_id: &str, text: &str) -> TaskEvent {
        event(json!({
            "event": "task_acceptance_added",
            "stream_id": stream_id.as_ref(),
            "stem": task_id,
            "item": {"checked": false, "text": text}
        }))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the duplicate-subtask fixture returns its decoded durable task fact directly"
    )]
    fn task_subtask_added(
        stream_id: &StreamId,
        task_id: &str,
        id: &str,
        title: &str,
        after: &[&str],
    ) -> TaskEvent {
        event(json!({
            "event": "task_subtask_added",
            "stream_id": stream_id.as_ref(),
            "stem": task_id,
            "subtask": {"after": after, "checked": false, "id": id, "title": title}
        }))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the completion-ready fixture returns its retained unique-ID check fact directly"
    )]
    fn task_subtask_checked(stream_id: &StreamId, task_id: &str, subtask_id: &str) -> TaskEvent {
        event(json!({
            "event": "task_subtask_checked",
            "stream_id": stream_id.as_ref(),
            "stem": task_id,
            "subtask_id": subtask_id,
            "checked": true
        }))
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the recreated-history fixture keeps its one signed-correction payload explicit and returns its decoded retained fact directly"
    )]
    fn task_subtask_id_corrected(
        stream_id: &StreamId,
        task_id: &str,
        index: usize,
        expected_id: &str,
        expected_title: &str,
        replacement_id: &str,
    ) -> TaskEvent {
        event(json!({
            "event": "task_subtask_id_corrected",
            "stream_id": stream_id.as_ref(),
            "stem": task_id,
            "index": index,
            "expected": {
                "after": [], "checked": false, "id": expected_id, "title": expected_title
            },
            "replacement_id": replacement_id
        }))
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the recreated-history fixture returns its decoded task-removal fact directly"
    )]
    fn task_removed(stream_id: &StreamId, task_id: &str) -> TaskEvent {
        event(json!({
            "event": "task_removed",
            "stream_id": stream_id.as_ref(),
            "stem": task_id
        }))
    }

    #[expect(
        clippy::implicit_return,
        reason = "the static board fixture returns its decoded wire representation directly"
    )]
    fn board_reordered(order: &[&str]) -> TaskEvent {
        event(json!({
            "event": "board_reordered",
            "stream_id": "tiber:board",
            "order": order
        }))
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the one ordering-sensitive board-side update fixture returns its decoded wire representation directly"
    )]
    fn board_task_details_updated(task_id: &str, title: &str) -> TaskEvent {
        event(json!({
            "event": "task_details_updated",
            "stream_id": "tiber:board",
            "stem": task_id,
            "title": title,
            "tags": ["native"],
            "summary": "Updated through the board stream.",
            "context": "Preserve global EventCore order across selected streams."
        }))
    }

    #[expect(
        clippy::single_call_fn,
        clippy::implicit_return,
        reason = "the paged-history loop uses one semantic event factory so its revision mutation remains explicit"
    )]
    fn task_details_updated(stream_id: &StreamId, revision: usize) -> TaskEvent {
        event(json!({
            "event": "task_details_updated",
            "stream_id": stream_id.as_ref(),
            "stem": TASK_ID,
            "title": format!("Paged task revision {revision}"),
            "tags": ["native"],
            "summary": format!("History page {revision}."),
            "context": "Read the complete EventCore history."
        }))
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the static fixture payload must deserialize before any black-box CLI assertion is meaningful"
    )]
    fn event(value: Value) -> TaskEvent {
        serde_json::from_value(value).expect("fixture event should match task history wire format")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture task identifiers are static and fail fast if their known-valid stream representation changes"
    )]
    fn task_stream(task_id: &str) -> StreamId {
        StreamId::try_new(format!("tiber:task:{task_id}"))
            .expect("fixture task stream should be valid")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the disposable signed repository fixture cannot exercise verification if its local files or ephemeral signing key cannot be prepared"
    )]
    fn signed_repository() -> (TempDir, PathBuf) {
        let directory = TempDir::new().expect("fixture directory should be created");
        let repository = directory.path().join("repository");
        let signing_key = directory.path().join("fixture-signing-key");
        git(
            directory.path(),
            ["init", repository.to_str().expect("fixture path is UTF-8")],
        );
        git(&repository, ["config", "user.name", "Tiber CLI Fixture"]);
        git(
            &repository,
            ["config", "user.email", "tiber-cli-fixture@example.invalid"],
        );
        let key_status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&signing_key)
            .status()
            .expect("fixture SSH signing key generation should start");
        assert!(
            key_status.success(),
            "fixture SSH signing key generation should succeed"
        );
        let allowed_signers = directory.path().join("allowed-signers");
        let public_key = fs::read_to_string(signing_key.with_extension("pub"))
            .expect("fixture SSH signing public key should be readable");
        fs::write(
            &allowed_signers,
            format!("tiber-cli-fixture@example.invalid {}", public_key.trim()),
        )
        .expect("fixture SSH allowed signers should be written");
        git(&repository, ["config", "gpg.format", "ssh"]);
        git(&repository, ["config", "commit.gpgsign", "true"]);
        git(
            &repository,
            [
                "config",
                "user.signingkey",
                signing_key
                    .to_str()
                    .expect("fixture signing key path is UTF-8"),
            ],
        );
        git(
            &repository,
            [
                "config",
                "gpg.ssh.allowedSignersFile",
                allowed_signers
                    .to_str()
                    .expect("fixture allowed signers path is UTF-8"),
            ],
        );
        (directory, repository)
    }

    fn commit_signed_tiber_history(repository: &Path) {
        git(repository, ["add", "eventstore/events"]);
        git(repository, ["commit", "-m", "fixture task history"]);
        let revision = git_output(repository, ["rev-parse", "HEAD"]);
        assert!(
            git_output(repository, ["cat-file", "commit", revision.as_str()])
                .contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
            "fixture history must retain an ephemeral SSH signature"
        );
        git(repository, ["update-ref", TIBER_REF, revision.as_str()]);
    }

    #[expect(
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the ordering regression fixture must fail fast while retaining a signed authority snapshot with an intentionally non-authoritative local EventCore cursor"
    )]
    fn commit_signed_tiber_history_with_shuffled_ingestion_cursor(repository: &Path) {
        let events_directory = repository.join("eventstore/events");
        let mut event_paths = fs::read_dir(&events_directory)
            .expect("fixture event directory should be readable")
            .map(|entry| {
                entry
                    .expect("fixture event directory entry should be readable")
                    .path()
            })
            .collect::<Vec<_>>();
        let event_path = event_paths
            .pop()
            .expect("ordering fixture should retain one transaction file");
        assert!(
            event_paths.is_empty(),
            "the ordering fixture must exercise envelope order inside one transaction rather than transaction filename order"
        );
        let event_ids = fs::read_to_string(event_path)
            .expect("fixture transaction file should be readable")
            .lines()
            .skip(1)
            .map(str::to_owned)
            .map(|line| {
                serde_json::from_str::<Value>(&line)
                    .expect("fixture event envelope should be valid JSON")
            })
            .map(|envelope| {
                envelope
                    .get("event_id")
                    .and_then(Value::as_str)
                    .expect("fixture event envelope should have an event ID")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let cursor_log = event_ids
            .iter()
            .rev()
            .enumerate()
            .map(|(index, event_id)| format!(r#"{{"event_id":"{event_id}","seq":{}}}"#, index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let index_directory = repository.join("eventstore/index");
        fs::create_dir_all(&index_directory).expect("fixture index directory should be created");
        fs::write(
            index_directory.join("ingestion.log"),
            format!("{cursor_log}\n"),
        )
        .expect("fixture shuffled ingestion cursor should be written");

        git(
            repository,
            [
                "add",
                "-f",
                "eventstore/events",
                "eventstore/index/ingestion.log",
            ],
        );
        git(
            repository,
            ["commit", "-m", "fixture task history with shuffled cursor"],
        );
        let revision = git_output(repository, ["rev-parse", "HEAD"]);
        assert!(
            git_output(repository, ["cat-file", "commit", revision.as_str()])
                .contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
            "fixture history must retain an ephemeral SSH signature"
        );
        git(repository, ["update-ref", TIBER_REF, revision.as_str()]);
    }

    fn assert_success(output: &Output) {
        assert!(
            output.status.success(),
            "Tiber task command should succeed; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn created_task_id(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout)
            .strip_prefix("created ")
            .and_then(|text| text.split_once(" at ").map(|(id, _revision)| id.to_owned()))
            .expect("successful creation names the durable task ID and authority revision")
    }

    fn task_row_ids(output: &Output) -> Vec<&str> {
        let rows = std::str::from_utf8(&output.stdout).expect("task rows are UTF-8");
        rows.lines()
            .map(|row| row.split('\t').next().expect("task row has an ID"))
            .collect()
    }

    #[expect(
        clippy::expect_used,
        reason = "fixture setup deliberately stops at the exact local Git command that prevents a trustworthy signed-history scenario"
    )]
    fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(
            output.status.success(),
            "fixture Git command should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "fixture inspection deliberately stops when local Git cannot provide the signed-history evidence it is asserting"
    )]
    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(
            output.status.success(),
            "fixture Git command should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("fixture Git output should be UTF-8")
            .trim()
            .to_owned()
    }
}
