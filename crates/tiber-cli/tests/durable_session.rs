#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::std_instead_of_core,
    reason = "the packaged-binary PTY fixture fails fast while constructing an isolated signed repository and owner terminal"
)]
mod tests {
    use std::{
        env, fs,
        io::Write as _,
        os::unix::fs::{PermissionsExt as _, symlink},
        path::{Path, PathBuf},
        process::{Child, Command, Output, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use eventcore_fs::FileEventStore;
    use eventcore_types::{BatchSize, EventStore as _, StreamPattern, StreamVersion, StreamWrites};
    use tempfile::TempDir;
    use tiber_repository_core::Sha256Digest;
    use tiber_repository_service::{RepositoryMutationEvent, RepositoryMutationFact};
    use tiber_session_service::{
        AssistantText, PromptText, SessionBinding, decide_observe_inference,
        decide_request_inference, decide_start_session, task_assignment_scope,
    };
    use tiber_store_git::{TiberEventStore, TransactionEventPage};
    use tiber_tasks_core::TaskId;
    use tiber_workflow_core::{
        AgentId, AssignmentEpoch, AssignmentId, AttemptNumber, ContextReceiptId,
        DeadlineMilliseconds, EffectId, HarnessState, IdempotencyKey, InferEffect,
        PolicyDecisionId, SessionId, WorkflowId,
    };
    use tiber_workflow_service::{WorkflowEvent, WorkflowFact};

    const TASK_PREFIX: &str = "session-fixture";

    #[expect(
        clippy::arbitrary_source_item_ordering,
        reason = "fixture fields follow construction and cleanup flow rather than public API ordering"
    )]
    struct HarnessFixture {
        _directory: TempDir,
        codex_directory: PathBuf,
        initialized: PathBuf,
        repository: PathBuf,
        signing_key: PathBuf,
        oversized: PathBuf,
        terminal_capture: PathBuf,
        invocations: PathBuf,
        approved_crash: PathBuf,
        prepared_crash: PathBuf,
        repository_worker_invocations: PathBuf,
        session_history_reads: PathBuf,
        state_home: PathBuf,
        task_id: String,
        turn_completed: PathBuf,
        completion_release: PathBuf,
    }

