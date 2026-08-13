#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

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
    const CLOSED_TASK_ID: &str = "20260812-d444-closed-task";
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

    impl TaskFixture {
        #[expect(
            clippy::implicit_return,
            clippy::single_call_fn,
            reason = "the active-task scenario names its distinct fixture state at its only public-boundary use"
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
                "id: {TASK_ID}\nstatus: backlog\ntitle: Paged task revision {}\nsummary: History page {}.\ncontext: Read the complete EventCore history.\ntags: native\n",
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

        for (index, reference) in ["../x", "", "task.md"].into_iter().enumerate() {
            let marker = directory.path().join(format!("git-was-run-{index}"));
            let usage_exit_code: i32 = 2;
            let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(["tasks", "show", reference])
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
                "invalid task reference {reference:?} must not invoke Git"
            );
        }
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
