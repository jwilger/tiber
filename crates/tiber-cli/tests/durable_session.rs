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
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        process::{Child, Command, Output, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use eventcore_fs::FileEventStore;
    use eventcore_types::{BatchSize, EventStore as _, StreamPattern, StreamVersion, StreamWrites};
    use tempfile::TempDir;
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
        state_home: PathBuf,
        task_id: String,
        turn_completed: PathBuf,
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
            let initialized = directory.path().join("initialized");
            let oversized = directory.path().join("oversized");
            let terminal_capture = directory.path().join("terminal-capture");
            let invocations = directory.path().join("invocations");

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
                state_home,
                task_id: String::new(),
                turn_completed,
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
            Command::new("script")
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
                .env("TIBER_FIXTURE_OVERSIZED_SENTINEL", &self.oversized)
                .env("TIBER_FIXTURE_INVOCATIONS", &self.invocations)
                .env("XDG_STATE_HOME", &self.state_home)
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
        clippy::single_call_fn,
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
            assert!(Instant::now() < deadline, "fixture turn should complete");
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