    #[expect(
        clippy::arbitrary_source_item_ordering,
        reason = "fixture helpers follow scenario setup and interaction flow"
    )]
    impl HarnessFixture {
        fn new() -> Self {
            Self::with_task(TASK_PREFIX, true)
        }

        #[expect(
            clippy::single_call_fn,
            reason = "one hostile-identity scenario uses the dedicated fixture constructor"
        )]
        fn with_task_prefix(task_prefix: &str) -> Self {
            Self::with_task(task_prefix, true)
        }

        #[expect(
            clippy::single_call_fn,
            reason = "one task-rotation scenario uses the backlog-only fixture constructor"
        )]
        fn with_backlog_task(task_prefix: &str) -> Self {
            Self::with_task(task_prefix, false)
        }

        #[expect(
            clippy::too_many_lines,
            reason = "one bounded black-box fixture constructs the complete signed repository and fake provider"
        )]
        fn with_task(task_prefix: &str, start_task: bool) -> Self {
            let directory = TempDir::new().expect("fixture directory should be created");
            let repository = directory.path().join("repository");
            let codex_directory = directory.path().join("bin");
            let state_home = directory.path().join("state");
            let signing_key = directory.path().join("fixture-signing-key");
            let allowed_signers = directory.path().join("allowed-signers");
            let turn_completed = directory.path().join("turn-completed");
            let completion_release = directory.path().join("completion-release");
            let initialized = directory.path().join("initialized");
            let oversized = directory.path().join("oversized");
            let terminal_capture = directory.path().join("terminal-capture");
            let invocations = directory.path().join("invocations");
            let approved_crash = directory.path().join("approved-crash");
            let prepared_crash = directory.path().join("prepared-crash");
            let repository_worker_invocations =
                directory.path().join("repository-worker-invocations");
            let session_history_reads = directory.path().join("session-history-reads");

            git(directory.path(), ["init", utf8(&repository)]);
            git(
                &repository,
                ["config", "user.name", "Tiber Session Fixture"],
            );
            git(
                &repository,
                [
                    "config",
                    "user.email",
                    "tiber-session-fixture@example.invalid",
                ],
            );
            let key_status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&signing_key)
                .status()
                .expect("fixture SSH signing key generation should start");
            assert!(key_status.success());
            let public_key = fs::read_to_string(signing_key.with_extension("pub"))
                .expect("fixture public signing key should be readable");
            fs::write(
                &allowed_signers,
                format!(
                    "tiber-session-fixture@example.invalid {}",
                    public_key.trim()
                ),
            )
            .expect("fixture allowed signers should be written");
            git(&repository, ["config", "gpg.format", "ssh"]);
            git(&repository, ["config", "commit.gpgsign", "true"]);
            git(
                &repository,
                ["config", "user.signingkey", utf8(&signing_key)],
            );
            git(
                &repository,
                [
                    "config",
                    "gpg.ssh.allowedSignersFile",
                    utf8(&allowed_signers),
                ],
            );

            let store = FileEventStore::open(repository.join("eventstore"))
                .expect("empty fixture EventCore store should initialize");
            drop(store);
            fs::write(repository.join("eventstore/events/.keep"), "")
                .expect("empty history marker should be written");
            git(&repository, ["add", "eventstore/events/.keep"]);
            git(&repository, ["commit", "-m", "empty task authority"]);
            let revision = git_output(&repository, ["rev-parse", "HEAD"]);
            git(
                &repository,
                ["update-ref", "refs/heads/tiber", revision.trim()],
            );

            fs::create_dir_all(&codex_directory)
                .expect("fixture executable directory should be created");
            let fake_server = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripts/tests/fake-app-server.mjs")
                .canonicalize()
                .expect("workspace fake app-server should exist");
            let codex = codex_directory.join("codex");
            fs::write(
                &codex,
                format!("#!/bin/sh\nexec node '{}' \"$@\"\n", fake_server.display()),
            )
            .expect("fixture Codex wrapper should be written");
            fs::set_permissions(&codex, fs::Permissions::from_mode(0o755))
                .expect("fixture Codex wrapper should be executable");

            let mut fixture = Self {
                _directory: directory,
                codex_directory,
                initialized,
                repository,
                signing_key,
                oversized,
                terminal_capture,
                invocations,
                approved_crash,
                prepared_crash,
                state_home,
                repository_worker_invocations,
                session_history_reads,
                task_id: String::new(),
                turn_completed,
                completion_release,
            };
            let created = fixture.tiber(&[
                "tasks",
                "create",
                "--id",
                task_prefix,
                "Resume a durable coding conversation",
            ]);
            assert_success(&created);
            let task_id = created_task_id(&created);
            fixture.task_id.clone_from(&task_id);
            if start_task {
                let started = fixture.tiber(&["tasks", "start", &task_id]);
                assert_success(&started);
            }
            fixture
        }

        fn path(&self) -> std::ffi::OsString {
            let mut entries = vec![self.codex_directory.clone()];
            entries.extend(env::split_paths(
                &env::var_os("PATH").expect("test PATH should be configured"),
            ));
            env::join_paths(entries).expect("fixture PATH should be valid")
        }

        fn tiber(&self, arguments: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_tiber"))
                .args(arguments)
                .current_dir(&self.repository)
                .output()
                .expect("packaged Tiber command should execute")
        }

        fn start_pty(&self) -> Child {
            self.start_pty_mode("split-stream")
        }

        fn start_pty_mode(&self, mode: &str) -> Child {
            self.start_pty_mode_with_capture(mode, Path::new("/dev/null"))
        }

        fn start_pty_mode_with_capture(&self, mode: &str, capture: &Path) -> Child {
            self.start_pty_mode_with_options(mode, capture, false, false, false, None)
        }

        fn start_pty_mode_crash_after_approved(&self, mode: &str) -> Child {
            self.start_pty_mode_with_options(mode, Path::new("/dev/null"), true, false, false, None)
        }

        fn start_pty_mode_crash_after_prepared(&self, mode: &str) -> Child {
            self.start_pty_mode_with_options(mode, Path::new("/dev/null"), false, true, false, None)
        }

        fn start_pty_mode_forced_unknown(&self, mode: &str) -> Child {
            let worker = forced_unknown_repository_worker();
            self.start_pty_mode_with_options(
                mode,
                Path::new("/dev/null"),
                false,
                false,
                false,
                Some(&worker),
            )
        }

        fn start_pty_mode_forced_failure(&self, mode: &str) -> Child {
            self.start_pty_mode_with_options(mode, Path::new("/dev/null"), false, false, true, None)
        }

        fn start_pty_mode_with_options(
            &self,
            mode: &str,
            capture: &Path,
            crash_after_approved: bool,
            crash_after_prepared: bool,
            force_repository_failure: bool,
            repository_worker_override: Option<&Path>,
        ) -> Child {
            let mut command = Command::new("script");
            command
                .args([
                    "--quiet",
                    "--return",
                    "--command",
                    &format!("stty rows 24 cols 80; {}", env!("CARGO_BIN_EXE_tiber")),
                ])
                .arg(capture)
                .current_dir(&self.repository)
                .env("PATH", self.path())
                .env("TIBER_FIXTURE_MODE", mode)
                .env("TIBER_FIXTURE_INITIALIZED_SENTINEL", &self.initialized)
                .env(
                    "TIBER_FIXTURE_TURN_COMPLETED_SENTINEL",
                    &self.turn_completed,
                )
                .env("TIBER_FIXTURE_COMPLETION_RELEASE", &self.completion_release)
                .env("TIBER_FIXTURE_OVERSIZED_SENTINEL", &self.oversized)
                .env("TIBER_FIXTURE_INVOCATIONS", &self.invocations)
                .env(
                    "TIBER_TEST_SESSION_HISTORY_READS",
                    &self.session_history_reads,
                )
                .env("XDG_STATE_HOME", &self.state_home);
            if mode.starts_with("repository-edit") {
                let repository_worker =
                    repository_worker_override.map_or_else(repository_worker, Path::to_path_buf);
                command
                    .env("TIBER_TEST_REPOSITORY_WORKER", repository_worker)
                    .env(
                        "TIBER_TEST_REPOSITORY_WORKER_INVOCATIONS",
                        &self.repository_worker_invocations,
                    );
            }
            if crash_after_prepared {
                command.env(
                    "TIBER_TEST_CRASH_AFTER_PREPARED_SENTINEL",
                    &self.prepared_crash,
                );
            }
            if crash_after_approved {
                command.env(
                    "TIBER_TEST_CRASH_AFTER_APPROVED_SENTINEL",
                    &self.approved_crash,
                );
            }
            if force_repository_failure {
                command.env("TIBER_TEST_REPOSITORY_FAILURE_CODE", "precondition_not_met");
            }
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("packaged Tiber should start inside a PTY")
        }

        async fn seed_started_session(&self) {
            git(&self.repository, ["switch", "tiber"]);
            let binding = session_binding();
            let publication = decide_start_session(&[], binding)
                .expect("fixture session start should be modeled")
                .expect("fixture session is new");
            let (event, [stream]) = publication.into_event_and_consistency_streams();
            let writes = StreamWrites::new()
                .register_stream(stream, StreamVersion::new(0))
                .expect("fixture session stream should register")
                .append(event)
                .expect("fixture session start should append");
            let store = FileEventStore::open(self.repository.join("eventstore"))
                .expect("fixture event store should reopen");
            let _slice = store
                .append_events(writes)
                .await
                .expect("fixture session fact should persist");
            drop(store);
            git(&self.repository, ["add", "eventstore/events"]);
            git(&self.repository, ["commit", "-m", "started session"]);
        }

        fn seed_long_session_history(&self) {
            git(&self.repository, ["switch", "tiber"]);
            let binding = session_binding();
            let start = decide_start_session(&[], binding.clone())
                .expect("start modeled")
                .expect("fixture session is new");
            let (started, [stream]) = start.into_event_and_consistency_streams();
            let mut history = vec![started];
            for turn in 1..=65 {
                let prompt = PromptText::parse(&format!("prompt-{turn}")).expect("prompt");
                let request =
                    decide_request_inference(&history, prompt, turn_effect(&binding, turn))
                        .expect("request modeled");
                let (requested, _) = request.into_event_and_consistency_streams();
                history.push(requested);
                let observation = decide_observe_inference(
                    &history,
                    AssistantText::parse(&format!("answer-{turn}")).expect("assistant"),
                )
                .expect("observation modeled");
                let (observed, _) = observation.into_event_and_consistency_streams();
                history.push(observed);
            }
            let mut writes = StreamWrites::new()
                .register_stream(stream, StreamVersion::new(0))
                .expect("stream registers");
            for event in history {
                writes = writes.append(event).expect("event appends");
            }
            let store =
                FileEventStore::open(self.repository.join("eventstore")).expect("store opens");
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime")
                .block_on(store.append_events(writes))
                .expect("history persists");
            drop(store);
            git(&self.repository, ["add", "eventstore/events"]);
            git(&self.repository, ["commit", "-m", "long session history"]);
        }
    }

    #[test]
    fn stable_startup_reads_one_immutable_verified_session_history() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        let output = child
            .wait_with_output()
            .expect("packaged Tiber should stop cleanly");
        assert_success(&output);
        let reads = fs::read(&fixture.session_history_reads)
            .expect("startup session-history reads should be observable");
        assert_eq!(reads, b"read\n", "startup must share one verified snapshot");
    }

    #[test]
    fn interactive_session_streams_with_active_task_and_displays_durable_bindings() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"keep this conversation durable\r")
            .expect("owner prompt should reach Tiber");
        wait_for_file(&fixture.turn_completed);
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        let output = child
            .wait_with_output()
            .expect("packaged Tiber should stop cleanly");
        assert_success(&output);
        assert!(
            !output.stdout.is_empty(),
            "the PTY should render the terminal UI"
        );

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert!(durable.contains(&format!("task: {TASK_PREFIX}")));
        assert!(durable.contains("session:"));
        assert!(durable.contains("workflow:"));
        assert!(durable.contains("workflow-state: completed"));
        assert!(durable.contains("assignment:"));
        assert!(durable.contains("next-action: prompt"));
        assert!(durable.contains("user: keep this conversation durable"));
        assert!(durable.contains("assistant: hello from Tiber"));
    }

    #[test]
    fn owner_approves_an_exact_scoped_repository_change_from_the_conversation() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"improve the fixture file\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept the approval")
            .write_all(b"approve\r")
            .expect("owner approval should reach Tiber");
        wait_for_session_text(&fixture, "repository change applied: README.md");
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        assert_eq!(
            fs::read_to_string(&target).expect("changed file should remain readable"),
            "after\n"
        );
        let diff = git_output(
            &fixture.repository,
            ["diff", "--no-ext-diff", "--no-textconv", "--", "README.md"],
        );
        assert!(diff.contains("-before"), "missing removed line in: {diff}");
        assert!(diff.contains("+after"), "missing added line in: {diff}");
    }

    #[test]
    fn repository_decision_blocks_next_prompt_until_turn_completion() {
        let fixture = HarnessFixture::new();
        fs::write(fixture.repository.join("README.md"), "before\n").expect("baseline");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit-delayed-completion");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("input")
            .write_all(b"first proposal\r")
            .expect("first prompt");
        wait_for_session_text(&fixture, "repository change proposed: README.md");
        child
            .stdin
            .as_mut()
            .expect("input")
            .write_all(b"deny\rtoo early\r")
            .expect("decision and early prompt");
        wait_for_session_text(&fixture, "repository change denied: README.md");
        assert!(
            child.try_wait().expect("status").is_none(),
            "shell must remain alive"
        );
        assert_eq!(
            invocation_count(&fixture),
            1,
            "early prompt must remain gated"
        );
        fs::write(&fixture.completion_release, b"continue\n").expect("release completion");
        wait_for_file(&fixture.turn_completed);
        wait_for_session_text(&fixture, "workflow-state: completed");
        fs::remove_file(&fixture.turn_completed).expect("reset completion sentinel");
        child
            .stdin
            .as_mut()
            .expect("input")
            .write_all(b"after completion\r")
            .expect("second prompt");
        wait_for_session_text(&fixture, "user: after completion");
        wait_for_file(&fixture.turn_completed);
        assert_eq!(
            invocation_count(&fixture),
            2,
            "post-completion prompt accepted once"
        );
        child
            .stdin
            .as_mut()
            .expect("input")
            .write_all(&[3])
            .expect("quit");
        let _output = child.wait_with_output().expect("exit");
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn first_repository_proposal_remains_the_only_pending_owner_decision() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit-duplicate");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"offer two repository proposals\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        let pending = fixture.tiber(&["session", "active"]);
        assert_success(&pending);
        let pending = String::from_utf8_lossy(&pending.stdout);
        assert_eq!(
            pending
                .matches("repository change proposed: README.md")
                .count(),
            1,
            "only the first proposal may become a durable owner decision: {pending}"
        );
        assert!(!pending.contains("repository change approved:"));
        assert_eq!(repository_worker_invocation_count(&fixture), 0);

        child
            .stdin
            .as_mut()
            .expect("PTY should accept the approval")
            .write_all(b"approve\r")
            .expect("owner approval should reach Tiber");
        wait_for_session_text(&fixture, "repository change applied: README.md");
        assert_eq!(
            fs::read_to_string(&target).expect("approved file should remain readable"),
            "after\n",
            "approval must apply the first exact proposal, never the duplicate"
        );
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
        let decided = fixture.tiber(&["session", "active"]);
        assert_success(&decided);
        let decided = String::from_utf8_lossy(&decided.stdout);
        assert_eq!(
            decided
                .matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert_eq!(
            decided
                .matches("repository change approved: README.md")
                .count(),
            1
        );
        assert_eq!(
            decided
                .matches("repository change prepared: README.md")
                .count(),
            1
        );
        assert_eq!(
            decided
                .matches("repository change applied: README.md")
                .count(),
            1
        );

        child
            .stdin
            .as_mut()
            .expect("composer should remain usable after approval")
            .write_all(b"prompt after duplicate proposal\r")
            .expect("new prompt should reach Tiber");
        wait_for_session_occurrences(
            &fixture,
            "I inspected README.md and propose changing before to after.",
            2,
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
    }

    #[test]
    fn owner_denies_an_exact_repository_change_without_dispatch_and_can_prompt_again() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose a change to deny\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept explicit denial")
            .write_all(b"deny\r")
            .expect("owner denial should reach Tiber");
        wait_for_session_text(&fixture, "repository change denied: README.md");
        assert!(
            !fixture
                .state_home
                .join("tiber/repository-mutations")
                .exists(),
            "denial must not create the repository adapter dispatch journal"
        );

        child
            .stdin
            .as_mut()
            .expect("composer should accept a new prompt after denial")
            .write_all(b"prompt after denial\r")
            .expect("new prompt should reach Tiber");
        wait_for_session_occurrences(
            &fixture,
            "I inspected README.md and propose changing before to after.",
            2,
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        assert_eq!(
            fs::read_to_string(&target).expect("denied file should remain readable"),
            "before\n"
        );
        assert_eq!(invocation_count(&fixture), 2);
        let diff = git_output(
            &fixture.repository,
            ["diff", "--no-ext-diff", "--no-textconv", "--", "README.md"],
        );
        assert!(
            diff.is_empty(),
            "denied change must leave no Git diff: {diff}"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn non_write_repository_tool_request_remains_inert() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit-non-write");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"inspect without proposing a write\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let active = String::from_utf8_lossy(&active.stdout);
        assert!(
            !active.contains("repository change proposed:"),
            "a non-write tool action must remain inert: {active}"
        );
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 0);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
        assert_eq!(
            fs::read_to_string(&target).expect("fixture target should remain readable"),
            "before\n"
        );

        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));
    }

    #[test]
    fn oversized_repository_preimage_is_rejected_before_proposal() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        let sparse = fs::File::create(&target).expect("sparse fixture file should be created");
        sparse
            .set_len(64 * 1024 + 1)
            .expect("sparse fixture should exceed the repository content bound");
        drop(sparse);
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "oversized sparse repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose against an oversized preimage\r")
            .expect("owner prompt should reach Tiber");

        let output = child
            .wait_with_output()
            .expect("oversized preimage rejection should terminate Tiber");
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("repository_mutation_preimage_too_large:"),
            "the public boundary should report the bounded preimage failure: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        assert!(
            !String::from_utf8_lossy(&active.stdout).contains("repository change proposed:"),
            "oversized preimage must never become durable proposal authority"
        );
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 0);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
    }

    #[test]
    fn non_utf8_repository_preimage_is_rejected_before_proposal() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, [0xff, 0xfe, b'\n']).expect("binary fixture should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "non utf8 repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose against a non utf8 preimage\r")
            .expect("owner prompt should reach Tiber");

        let output = child
            .wait_with_output()
            .expect("encoding refusal should terminate Tiber");
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("repository_mutation_preimage_unsupported_encoding:"),
            "public boundary must report the UTF-8-only contract: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        assert!(!String::from_utf8_lossy(&active.stdout).contains("repository change proposed:"));
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 0);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn symlink_repository_preimage_is_rejected_before_proposal() {
        let fixture = HarnessFixture::new();
        let outside = fixture
            .repository
            .parent()
            .expect("fixture parent")
            .join("outside.txt");
        fs::write(&outside, "before\n").expect("outside fixture should be written");
        let target = fixture.repository.join("README.md");
        symlink(&outside, &target).expect("repository symlink should be created");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "symlink repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose against a symlink preimage\r")
            .expect("owner prompt should reach Tiber");

        let deadline = Instant::now() + Duration::from_secs(2);
        let exited = loop {
            if child
                .try_wait()
                .expect("process status should be observable")
                .is_some()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let active = String::from_utf8_lossy(&active.stdout);
        if !exited {
            child
                .stdin
                .as_mut()
                .expect("cleanup PTY")
                .write_all(&[3])
                .expect("cleanup quit");
        }
        let output = child
            .wait_with_output()
            .expect("process should be collected");
        assert!(
            exited,
            "symlink preimage remained active instead of failing closed: {active}"
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("repository_mutation_preimage_unsafe:"),
            "symlink rejection should retain a stable code: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!active.contains("repository change proposed:"));
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
    }

    #[test]
    fn fifo_repository_preimage_is_rejected_without_blocking() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("baseline target should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "regular repository baseline"],
        );
        fs::remove_file(&target).expect("baseline target should be replaced");
        let status = Command::new("mkfifo")
            .arg(&target)
            .status()
            .expect("mkfifo fixture should execute");
        assert!(status.success());
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose against a fifo preimage\r")
            .expect("owner prompt should reach Tiber");

        let deadline = Instant::now() + Duration::from_secs(2);
        let exited = loop {
            if child
                .try_wait()
                .expect("process status should be observable")
                .is_some()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(10));
        };
        if !exited {
            child
                .kill()
                .expect("blocked FIFO fixture should be killable");
        }
        let output = child
            .wait_with_output()
            .expect("process should be collected");
        assert!(
            exited,
            "FIFO preimage must fail within the bounded command deadline"
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("repository_mutation_preimage_unsafe:"),
            "FIFO rejection should retain a stable code: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        assert!(!String::from_utf8_lossy(&active.stdout).contains("repository change proposed:"));
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
    }

    #[test]
    fn owner_cancels_an_exact_repository_change_without_dispatch_and_can_prompt_again() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose a change to cancel\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept explicit cancellation")
            .write_all(b"cancel\r")
            .expect("owner cancellation should reach Tiber");
        wait_for_session_text(&fixture, "repository change cancelled: README.md");
        assert!(
            !fixture
                .state_home
                .join("tiber/repository-mutations")
                .exists(),
            "cancellation must not create the repository adapter dispatch journal"
        );

        child
            .stdin
            .as_mut()
            .expect("composer should accept a new prompt after cancellation")
            .write_all(b"prompt after cancellation\r")
            .expect("new prompt should reach Tiber");
        wait_for_session_occurrences(
            &fixture,
            "I inspected README.md and propose changing before to after.",
            2,
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        assert_eq!(
            fs::read_to_string(&target).expect("cancelled file should remain readable"),
            "before\n"
        );
        assert_eq!(invocation_count(&fixture), 2);
        let diff = git_output(
            &fixture.repository,
            ["diff", "--no-ext-diff", "--no-textconv", "--", "README.md"],
        );
        assert!(
            diff.is_empty(),
            "cancelled change must leave no Git diff: {diff}"
        );
    }

    #[test]
    fn repository_approval_footer_lists_every_owner_decision() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child =
            fixture.start_pty_mode_with_capture("repository-edit", &fixture.terminal_capture);
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"display every owner decision\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept cleanup quit")
            .write_all(&[3])
            .expect("cleanup quit should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        let terminal = fs::read_to_string(&fixture.terminal_capture)
            .expect("captured approval frame should remain readable");
        assert!(
            terminal.contains("approve, deny, or cancel"),
            "approval footer must visibly list every valid decision: {terminal}"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        clippy::too_many_lines,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn restart_cancels_lost_ephemeral_repository_proposal_once() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );

        let mut first = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose a change before restart\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        let open = fixture.tiber(&["session", "active"]);
        assert_success(&open);
        let open = String::from_utf8_lossy(&open.stdout);
        assert_eq!(
            open.matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert!(!open.contains("repository change cancelled:"));
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        first
            .stdin
            .as_mut()
            .expect("PTY should accept quit before owner decision")
            .write_all(&[3])
            .expect("quit should reach Tiber");
        assert_success(&first.wait_with_output().expect("first Tiber exits"));

        fs::remove_file(&fixture.initialized).expect("restart should reset init sentinel");
        let mut recovered =
            fixture.start_pty_mode_with_capture("repository-edit", &fixture.terminal_capture);
        wait_for_file_or_exit(
            &mut recovered,
            &fixture.initialized,
            &fixture.terminal_capture,
        );
        wait_for_session_text(&fixture, "repository change cancelled: README.md");
        let cancelled = fixture.tiber(&["session", "active"]);
        recovered
            .stdin
            .as_mut()
            .expect("restarted PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach restarted Tiber");
        assert_success(&recovered.wait_with_output().expect("restarted Tiber exits"));
        assert_success(&cancelled);
        let cancelled = String::from_utf8_lossy(&cancelled.stdout);
        assert_eq!(
            cancelled
                .matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert_eq!(
            cancelled
                .matches("repository change cancelled: README.md")
                .count(),
            1,
            "restart must terminate the exact open proposal: {cancelled}"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        assert_eq!(
            fs::read_to_string(&target).expect("undispatched file should remain readable"),
            "before\n"
        );

        fs::remove_file(&fixture.initialized).expect("second restart should reset sentinel");
        let mut again = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        let still_cancelled = fixture.tiber(&["session", "active"]);
        assert_success(&still_cancelled);
        assert_eq!(
            String::from_utf8_lossy(&still_cancelled.stdout)
                .matches("repository change cancelled: README.md")
                .count(),
            1,
            "another restart must not duplicate cancellation"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 0);

        again
            .stdin
            .as_mut()
            .expect("composer should accept a fresh prompt")
            .write_all(b"fresh proposal after restart cancellation\r")
            .expect("fresh prompt should reach Tiber");
        wait_for_session_occurrences(
            &fixture,
            "I inspected README.md and propose changing before to after.",
            2,
        );
        let fresh = fixture.tiber(&["session", "active"]);
        assert_success(&fresh);
        let fresh = String::from_utf8_lossy(&fresh.stdout);
        assert_eq!(
            fresh
                .matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert!(!fresh.contains("repository change cancelled:"));
        again
            .stdin
            .as_mut()
            .expect("fresh proposal should accept explicit cancellation")
            .write_all(b"cancel\r")
            .expect("fresh cancellation should reach Tiber");
        wait_for_session_text(&fixture, "repository change cancelled: README.md");
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        again
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&again.wait_with_output().expect("final Tiber exits"));
    }

    #[test]
    fn stale_approval_reproposes_current_bytes_and_requires_a_second_approval() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child =
            fixture.start_pty_mode_with_capture("repository-edit", &fixture.terminal_capture);
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose a change that can become stale\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );

        fs::write(&target, "external\n").expect("external edit should change the proposal digest");
        child
            .stdin
            .as_mut()
            .expect("PTY should accept the stale approval attempt")
            .write_all(b"approve\r")
            .expect("stale approval attempt should reach Tiber");
        wait_for_session_text(&fixture, "repository change reproposed: README.md");
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert_eq!(
            durable
                .matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert_eq!(
            durable
                .matches("repository change reproposed: README.md")
                .count(),
            1
        );
        assert!(durable.contains(&format!(
            "precondition: {}",
            Sha256Digest::of(b"external\n").as_hex()
        )));
        assert!(!durable.contains("repository change approved:"));
        assert!(!durable.contains("repository change prepared:"));
        assert_eq!(
            fs::read_to_string(&target).expect("externally edited file should remain readable"),
            "external\n"
        );
        assert!(
            !fixture
                .state_home
                .join("tiber/repository-mutations")
                .exists(),
            "stale approval must not dispatch the repository adapter"
        );

        child
            .stdin
            .as_mut()
            .expect("PTY should accept approval of the replacement")
            .write_all(b"approve\r")
            .expect("replacement approval should reach Tiber");
        wait_for_session_text(&fixture, "repository change applied: README.md");
        child
            .stdin
            .as_mut()
            .expect("PTY should remain interactive")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        assert_success(&child.wait_with_output().expect("Tiber should exit cleanly"));

        assert_eq!(
            fs::read_to_string(&target).expect("approved replacement should remain readable"),
            "after\n"
        );
        let terminal = fs::read_to_string(&fixture.terminal_capture)
            .expect("captured terminal should remain readable");
        assert!(
            terminal.contains("external"),
            "replacement diff should display the reread bytes"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn oversized_repository_preimage_is_rejected_during_approval_reread() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"propose before the preimage grows\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );

        let sparse = fs::File::create(&target).expect("sparse replacement should be created");
        sparse
            .set_len(64 * 1024 + 1)
            .expect("approval preimage should exceed the content bound");
        drop(sparse);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept approval")
            .write_all(b"approve\r")
            .expect("approval should reach Tiber");

        let deadline = Instant::now() + Duration::from_secs(5);
        let exited = loop {
            if child
                .try_wait()
                .expect("approval process status should remain observable")
                .is_some()
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let active = String::from_utf8_lossy(&active.stdout);
        if !exited {
            child
                .stdin
                .as_mut()
                .expect("PTY should accept cleanup quit")
                .write_all(&[3])
                .expect("cleanup quit should reach Tiber");
        }
        let output = child
            .wait_with_output()
            .expect("oversized approval reread process should be collected");
        assert!(
            exited,
            "approval reread remained active instead of rejecting the oversized preimage: {active}"
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("repository_mutation_preimage_too_large:"),
            "approval reread should report the bounded preimage failure: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            active
                .matches("repository change proposed: README.md")
                .count(),
            1
        );
        assert_eq!(
            active
                .matches("repository change reproposed: README.md")
                .count(),
            0
        );
        assert_eq!(
            active
                .matches("repository change approved: README.md")
                .count(),
            0
        );
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 0);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn approval_and_preparation_are_one_crash_atomic_signed_publication() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(
            &target, "before
",
        )
        .expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );

        let mut child = fixture.start_pty_mode_crash_after_approved("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"approve atomically before a forced crash\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept approval")
            .write_all(b"approve\r")
            .expect("approval should reach Tiber");
        wait_for_file(&fixture.approved_crash);
        let crashed = child
            .wait_with_output()
            .expect("forced post-approval process should terminate");
        assert!(!crashed.status.success());

        let durable = fixture.tiber(&["session", "active"]);
        assert_success(&durable);
        let durable = String::from_utf8_lossy(&durable.stdout);
        assert_eq!(
            durable
                .matches("repository change approved: README.md")
                .count(),
            1
        );
        assert_eq!(
            durable
                .matches("repository change prepared: README.md")
                .count(),
            1,
            "approval must never become durable without its prepared dispatch boundary: {durable}"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        assert_eq!(
            fs::read_to_string(target).expect("pre-dispatch target should remain readable"),
            "before
"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        clippy::too_many_lines,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn restart_reconciles_signed_prepared_once_without_redispatch() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );

        let mut first = fixture.start_pty_mode_crash_after_prepared("repository-edit");
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"prepare a change before crashing\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        first
            .stdin
            .as_mut()
            .expect("PTY should accept approval")
            .write_all(b"approve\r")
            .expect("approval should reach Tiber");
        wait_for_file(&fixture.prepared_crash);
        let crashed = first
            .wait_with_output()
            .expect("prepared crash process should terminate");
        assert!(!crashed.status.success());
        assert_eq!(
            fs::read_to_string(&target).expect("undispatched file should remain readable"),
            "before\n"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        let prepared = fixture.tiber(&["session", "active"]);
        assert_success(&prepared);
        let prepared = String::from_utf8_lossy(&prepared.stdout);
        assert!(prepared.contains("repository change prepared: README.md"));
        assert!(!prepared.contains("repository change applied:"));
        assert!(!prepared.contains("repository change reconciled:"));

        let predecessor_task = fixture.task_id.clone();
        assert_success(&fixture.tiber(&["tasks", "transition", &predecessor_task, "done"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "prepared-successor",
            "Continue after predecessor mutation recovery",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));

        fs::remove_file(&fixture.initialized).expect("restart should reset init sentinel");
        let mut recovered =
            fixture.start_pty_mode_with_capture("repository-edit", &fixture.terminal_capture);
        wait_for_file_or_exit(
            &mut recovered,
            &fixture.initialized,
            &fixture.terminal_capture,
        );
        wait_for_session_text(&fixture, "repository change reconciled: README.md");
        let first_reconciliation = fixture.tiber(&["session", "active"]);
        assert_success(&first_reconciliation);
        assert!(
            String::from_utf8_lossy(&first_reconciliation.stdout)
                .contains("repository change reconciled: README.md not-applied"),
            "pre-dispatch crash must reconcile as not applied: {}",
            String::from_utf8_lossy(&first_reconciliation.stdout)
        );
        assert!(
            String::from_utf8_lossy(&first_reconciliation.stdout)
                .contains(&format!("task: {predecessor_task}")),
            "predecessor authority must remain active through recovery"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 1);
        assert_eq!(
            fs::read_to_string(&target).expect("reconciled file should remain readable"),
            "before\n"
        );
        recovered
            .stdin
            .as_mut()
            .expect("recovered PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach recovered Tiber");
        assert_success(&recovered.wait_with_output().expect("recovered Tiber exits"));

        fs::remove_file(&fixture.initialized).expect("second restart should reset sentinel");
        let mut again = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        assert_eq!(repository_worker_invocation_count(&fixture), 1);
        let reconciled = fixture.tiber(&["session", "active"]);
        assert_success(&reconciled);
        assert!(
            String::from_utf8_lossy(&reconciled.stdout).contains(&format!("task: {successor}"))
        );
        again
            .stdin
            .as_mut()
            .expect("second restart PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach second restart");
        assert_success(&again.wait_with_output().expect("second restart exits"));
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn adapter_failure_preserves_typed_code_retry_guidance_and_durable_query() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut child = fixture.start_pty_mode_forced_failure("repository-edit");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"force a typed adapter failure\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        child
            .stdin
            .as_mut()
            .expect("PTY should accept approval")
            .write_all(b"approve\r")
            .expect("approval should reach Tiber");
        let failed = child
            .wait_with_output()
            .expect("definitive adapter failure should terminate");
        assert!(!failed.status.success());
        let output = String::from_utf8_lossy(&failed.stdout);
        assert!(
            output.contains("repository_precondition_not_met:"),
            "CLI must preserve the adapter's stable typed code: {output}"
        );
        assert!(
            output.contains("fresh authorization required"),
            "CLI must render the safe retry directive: {output}"
        );

        let durable = fixture.tiber(&["session", "active"]);
        assert_success(&durable);
        let durable = String::from_utf8_lossy(&durable.stdout);
        assert!(
            durable.contains(
                "repository change failed: README.md repository_precondition_not_met retry: fresh-authorization-required"
            ),
            "durable query must render the content-free failure receipt: {durable}"
        );
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
        assert_eq!(
            fs::read_to_string(target).expect("failed target should remain readable"),
            "before\n"
        );
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        clippy::too_many_lines,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn restart_reconciles_forced_unknown_once_without_mutation_redispatch() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );

        let mut first = fixture.start_pty_mode_forced_unknown("repository-edit");
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(b"force an ambiguous adapter outcome\r")
            .expect("owner prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        first
            .stdin
            .as_mut()
            .expect("PTY should accept approval")
            .write_all(b"approve\r")
            .expect("approval should reach Tiber");
        let unknown = first
            .wait_with_output()
            .expect("forced unknown process should terminate");
        assert!(!unknown.status.success());
        assert_eq!(repository_worker_invocation_count(&fixture), 1);
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
        assert_eq!(
            fs::read_to_string(&target).expect("ambiguous file should remain readable"),
            "before\n"
        );
        let durable_unknown = fixture.tiber(&["session", "active"]);
        assert_success(&durable_unknown);
        let durable_unknown = String::from_utf8_lossy(&durable_unknown.stdout);
        assert!(durable_unknown.contains("repository change prepared: README.md"));
        assert!(durable_unknown.contains(
            "repository change unknown: README.md retry: read-only-reconciliation-required"
        ));
        assert!(!durable_unknown.contains("repository change reconciled:"));

        fs::remove_file(&fixture.initialized).expect("restart should reset init sentinel");
        let mut recovered =
            fixture.start_pty_mode_with_capture("repository-edit", &fixture.terminal_capture);
        wait_for_file_or_exit(
            &mut recovered,
            &fixture.initialized,
            &fixture.terminal_capture,
        );
        wait_for_session_text(&fixture, "repository change reconciled: README.md");
        let reconciliation = fixture.tiber(&["session", "active"]);
        recovered
            .stdin
            .as_mut()
            .expect("recovered PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach recovered Tiber");
        assert_success(&recovered.wait_with_output().expect("recovered Tiber exits"));
        assert_success(&reconciliation);
        let recovered_terminal = fs::read_to_string(&fixture.terminal_capture)
            .expect("recovered terminal transcript should remain readable");
        assert!(
            recovered_terminal.contains("reconciled:")
                && recovered_terminal.contains("README.md")
                && recovered_terminal.contains("not-applied"),
            "restart reconciliation must be visible in the interactive transcript before input: {recovered_terminal}"
        );
        assert!(
            String::from_utf8_lossy(&reconciliation.stdout)
                .contains("repository change reconciled: README.md not-applied"),
            "restart must read-only reconcile the durable unknown: {}",
            String::from_utf8_lossy(&reconciliation.stdout)
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 2);
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 1);
        assert_eq!(
            fs::read_to_string(&target).expect("reconciled file should remain readable"),
            "before\n"
        );

        fs::remove_file(&fixture.initialized).expect("second restart should reset sentinel");
        let mut again = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        assert_eq!(repository_worker_invocation_count(&fixture), 2);
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 1);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 1);
        let reconciled = fixture.tiber(&["session", "active"]);
        assert_success(&reconciled);
        assert_eq!(
            String::from_utf8_lossy(&reconciled.stdout)
                .matches("repository change reconciled: README.md not-applied")
                .count(),
            1
        );
        again
            .stdin
            .as_mut()
            .expect("second restart PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach second restart");
        assert_success(&again.wait_with_output().expect("second restart exits"));
    }

    #[test]
    fn prompt_publication_reports_the_typed_signing_failure() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        fs::rename(
            &fixture.signing_key,
            fixture.signing_key.with_extension("unavailable"),
        )
        .expect("fixture signing authority should become temporarily unavailable");
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"durable failure\r")
            .expect("prompt reaches Tiber");

        let output = child
            .wait_with_output()
            .expect("Tiber reports publication failure");

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("tiber_git_sign_tiber_candidate_failed:")
        );

        fs::rename(
            fixture.signing_key.with_extension("unavailable"),
            &fixture.signing_key,
        )
        .expect("fixture signing authority should be restored");
        fs::remove_file(&fixture.initialized).expect("reset init sentinel");
        let mut retry = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        retry
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"durable failure\r")
            .expect("same prompt retries");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        retry
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&retry.wait_with_output().expect("retry exits"));
        assert_eq!(invocation_count(&fixture), 1);
        let active = fixture.tiber(&["session", "active"]);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert_eq!(durable.matches("user: durable failure").count(), 1);
        assert_eq!(durable.matches("assistant: hello from Tiber").count(), 1);
    }

    #[test]
    fn terminal_task_session_is_succeeded_by_the_new_active_task() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("first session exits");
        assert_success(&first.wait_with_output().expect("first Tiber exits"));

        let first_task = fixture.task_id.clone();
        assert_success(&fixture.tiber(&["tasks", "transition", &first_task, "done"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "successor-fixture",
            "Continue with a successor task",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));

        fs::remove_file(&fixture.initialized).expect("reset init sentinel");
        let mut second = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"successor owns this turn\r")
            .expect("successor prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("successor session exits");
        assert_success(&second.wait_with_output().expect("successor Tiber exits"));

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let rendered = String::from_utf8_lossy(&active.stdout);
        assert!(rendered.contains(&format!("task: {successor}")));
        assert!(!rendered.contains(&format!("task: {first_task}\n")));
        assert!(rendered.contains("user: successor owns this turn"));
    }

    #[test]
    #[expect(
        clippy::shadow_reuse,
        reason = "the black-box restart scenario preserves successive durable-state names to show each lifecycle transition"
    )]
    fn successor_session_restart_does_not_cancel_predecessor_proposal() {
        let fixture = HarnessFixture::new();
        let target = fixture.repository.join("README.md");
        fs::write(&target, "before\n").expect("fixture repository file should be written");
        git(&fixture.repository, ["add", "README.md"]);
        git(
            &fixture.repository,
            ["commit", "-m", "fixture repository baseline"],
        );
        let mut predecessor = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        predecessor
            .stdin
            .as_mut()
            .expect("predecessor PTY should accept input")
            .write_all(b"leave a proposal for the predecessor\r")
            .expect("predecessor prompt should reach Tiber");
        wait_for_session_text(
            &fixture,
            "I inspected README.md and propose changing before to after.",
        );
        let predecessor_active = fixture.tiber(&["session", "active"]);
        assert_success(&predecessor_active);
        let predecessor_active = String::from_utf8_lossy(&predecessor_active.stdout);
        let predecessor_effect = predecessor_active
            .lines()
            .find_map(|line| line.strip_prefix("effect: "))
            .expect("predecessor effect should be projected")
            .to_owned();
        assert_eq!(
            repository_cancellation_count(&fixture, &predecessor_effect),
            0
        );
        predecessor
            .stdin
            .as_mut()
            .expect("predecessor PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach predecessor");
        assert_success(
            &predecessor
                .wait_with_output()
                .expect("predecessor Tiber exits"),
        );

        let predecessor_task = fixture.task_id.clone();
        assert_success(&fixture.tiber(&["tasks", "transition", &predecessor_task, "done"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "proposal-successor",
            "Continue without predecessor proposal recovery",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));

        fs::remove_file(&fixture.initialized).expect("successor restart resets sentinel");
        let mut restarted = fixture.start_pty_mode("repository-edit");
        wait_for_file(&fixture.initialized);
        let successor_active = fixture.tiber(&["session", "active"]);
        let predecessor_cancellations =
            repository_cancellation_count(&fixture, &predecessor_effect);
        restarted
            .stdin
            .as_mut()
            .expect("successor PTY should accept quit")
            .write_all(&[3])
            .expect("quit should reach successor");
        assert_success(&restarted.wait_with_output().expect("successor Tiber exits"));

        assert_success(&successor_active);
        assert!(
            String::from_utf8_lossy(&successor_active.stdout)
                .contains(&format!("task: {successor}"))
        );
        assert_eq!(
            predecessor_cancellations, 0,
            "successor restart must not publish into predecessor proposal history"
        );
        assert_eq!(repository_worker_invocation_count(&fixture), 0);
        assert_eq!(
            fs::read_to_string(&target).expect("predecessor file remains readable"),
            "before\n"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the black-box crash-boundary scenario proves signed publication order across recovery and transfer"
    )]
    fn observed_workflow_completes_before_terminal_session_transfers() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"observed before transfer\r")
            .expect("predecessor prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("predecessor session exits");
        assert_success(&first.wait_with_output().expect("predecessor Tiber exits"));

        let predecessor_output = fixture.tiber(&["session", "active"]);
        assert_success(&predecessor_output);
        let predecessor_rendered = String::from_utf8_lossy(&predecessor_output.stdout);
        let effect_id = predecessor_rendered
            .lines()
            .find_map(|line| line.strip_prefix("effect: "))
            .expect("predecessor effect should be projected")
            .to_owned();

        let observed_revision = git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber^"]);
        git(
            &fixture.repository,
            ["update-ref", "refs/heads/tiber", observed_revision.trim()],
        );
        assert!(!workflow_completed(&fixture, &effect_id));
        assert_success(&fixture.tiber(&["tasks", "transition", &fixture.task_id, "done"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "observed-successor",
            "Continue after observed predecessor",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));
        let authority_before_relaunch =
            git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber"]);

        fs::remove_file(&fixture.initialized).expect("reset init sentinel");
        let mut relaunched = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        let transfer_revision = git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber"]);
        let completed_revision = git_output(
            &fixture.repository,
            ["rev-parse", &format!("{}^", transfer_revision.trim())],
        );
        let prior_revision = git_output(
            &fixture.repository,
            ["rev-parse", &format!("{}^", completed_revision.trim())],
        );
        assert_eq!(prior_revision.trim(), authority_before_relaunch.trim());
        git(
            &fixture.repository,
            ["update-ref", "refs/heads/tiber", completed_revision.trim()],
        );
        assert!(workflow_completed(&fixture, &effect_id));
        let before_transfer = fixture.tiber(&["session", "active"]);
        assert_success(&before_transfer);
        assert!(
            String::from_utf8_lossy(&before_transfer.stdout)
                .contains(&format!("task: {}", fixture.task_id))
        );
        git(
            &fixture.repository,
            ["update-ref", "refs/heads/tiber", transfer_revision.trim()],
        );
        let after_transfer = fixture.tiber(&["session", "active"]);
        assert_success(&after_transfer);
        assert!(
            String::from_utf8_lossy(&after_transfer.stdout).contains(&format!("task: {successor}"))
        );
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"successor after observed recovery\r")
            .expect("successor prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("successor session exits");
        assert_success(
            &relaunched
                .wait_with_output()
                .expect("successor Tiber exits"),
        );

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let rendered = String::from_utf8_lossy(&active.stdout);
        assert!(rendered.contains(&format!("task: {successor}")));
        assert!(rendered.contains("user: successor after observed recovery"));
        assert_eq!(invocation_count(&fixture), 2);
        assert!(workflow_completed(&fixture, &effect_id));
    }

    #[test]
    fn abandoned_task_session_is_succeeded_by_the_new_active_task() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&first.wait_with_output().expect("first exits"));
        let abandoned = fixture.task_id.clone();
        assert_success(&fixture.tiber(&["tasks", "transition", &abandoned, "abandoned"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "abandoned-successor",
            "Continue abandoned work",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));
        fs::remove_file(&fixture.initialized).expect("reset sentinel");
        let mut second = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"successor after abandonment\r")
            .expect("prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&second.wait_with_output().expect("second exits"));
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let rendered = String::from_utf8_lossy(&active.stdout);
        assert!(rendered.contains(&format!("task: {successor}")));
        assert!(rendered.contains("user: successor after abandonment"));
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        reason = "the crash-boundary scenario intentionally discards only the interrupted process result"
    )]
    fn terminal_task_with_pending_inference_remains_in_reconciliation_before_successor() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty_mode("hold-thread-start");
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"pending A\r")
            .expect("prompt");
        wait_for_session_text(&fixture, "user: pending A");
        first.kill().expect("kill");
        let _ = first.wait();
        assert_success(&fixture.tiber(&["tasks", "transition", &fixture.task_id, "abandoned"]));
        let created = fixture.tiber(&[
            "tasks",
            "create",
            "--id",
            "pending-successor",
            "Must wait for reconciliation",
        ]);
        assert_success(&created);
        let successor = created_task_id(&created);
        assert_success(&fixture.tiber(&["tasks", "start", &successor]));
        fs::remove_file(&fixture.initialized).expect("reset sentinel");

        let mut relaunched = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        let output = relaunched.wait_with_output().expect("relaunch exits");
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("reconcile required"));
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let rendered = String::from_utf8_lossy(&active.stdout);
        assert!(rendered.contains(&format!("task: {}", fixture.task_id)));
        assert!(!rendered.contains(&format!("task: {successor}")));
    }

    #[test]
    fn failed_provider_turn_locks_prompting_until_explicit_reconciliation() {
        let fixture = HarnessFixture::new();
        let mut child =
            fixture.start_pty_mode_with_capture("close-after-request", &fixture.terminal_capture);
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"first durable request\r")
            .expect("first prompt");
        thread::sleep(Duration::from_millis(500));
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"second request must remain blocked\r")
            .expect("second input");
        thread::sleep(Duration::from_millis(200));
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        let output = child.wait_with_output().expect("Tiber exits cleanly");
        assert_success(&output);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("reconcile required")
                || fs::read_to_string(&fixture.terminal_capture)
                    .is_ok_and(|text| text.contains("reconcile required"))
        );

        assert_eq!(
            fs::read_to_string(&fixture.invocations)
                .expect("invocation ledger")
                .lines()
                .count(),
            1
        );
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        assert!(String::from_utf8_lossy(&active.stdout).contains("next-action: reconcile"));
    }

    #[test]
    fn oversized_streamed_assistant_is_rejected_without_a_durable_observation() {
        let fixture = HarnessFixture::new();
        let mut child =
            fixture.start_pty_mode_with_capture("oversized-assistant", &fixture.terminal_capture);
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"bounded output\r")
            .expect("prompt");
        wait_for_file(&fixture.oversized);
        let output = child
            .wait_with_output()
            .expect("Tiber reports bounded-output failure");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("session_assistant_too_large:"));

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert!(durable.contains("user: bounded output"));
        assert!(!durable.contains("assistant:"));
    }

    #[test]
    fn terminal_control_from_provider_is_rejected_without_rendering_or_persistence() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty_mode("control-assistant");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"safe terminal output\r")
            .expect("prompt");
        let output = child
            .wait_with_output()
            .expect("Tiber rejects control output");
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("session_assistant_control_character:")
        );
        assert!(
            !output
                .stdout
                .windows(b"PROVIDER_BEFORE\x1b[31mPROVIDER_AFTER".len())
                .any(|window| window == b"PROVIDER_BEFORE\x1b[31mPROVIDER_AFTER")
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PROVIDER_BEFORE"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PROVIDER_AFTER"));

        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert!(durable.contains("user: safe terminal output"));
        assert!(!durable.contains("assistant:"));
    }

    #[test]
    fn relaunch_restores_the_completed_transcript_and_prompt_next_action() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"first durable turn\r")
            .expect("prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&first.wait_with_output().expect("first launch exits"));
        fs::remove_file(&fixture.initialized).expect("initialization sentinel resets");
        let revision_before_relaunch =
            git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber"]);

        let mut second = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"x\x03")
            .expect("render then quit");
        let output = second.wait_with_output().expect("relaunch exits");

        assert_success(&output);
        let terminal = String::from_utf8_lossy(&output.stdout);
        assert!(
            terminal.contains("first") && terminal.contains("durable") && terminal.contains("turn")
        );
        assert!(
            terminal.contains("hello") && terminal.contains("from") && terminal.contains("Tiber")
        );
        let revision_after_relaunch =
            git_output(&fixture.repository, ["rev-parse", "refs/heads/tiber"]);
        assert_eq!(revision_after_relaunch, revision_before_relaunch);
        let active = fixture.tiber(&["session", "active"]);
        assert_success(&active);
        let restored = String::from_utf8_lossy(&active.stdout);
        assert!(restored.contains("user: first durable turn"));
        assert!(restored.contains("assistant: hello from Tiber"));
        assert!(restored.contains("next-action: prompt"));
    }

    #[test]
    #[expect(
        clippy::indexing_slicing,
        reason = "the fixture asserts a fixed two-turn transcript before selecting each expected turn"
    )]
    fn relaunch_accepts_exactly_one_additional_durable_turn_in_the_same_session() {
        let fixture = HarnessFixture::new();
        let mut first = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"first turn\r")
            .expect("first prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        first
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&first.wait_with_output().expect("first exits"));
        fs::remove_file(&fixture.initialized).expect("reset init sentinel");
        fs::remove_file(&fixture.turn_completed).expect("reset turn sentinel");

        let mut second = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"second turn\r")
            .expect("second prompt");
        wait_for_session_text(&fixture, "user: second turn");
        wait_for_session_occurrences(&fixture, "assistant: hello from Tiber", 2);
        second
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        assert_success(&second.wait_with_output().expect("second exits"));

        let active = fixture.tiber(&["session", "active"]);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert_eq!(durable.matches("user:").count(), 2);
        assert_eq!(durable.matches("assistant:").count(), 2);
        assert_eq!(durable.matches("session:").count(), 1);
        let effects = durable
            .lines()
            .filter(|line| line.starts_with("effect: "))
            .collect::<Vec<_>>();
        assert_eq!(effects.len(), 2);
        assert_ne!(effects[0], effects[1]);
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        reason = "the crash-boundary scenario intentionally discards only the interrupted process result"
    )]
    fn requested_kill_relaunches_to_reconcile_without_provider_invocation() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty_mode("hold-thread-start");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"requested boundary\r")
            .expect("prompt");
        wait_for_session_text(&fixture, "user: requested boundary");
        assert_eq!(invocation_count(&fixture), 0);
        child.kill().expect("kill requested-boundary Tiber");
        let _ = child.wait();
        fs::remove_file(&fixture.initialized).expect("reset init sentinel");

        let mut relaunched = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"must remain blocked\r")
            .expect("blocked input");
        thread::sleep(Duration::from_millis(100));
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        let output = relaunched.wait_with_output().expect("relaunch exits");
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("reconcile required"));

        assert_eq!(invocation_count(&fixture), 0);
        let active = fixture.tiber(&["session", "active"]);
        assert!(String::from_utf8_lossy(&active.stdout).contains("next-action: reconcile"));
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        reason = "the crash-boundary scenario intentionally discards only the interrupted process result"
    )]
    fn in_flight_kill_relaunches_to_reconcile_without_a_second_provider_invocation() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty_mode("delayed-stream");
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"in flight boundary\r")
            .expect("prompt");
        wait_for_file(&fixture.invocations);
        child.kill().expect("kill in-flight Tiber");
        let _ = child.wait();
        fs::remove_file(&fixture.initialized).expect("reset init sentinel");

        let mut relaunched = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        let output = relaunched.wait_with_output().expect("relaunch exits");
        assert_success(&output);

        assert_eq!(invocation_count(&fixture), 1);
        let active = fixture.tiber(&["session", "active"]);
        assert!(String::from_utf8_lossy(&active.stdout).contains("next-action: reconcile"));
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        reason = "the crash-boundary scenario intentionally discards only the interrupted process result"
    )]
    fn observed_kill_relaunches_to_prompt_without_a_second_provider_invocation() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(b"observed boundary\r")
            .expect("prompt");
        wait_for_session_text(&fixture, "assistant: hello from Tiber");
        child.kill().expect("kill observed Tiber");
        let _ = child.wait();
        fs::remove_file(&fixture.initialized).expect("reset init sentinel");

        let mut relaunched = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        relaunched
            .stdin
            .as_mut()
            .expect("PTY input")
            .write_all(&[3])
            .expect("quit");
        let output = relaunched.wait_with_output().expect("relaunch exits");
        assert_success(&output);

        assert_eq!(invocation_count(&fixture), 1);
        let active = fixture.tiber(&["session", "active"]);
        assert!(String::from_utf8_lossy(&active.stdout).contains("next-action: prompt"));
    }

    #[test]
    #[expect(
        clippy::default_numeric_fallback,
        reason = "the local fixture port is used only to prove a typed connection failure"
    )]
    fn active_session_query_reports_missing_signed_authority_distinctly() {
        let directory = TempDir::new().expect("fixture directory should be created");
        git(
            directory.path(),
            ["init", utf8(&directory.path().join("repository"))],
        );
        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .args(["session", "active"])
            .current_dir(directory.path().join("repository"))
            .env("XDG_STATE_HOME", directory.path().join("state"))
            .output()
            .expect("packaged Tiber query should execute");

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tiber_git_resolve_tiber_ref_failed: signed Tiber authority could not be read\n"
        );
    }

    #[test]
    fn session_query_paginates_the_complete_durable_history() {
        let fixture = HarnessFixture::new();
        fixture.seed_long_session_history();

        let active = fixture.tiber(&["session", "active"]);

        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert!(durable.contains("user: prompt-65"));
        assert!(durable.contains("assistant: answer-65"));
    }

    #[test]
    fn startup_recovery_ignores_completed_turns_when_bounding_unresolved_candidates() {
        let fixture = HarnessFixture::new();
        fixture.seed_long_session_history();

        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("healthy long session should accept owner input")
            .write_all(&[3])
            .expect("owner quit should reach the healthy long session");
        let output = child
            .wait_with_output()
            .expect("healthy long session exits");

        assert_success(&output);
        assert_eq!(repository_worker_operation_count(&fixture, "dispatch"), 0);
        assert_eq!(repository_worker_operation_count(&fixture, "reconcile"), 0);
    }

    #[test]
    #[expect(
        clippy::default_numeric_fallback,
        reason = "the local fixture port is used only to prove the missing-session projection"
    )]
    fn active_session_query_reports_that_signed_history_has_no_session() {
        let fixture = HarnessFixture::new();

        let output = fixture.tiber(&["session", "active"]);

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "tiber_session_not_found: this repository has no active Tiber session\n"
        );
    }

    #[test]
    fn public_help_advertises_the_supported_session_query() {
        let fixture = HarnessFixture::new();
        let root = fixture.tiber(&["--help"]);
        assert_success(&root);
        assert!(String::from_utf8_lossy(&root.stdout).contains("session active"));
        let nested = fixture.tiber(&["session", "--help"]);
        assert_success(&nested);
        assert_eq!(
            String::from_utf8_lossy(&nested.stdout),
            "usage: tiber session active\n"
        );
    }

    #[tokio::test]
    async fn active_session_query_projects_a_durable_started_binding() {
        let fixture = HarnessFixture::new();
        fixture.seed_started_session().await;

        let output = fixture.tiber(&["session", "active"]);

        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            concat!(
                "task: session-fixture-resume-a-durable-coding-conversation\n",
                "session: session-1\n",
                "workflow: workflow-1\n",
                "workflow-state: ready\n",
                "assignment: assignment-1\n",
                "next-action: prompt\n",
            )
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn tui_startup_binds_the_active_task_to_a_durable_session() {
        let fixture = HarnessFixture::new();
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        let stopped = child
            .wait_with_output()
            .expect("packaged Tiber should stop cleanly");
        assert_success(&stopped);

        let active = fixture.tiber(&["session", "active"]);

        assert_success(&active);
        let durable = String::from_utf8_lossy(&active.stdout);
        assert!(durable.contains(&format!("task: {TASK_PREFIX}")));
        assert!(durable.contains("session:"));
        assert!(durable.contains("workflow:"));
        assert!(durable.contains("assignment:"));
        assert!(durable.contains("next-action: prompt"));
    }

    #[test]
    fn tui_startup_accepts_a_valid_long_task_identity() {
        let task_prefix = "long-task-".repeat(20);
        let fixture = HarnessFixture::with_task_prefix(&task_prefix);
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        let stopped = child
            .wait_with_output()
            .expect("packaged Tiber should stop cleanly");
        assert_success(&stopped);

        let active = fixture.tiber(&["session", "active"]);

        assert_success(&active);
        assert!(String::from_utf8_lossy(&active.stdout).contains(&format!("task: {task_prefix}")));
    }

    #[test]
    fn tui_startup_selects_and_starts_the_eligible_backlog_task() {
        let fixture = HarnessFixture::with_backlog_task(TASK_PREFIX);
        let mut child = fixture.start_pty();
        wait_for_file(&fixture.initialized);
        child
            .stdin
            .as_mut()
            .expect("PTY should accept owner input")
            .write_all(&[3])
            .expect("owner quit intent should reach Tiber");
        let stopped = child
            .wait_with_output()
            .expect("packaged Tiber should stop cleanly");
        assert_success(&stopped);

        let active_session = fixture.tiber(&["session", "active"]);
        assert_success(&active_session);
        assert!(
            String::from_utf8_lossy(&active_session.stdout)
                .contains(&format!("task: {TASK_PREFIX}"))
        );
        let active_task = fixture.tiber(&["tasks", "list", "--status", "in-progress"]);
        assert_success(&active_task);
        assert!(String::from_utf8_lossy(&active_task.stdout).contains(TASK_PREFIX));
    }

    #[test]
    fn tui_startup_reports_when_no_task_is_eligible() {
        let fixture = HarnessFixture::new();
        let active = fixture.tiber(&["tasks", "list", "--status", "in-progress"]);
        assert_success(&active);
        let task = String::from_utf8_lossy(&active.stdout)
            .split('\t')
            .next()
            .expect("fixture active task should have an identity")
            .to_owned();
        let done = fixture.tiber(&["tasks", "transition", &task, "done"]);
        assert_success(&done);

        let output = fixture
            .start_pty()
            .wait_with_output()
            .expect("packaged Tiber should report startup failure");

        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            "tiber_session_no_eligible_task: no active or eligible backlog task is available\n"
        );
    }

    fn session_binding() -> SessionBinding {
        let task = TaskId::parse("session-fixture-resume-a-durable-coding-conversation")
            .expect("task identity should be valid");
        let effect = InferEffect::new(
            parsed(SessionId::parse, "session-1"),
            parsed(AgentId::parse, "agent-1"),
            parsed(WorkflowId::parse, "workflow-1"),
            parsed(AssignmentId::parse, "assignment-1"),
            task_assignment_scope(&task).expect("task scope"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            parsed(ContextReceiptId::parse, "context-1"),
            parsed(PolicyDecisionId::parse, "policy-1"),
            parsed(EffectId::parse, "effect-1"),
            parsed(IdempotencyKey::parse, "session-1:turn-1"),
            DeadlineMilliseconds::parse(60_000).expect("deadline should be valid"),
        );
        SessionBinding::new(task, HarnessState::new(effect))
    }

    #[expect(
        clippy::single_call_fn,
        reason = "one pagination fixture derives deterministic later-turn effects"
    )]
    fn turn_effect(binding: &SessionBinding, turn: usize) -> InferEffect {
        let base = binding.workflow_state().initial_effect();
        InferEffect::new(
            base.session_id().clone(),
            base.agent_id().clone(),
            base.workflow_id().clone(),
            base.assignment_id().clone(),
            base.assignment_scope().clone(),
            base.assignment_epoch(),
            base.attempt_number(),
            parsed(ContextReceiptId::parse, &format!("context-{turn}")),
            parsed(PolicyDecisionId::parse, &format!("policy-{turn}")),
            parsed(EffectId::parse, &format!("effect-{turn}")),
            parsed(IdempotencyKey::parse, &format!("session-1:turn-{turn}")),
            base.deadline_milliseconds(),
        )
    }

    #[expect(
        clippy::panic,
        reason = "the generic fixture reports the exact invalid deterministic value"
    )]
    fn parsed<T, E: core::fmt::Display>(
        parser: impl FnOnce(&str) -> Result<T, E>,
        value: &str,
    ) -> T {
        parser(value).unwrap_or_else(|error| panic!("{value} should parse: {error}"))
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the forced-ambiguity scenario alone resolves the pinned sandbox worker"
    )]
    fn forced_unknown_repository_worker() -> PathBuf {
        let path = env::var_os("PATH").expect("test PATH should be available");
        env::split_paths(&path)
            .map(|directory| directory.join("bwrap"))
            .find(|candidate| candidate.is_file())
            .expect("bwrap should be available inside the pinned test shell")
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the repository helper build is owned by one packaged-binary fixture boundary"
    )]
    fn repository_worker() -> PathBuf {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "build",
                "-p",
                "tiber-repository-linux",
                "--bin",
                "tiber-repository-worker",
            ])
            .current_dir(workspace)
            .status()
            .expect("fixture repository worker build should start");
        assert!(status.success(), "fixture repository worker should build");
        PathBuf::from(env!("CARGO_BIN_EXE_tiber"))
            .parent()
            .expect("packaged binary should have a target directory")
            .join("tiber-repository-worker")
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "the bounded polling fixture increments a capped local attempt counter"
    )]
    fn wait_for_session_text(fixture: &HarnessFixture, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = fixture.tiber(&["session", "active"]);
            let last = String::from_utf8_lossy(&output.stdout).into_owned();
            if output.status.success() && last.contains(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "session projection did not contain {expected}; last: {last}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "one bounded polling fixture increments a capped local attempt counter"
    )]
    fn wait_for_session_occurrences(fixture: &HarnessFixture, expected: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = fixture.tiber(&["session", "active"]);
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .matches(expected)
                    .count()
                    == count
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "session projection did not contain {count} occurrences of {expected}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn invocation_count(fixture: &HarnessFixture) -> usize {
        fs::read_to_string(&fixture.invocations).map_or(0, |text| text.lines().count())
    }

    fn repository_worker_invocation_count(fixture: &HarnessFixture) -> usize {
        fs::read_to_string(&fixture.repository_worker_invocations)
            .map_or(0, |text| text.lines().count())
    }

    fn repository_worker_operation_count(fixture: &HarnessFixture, operation: &str) -> usize {
        fs::read_to_string(&fixture.repository_worker_invocations).map_or(0, |text| {
            text.lines().filter(|line| *line == operation).count()
        })
    }

    fn repository_cancellation_count(fixture: &HarnessFixture, effect_id: &str) -> usize {
        let store = TiberEventStore::open(&fixture.repository).expect("signed authority opens");
        let pattern = StreamPattern::try_new(format!("tiber:repository-mutation:{effect_id}"))
            .expect("effect identifies a valid repository mutation stream");
        let reader = store
            .verified_transaction_reader::<RepositoryMutationEvent>(&[pattern])
            .expect("repository mutation history is verified");
        reader
            .read_page(TransactionEventPage::first(BatchSize::new(128)))
            .expect("repository mutation history reads")
            .iter()
            .filter(|event| matches!(event.fact(), RepositoryMutationFact::Cancelled(_)))
            .count()
    }

    fn workflow_completed(fixture: &HarnessFixture, effect_id: &str) -> bool {
        let store = TiberEventStore::open(&fixture.repository).expect("signed authority opens");
        let pattern = StreamPattern::try_new(format!("tiber:workflow:{effect_id}"))
            .expect("projected effect identifies a valid workflow stream");
        let reader = store
            .verified_transaction_reader::<WorkflowEvent>(&[pattern])
            .expect("workflow history is verified");
        reader
            .read_page(TransactionEventPage::first(BatchSize::new(128)))
            .expect("workflow history reads")
            .iter()
            .any(|event| matches!(event.fact(), WorkflowFact::WorkflowCompleted { .. }))
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "the bounded polling fixture increments a capped local attempt counter"
    )]
    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.is_file() {
            assert!(
                Instant::now() < deadline,
                "fixture file should appear: {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        clippy::panic,
        reason = "bounded crash fixtures poll a child and fail fast with captured diagnostics"
    )]
    fn wait_for_file_or_exit(child: &mut Child, path: &Path, capture: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.is_file() {
            if child
                .try_wait()
                .expect("fixture process status should remain readable")
                .is_some()
            {
                panic!(
                    "fixture exited before {} appeared: {}",
                    path.display(),
                    fs::read_to_string(capture).unwrap_or_default()
                );
            }
            assert!(
                Instant::now() < deadline,
                "fixture file should appear: {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_success(output: &Output) {
        assert!(
            output.status.success(),
            "command should succeed; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn created_task_id(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout)
            .strip_prefix("created ")
            .and_then(|text| text.split_once(" at ").map(|(id, _revision)| id.to_owned()))
            .expect("task creation should name its durable identity")
    }

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

    fn git_output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .expect("fixture Git command should start");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("fixture Git output should be UTF-8")
    }

    fn utf8(path: &Path) -> &str {
        path.to_str().expect("fixture paths should be UTF-8")
    }
}
