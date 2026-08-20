#![expect(
    clippy::absolute_paths,
    clippy::default_numeric_fallback,
    clippy::default_trait_access,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::pattern_type_mismatch,
    clippy::tests_outside_test_module,
    clippy::wildcard_enum_match_arm,
    reason = "public adapter scenarios use local numeric fixtures, no-cancellation defaults, and explicit panic diagnostics at crate scope"
)]

use core::{fmt, iter, time::Duration};
use std::{
    fs,
    io::Write as _,
    net::TcpListener,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
    thread,
};
use tempfile::TempDir;
use tiber_process_core::{
    AssignmentWorkflowProvenance, ConfiguredCommand, ConfiguredCommandCatalog, ConfiguredCommandId,
    FixedEnvironment, LiteralArgument, OutputBounds, ProcessInvocationId, ProcessRequest,
    RelativeWorkingDirectory,
};
use tiber_process_linux::{
    LinuxProcessAdapter, LinuxProcessAdapterConfig, LinuxProcessError, ProcessCancellation,
    ProcessDispatchOutcome,
};
use tiber_process_service::{
    AuthorizedProcess, ProcessEvent, ProcessExitStatus, ProcessFact, ProcessReconciliationOutcome,
    ProcessSpawnFailureCode, ProcessStream, ProcessUnknown, authorize_prepared_process,
    authorize_process_retirement, decide_process_request, decide_record_completed,
    decide_record_unknown, recover_process_reconciliation,
};
use tiber_workflow_core::{AssignmentId, EffectId, WorkflowId};

fn parsed<T, E: fmt::Debug>(result: Result<T, E>) -> T {
    result.expect("fixture value should satisfy its semantic boundary")
}

fn fixture_authority(
    program: &str,
    argv: &[&str],
    timeout: Duration,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> tiber_process_service::AuthorizedProcess {
    fixture_preparation(program, argv, timeout, stdout_bytes, stderr_bytes).0
}

fn fixture_preparation(
    program: &str,
    argv: &[&str],
    timeout: Duration,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> (AuthorizedProcess, Vec<ProcessEvent>, ProcessStream) {
    let command_id = parsed(ConfiguredCommandId::parse("adapter-test"));
    let request = ProcessRequest::for_invocation(
        command_id.clone(),
        parsed(ProcessInvocationId::parse("invocation-adapter")),
        AssignmentWorkflowProvenance::new(
            parsed(WorkflowId::parse("workflow-adapter")),
            parsed(AssignmentId::parse("assignment-adapter")),
            parsed(EffectId::parse("effect-adapter")),
        ),
    );
    let catalog = parsed(ConfiguredCommandCatalog::new([(
        command_id,
        parsed(ConfiguredCommand::new(
            PathBuf::from(program),
            argv.iter()
                .map(|argument| parsed(LiteralArgument::parse(argument)))
                .collect(),
            parsed(RelativeWorkingDirectory::parse(".")),
            parsed(FixedEnvironment::new(iter::empty::<(&str, &str)>())),
            timeout,
            parsed(OutputBounds::new(stdout_bytes, stderr_bytes)),
        )),
    )]));
    let stream =
        ProcessStream::for_request(&request).expect("request should form a process stream");
    let publication = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
        .expect("configured request should prepare");
    let (history, _) = publication.into_events_and_consistency_streams();
    let authority = authorize_prepared_process(&history, &stream, &request, &catalog)
        .expect("prepared history should authorize execution");
    (authority, history, stream)
}

fn journal_artifacts(state_root: &std::path::Path) -> Vec<PathBuf> {
    let mut artifacts = fs::read_dir(state_root)
        .expect("journal state root should remain readable")
        .map(|entry| {
            entry
                .expect("journal artifact should remain readable")
                .path()
        })
        .collect::<Vec<_>>();
    artifacts.sort();
    artifacts
}

#[test]
fn exact_signed_terminal_history_authorizes_idempotent_journal_retirement() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let (authority, mut history, stream) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf retired"],
        Duration::from_secs(1),
        100,
        100,
    );
    let adapter = LinuxProcessAdapter::new(config);
    let ProcessDispatchOutcome::Completed(completed) = adapter
        .execute(authority, &Default::default())
        .expect("configured command should complete")
    else {
        panic!("configured command should produce a completed receipt")
    };
    assert_eq!(journal_artifacts(state.path()).len(), 3);
    assert!(
        authorize_process_retirement(&history, &stream)
            .expect("prepared history should be valid")
            .is_none(),
        "recoverable prepared authority must retain every private artifact"
    );
    let launch_artifact = journal_artifacts(state.path())
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "launch")
        })
        .expect("completed operation should retain its private launch directory");
    let symlink_target = tempfile::tempdir().expect("external symlink target");
    let target_sentinel = symlink_target.path().join("must-remain");
    fs::write(&target_sentinel, b"owner data").expect("write symlink target sentinel");
    fs::remove_dir_all(&launch_artifact).expect("replace stale launch directory fixture");
    symlink(symlink_target.path(), &launch_artifact).expect("inject stale launch symlink fixture");
    let (other_authority, other_history, other_stream) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf retained"],
        Duration::from_secs(1),
        100,
        100,
    );
    assert!(matches!(
        adapter.execute(other_authority, &Default::default()),
        Ok(ProcessDispatchOutcome::Completed(_))
    ));
    assert_eq!(journal_artifacts(state.path()).len(), 6);
    assert!(
        authorize_process_retirement(&other_history, &other_stream)
            .expect("other prepared history should be valid")
            .is_none(),
        "another recoverable lifecycle must retain its artifacts"
    );

    let publication = decide_record_completed(
        &history,
        stream.clone(),
        completed
            .into_receipt()
            .expect("bounded completion should form a receipt"),
    )
    .expect("exact terminal should be publishable");
    let (terminal_events, _) = publication.into_events_and_consistency_streams();
    history.extend(terminal_events);
    let retirement = authorize_process_retirement(&history, &stream)
        .expect("signed terminal history should be valid")
        .expect("signed terminal history should mint retirement authority");

    adapter
        .retire(retirement)
        .expect("retirement should durably remove exact journal artifacts");
    assert_eq!(
        journal_artifacts(state.path()).len(),
        3,
        "retiring one exact identity must preserve another lifecycle"
    );
    assert_eq!(
        fs::read(target_sentinel).expect("retirement must not traverse a launch symlink"),
        b"owner data"
    );

    let repeated = authorize_process_retirement(&history, &stream)
        .expect("closed history should remain valid")
        .expect("closed history should reproduce exact retirement authority after a crash");
    adapter
        .retire(repeated)
        .expect("restart cleanup after prior retirement should be idempotent");
    assert_eq!(journal_artifacts(state.path()).len(), 3);
}

#[test]
fn many_signed_terminal_invocations_leave_no_private_journal_residue() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let adapter = LinuxProcessAdapter::new(config);

    let invocation_count: u8 = 32;
    for invocation in 0..invocation_count {
        let command = format!("printf invocation-{invocation}");
        let (authority, mut history, stream) = fixture_preparation(
            "/bin/sh",
            &["-c", &command],
            Duration::from_secs(1),
            100,
            100,
        );
        let ProcessDispatchOutcome::Completed(completed) = adapter
            .execute(authority, &Default::default())
            .expect("configured command should complete")
        else {
            panic!("configured command should produce a completed receipt")
        };
        let publication = decide_record_completed(
            &history,
            stream.clone(),
            completed
                .into_receipt()
                .expect("bounded completion should form a receipt"),
        )
        .expect("exact terminal should be publishable");
        history.extend(publication.into_events_and_consistency_streams().0);
        let retirement = authorize_process_retirement(&history, &stream)
            .expect("signed terminal history should be valid")
            .expect("signed terminal history should mint retirement authority");
        adapter
            .retire(retirement)
            .expect("terminal retirement should remain bounded and durable");
    }

    assert!(
        journal_artifacts(state.path()).is_empty(),
        "normal terminal traffic must not grow private process state without bound"
    );
}

#[expect(
    clippy::single_call_fn,
    reason = "the named containment fixture keeps its richer catalog shape out of the scenario body"
)]
fn fixture_authority_with_environment(
    program: &str,
    argv: &[&str],
    cwd: &str,
    environment: &[(&str, &str)],
) -> AuthorizedProcess {
    let command_id = parsed(ConfiguredCommandId::parse("containment-test"));
    let request = ProcessRequest::for_invocation(
        command_id.clone(),
        parsed(ProcessInvocationId::parse("invocation-containment")),
        AssignmentWorkflowProvenance::new(
            parsed(WorkflowId::parse("workflow-containment")),
            parsed(AssignmentId::parse("assignment-containment")),
            parsed(EffectId::parse("effect-containment")),
        ),
    );
    let catalog = parsed(ConfiguredCommandCatalog::new([(
        command_id,
        parsed(ConfiguredCommand::new(
            PathBuf::from(program),
            argv.iter()
                .map(|argument| parsed(LiteralArgument::parse(argument)))
                .collect(),
            parsed(RelativeWorkingDirectory::parse(cwd)),
            parsed(FixedEnvironment::new(environment.iter().copied())),
            Duration::from_secs(3),
            parsed(OutputBounds::new(4096, 4096)),
        )),
    )]));
    let stream =
        ProcessStream::for_request(&request).expect("request should form a process stream");
    let publication = decide_process_request(&[], stream.clone(), request.clone(), &catalog)
        .expect("configured containment request should prepare");
    let (history, _) = publication.into_events_and_consistency_streams();
    authorize_prepared_process(&history, &stream, &request, &catalog)
        .expect("prepared containment history should authorize execution")
}

fn fake_bubblewrap() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary fake Bubblewrap directory");
    let executable = directory.path().join("bwrap");
    fs::write(
        &executable,
        concat!(
            "#!/bin/sh\n",
            "repo=\n",
            "runtime=\n",
            "launcher=\n",
            "cwd=/workspace\n",
            "while [ \"$#\" -gt 0 ]; do\n",
            "  case \"$1\" in\n",
            "    --bind) case \"$3\" in /workspace) repo=$2 ;; /run/tiber) runtime=$2 ;; esac; shift 3 ;;\n",
            "    --chdir) cwd=$2; shift 2 ;;\n",
            "    --setenv) export \"$2=$3\"; shift 3 ;;\n",
            "    --) shift; break ;;\n",
            "    --ro-bind) case \"$3\" in /run/tiber/launcher) launcher=$2 ;; esac; shift 3 ;;\n",
            "    --dir|--proc|--dev|--tmpfs) shift 2 ;;\n",
            "    *) shift ;;\n",
            "  esac\n",
            "done\n",
            "case \"$cwd\" in /workspace*) cwd=\"$repo${cwd#/workspace}\" ;; esac\n",
            "export TIBER_LAUNCH_HANDSHAKE=\"$runtime/launched\"\n",
            "cd \"$cwd\" || exit 125\n",
            "if [ \"$1\" = /run/tiber/launcher ]; then shift; set -- \"$launcher\" \"$@\"; fi\n",
            "exec \"$@\"\n",
        ),
    )
    .expect("write fake Bubblewrap");
    let mut permissions = fs::metadata(&executable)
        .expect("fake Bubblewrap metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("make fake Bubblewrap executable");
    (directory, executable)
}

fn executable_on_path(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("test PATH"))
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{name} should be present in the pinned Nix shell"))
        .canonicalize()
        .expect("tool path should be canonical")
}

fn adapter_config(
    repository_root: PathBuf,
    state_root: PathBuf,
    bubblewrap: PathBuf,
    max_deadline: Duration,
) -> Result<LinuxProcessAdapterConfig, tiber_process_linux::LinuxProcessConfigurationError> {
    LinuxProcessAdapterConfig::new(
        repository_root,
        state_root,
        bubblewrap,
        PathBuf::from(env!("CARGO_BIN_EXE_tiber-process-launcher")),
        max_deadline,
    )
}

#[test]
fn consumed_authority_executes_direct_argv_with_bounded_raw_output() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");
    let adapter = LinuxProcessAdapter::new(config);
    let authority = fixture_authority(
        "/bin/sh",
        &["-c", "printf 'out'; printf 'err' >&2; exit 7"],
        Duration::from_secs(2),
        3,
        3,
    );

    let outcome = adapter
        .execute(authority, &Default::default())
        .expect("adapter execution should be definitive");

    let ProcessDispatchOutcome::Completed(completed) = outcome else {
        panic!("expected completed process")
    };
    assert_eq!(completed.status(), ProcessExitStatus::Exited(7));
    assert_eq!(completed.stdout().as_bytes(), b"out");
    assert_eq!(completed.stderr().as_bytes(), b"err");
    assert_eq!(format!("{completed:?}"), "CompletedProcess(<redacted>)");
}

#[test]
fn acknowledged_target_exit_125_is_a_definitive_completion() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", "exit 125"],
                Duration::from_secs(2),
                4096,
                4096,
            ),
            &Default::default(),
        )
        .expect("acknowledged target status should be definitive");

    let ProcessDispatchOutcome::Completed(completed) = outcome else {
        panic!("a valid target exit 125 must not collide with launcher protocol")
    };
    assert_eq!(completed.status(), ProcessExitStatus::Exited(125));
}

#[test]
fn adapter_rejects_an_authorized_deadline_above_its_configured_maximum() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(1),
    )
    .expect("trusted adapter configuration");

    let error = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority("/bin/true", &[], Duration::from_secs(2), 4096, 4096),
            &Default::default(),
        )
        .expect_err("an adapter deadline must reject rather than silently clamp authority");
    assert_eq!(error.code(), "process_linux_deadline_exceeded");
}

#[test]
fn deadline_kills_and_reaps_the_complete_process_tree() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap = executable_on_path("bwrap");
    let setsid = executable_on_path("setsid");
    let shell = executable_on_path("sh");
    let sleep = executable_on_path("sleep");
    let heartbeat = repository.path().join("descendant-heartbeat");
    let command = format!(
        "'{}' '{}' -c 'while :; do printf x >> /workspace/descendant-heartbeat; \"{}\" 0.02; done' & wait",
        setsid.display(),
        shell.display(),
        sleep.display()
    );
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let (authority, mut history, stream) = fixture_preparation(
        shell.to_string_lossy().as_ref(),
        &["-c", &command],
        Duration::from_secs(1),
        1_000_000,
        1_000_000,
    );

    let outcome = LinuxProcessAdapter::new(config.clone())
        .execute(authority, &Default::default())
        .expect("deadline should produce a definitive outcome");

    if let ProcessDispatchOutcome::Completed(completed) = &outcome {
        panic!(
            "expected timeout, completed with {:?}, stdout {:?}, stderr {:?}",
            completed.status(),
            String::from_utf8_lossy(completed.stdout().as_bytes()),
            String::from_utf8_lossy(completed.stderr().as_bytes())
        );
    }
    assert!(matches!(outcome, ProcessDispatchOutcome::TimedOut(_)));
    let stopped_size = fs::metadata(&heartbeat)
        .expect("descendant should publish heartbeats before timeout")
        .len();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::metadata(&heartbeat)
            .expect("heartbeat remains inspectable")
            .len(),
        stopped_size,
        "descendant must stop running before timeout returns"
    );
    let timed_out_identity = match &outcome {
        ProcessDispatchOutcome::TimedOut(terminal) => terminal.identity().clone(),
        _ => panic!("timeout outcome should carry its exact identity"),
    };
    let publication = decide_record_unknown(
        &history,
        stream.clone(),
        ProcessUnknown::new(timed_out_identity),
    )
    .expect("simulate crash before durable timeout publication");
    let (unknown_events, _) = publication.into_events_and_consistency_streams();
    history.extend(unknown_events);
    let capability = recover_process_reconciliation(&history, &stream)
        .expect("unknown timeout history")
        .expect("unknown timeout should mint read-only reconciliation");
    let reconciled = LinuxProcessAdapter::new(config)
        .reconcile(capability)
        .expect("durable timeout should reconcile definitively");
    assert!(matches!(
        reconciled.outcome(),
        ProcessReconciliationOutcome::DefinitelyNotCompleted
    ));
}

#[test]
fn explicit_cancellation_kills_and_reaps_the_complete_process_tree() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap = executable_on_path("bwrap");
    let setsid = executable_on_path("setsid");
    let shell = executable_on_path("sh");
    let sleep = executable_on_path("sleep");
    let heartbeat = repository.path().join("cancel-heartbeat");
    let command = format!(
        "'{}' '{}' -c 'while :; do printf x >> /workspace/cancel-heartbeat; \"{}\" 0.02; done' & wait",
        setsid.display(),
        shell.display(),
        sleep.display()
    );
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(10),
    )
    .expect("trusted adapter configuration");
    let authority = fixture_authority(
        shell.to_string_lossy().as_ref(),
        &["-c", &command],
        Duration::from_secs(10),
        1_000_000,
        1_000_000,
    );
    let cancellation = ProcessCancellation::default();
    let execution_cancellation = cancellation.clone();
    let handle = thread::spawn(move || {
        LinuxProcessAdapter::new(config).execute(authority, &execution_cancellation)
    });
    let started = std::time::Instant::now();
    while !heartbeat.exists() && started.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        heartbeat.exists(),
        "descendant should start before cancellation"
    );
    let acknowledgment_started = std::time::Instant::now();
    while !journal_artifacts(state.path()).iter().any(|artifact| {
        artifact
            .extension()
            .is_some_and(|extension| extension == "launch")
            && artifact.join("launched").exists()
    }) && acknowledgment_started.elapsed() < Duration::from_secs(2)
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        journal_artifacts(state.path()).iter().any(|artifact| {
            artifact
                .extension()
                .is_some_and(|extension| extension == "launch")
                && artifact.join("launched").exists()
        }),
        "launcher should durably acknowledge the target before cancellation"
    );

    cancellation.cancel();
    let outcome = handle
        .join()
        .expect("adapter thread should not panic")
        .expect("cancellation should produce a definitive outcome");

    assert!(
        matches!(outcome, ProcessDispatchOutcome::Cancelled(_)),
        "expected cancellation, received {outcome:?}"
    );
    let stopped_size = fs::metadata(&heartbeat).expect("heartbeat metadata").len();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::metadata(&heartbeat)
            .expect("heartbeat remains inspectable")
            .len(),
        stopped_size,
        "descendant must stop running before cancellation returns"
    );
}

#[test]
fn cancellation_before_launch_acknowledgment_is_outcome_unknown() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let launcher_directory = tempfile::tempdir().expect("temporary launcher directory");
    let launcher = launcher_directory.path().join("launcher");
    fs::write(
        &launcher,
        concat!(
            "#!/bin/sh\n",
            "while [ \"$1\" = --env ]; do shift 3; done\n",
            "[ \"$1\" = -- ] || exit 125\n",
            "shift\n",
            "\"$@\" &\n",
            "wait\n",
        ),
    )
    .expect("write delayed-acknowledgment launcher");
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))
        .expect("make delayed-acknowledgment launcher executable");
    let config = LinuxProcessAdapterConfig::new(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        launcher,
        Duration::from_secs(10),
    )
    .expect("trusted adapter configuration");
    let authority = fixture_authority(
        "/bin/sh",
        &[
            "-c",
            "printf started > launched-target; while :; do :; done",
        ],
        Duration::from_secs(10),
        4096,
        4096,
    );
    let cancellation = ProcessCancellation::default();
    let execution_cancellation = cancellation.clone();
    let handle = thread::spawn(move || {
        LinuxProcessAdapter::new(config).execute(authority, &execution_cancellation)
    });
    let marker = repository.path().join("launched-target");
    let started = std::time::Instant::now();
    while !marker.exists() && started.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "target should run before cancellation");

    cancellation.cancel();
    let outcome = handle
        .join()
        .expect("adapter thread should not panic")
        .expect("uncertain cancellation should remain a typed outcome");

    assert!(matches!(outcome, ProcessDispatchOutcome::OutcomeUnknown(_)));
}

#[test]
fn acknowledgment_failure_after_target_launch_remains_unknown_without_rerun() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let launcher_directory = tempfile::tempdir().expect("temporary launcher directory");
    let launcher = launcher_directory.path().join("launcher");
    fs::write(
        &launcher,
        concat!(
            "#!/bin/sh\n",
            "while [ \"$1\" = --env ]; do shift 3; done\n",
            "[ \"$1\" = -- ] || exit 126\n",
            "shift\n",
            "\"$@\"\n",
            "printf launched > \"$TIBER_LAUNCH_HANDSHAKE\"\n",
            "exit 125\n",
        ),
    )
    .expect("write post-launch acknowledgment-failure launcher");
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))
        .expect("make acknowledgment-failure launcher executable");
    let config = LinuxProcessAdapterConfig::new(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        launcher,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");
    let (authority, mut history, stream) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf x >> launch-marker"],
        Duration::from_secs(2),
        4096,
        4096,
    );

    let outcome = LinuxProcessAdapter::new(config.clone())
        .execute(authority, &Default::default())
        .expect("post-launch acknowledgment failure should remain typed");

    let ProcessDispatchOutcome::OutcomeUnknown(unknown) = outcome else {
        panic!("a target that ran without durable acknowledgment must remain unknown")
    };
    let marker = repository.path().join("launch-marker");
    assert_eq!(fs::read(&marker).expect("target launch marker"), b"x");
    let publication = decide_record_unknown(&history, stream.clone(), unknown)
        .expect("unknown launch history should be publishable");
    let (unknown_events, _) = publication.into_events_and_consistency_streams();
    history.extend(unknown_events);
    let capability = recover_process_reconciliation(&history, &stream)
        .expect("unknown launch history")
        .expect("unknown launch should mint read-only reconciliation");
    let reconciled = LinuxProcessAdapter::new(config.clone())
        .reconcile(capability)
        .expect("unknown launch should reconcile without execution");
    assert!(matches!(
        reconciled.outcome(),
        ProcessReconciliationOutcome::StillUnknown
    ));
    let replay = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", "printf x >> launch-marker"],
                Duration::from_secs(2),
                4096,
                4096,
            ),
            &Default::default(),
        )
        .expect("identity reuse should return the durable ambiguity");
    assert!(matches!(replay, ProcessDispatchOutcome::OutcomeUnknown(_)));
    assert_eq!(fs::read(marker).expect("single target launch marker"), b"x");
}

#[test]
fn unacknowledged_nonprotocol_launcher_exit_after_target_launch_is_outcome_unknown() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let launcher_directory = tempfile::tempdir().expect("temporary launcher directory");
    let launcher = launcher_directory.path().join("launcher");
    fs::write(
        &launcher,
        concat!(
            "#!/bin/sh\n",
            "while [ \"$1\" = --env ]; do shift 3; done\n",
            "[ \"$1\" = -- ] || exit 126\n",
            "shift\n",
            "\"$@\"\n",
            "exit 7\n",
        ),
    )
    .expect("write unacknowledged launcher fixture");
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))
        .expect("make unacknowledged launcher executable");
    let config = LinuxProcessAdapterConfig::new(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        launcher,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");

    let (authority, mut history, stream) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf launched > nonprotocol-launch-marker"],
        Duration::from_secs(2),
        4096,
        4096,
    );
    let outcome = LinuxProcessAdapter::new(config.clone())
        .execute(authority, &Default::default())
        .expect("unacknowledged launch should remain a typed outcome");

    let ProcessDispatchOutcome::OutcomeUnknown(unknown) = outcome else {
        panic!("a nonprotocol exit without acknowledgment must remain unknown")
    };
    assert_eq!(
        fs::read(repository.path().join("nonprotocol-launch-marker"))
            .expect("target launch marker"),
        b"launched"
    );
    let publication = decide_record_unknown(&history, stream.clone(), unknown)
        .expect("unknown launch history should be publishable");
    let (unknown_events, _) = publication.into_events_and_consistency_streams();
    history.extend(unknown_events);
    let capability = recover_process_reconciliation(&history, &stream)
        .expect("unknown launch history")
        .expect("unknown launch should mint read-only reconciliation");
    let reconciled = LinuxProcessAdapter::new(config)
        .reconcile(capability)
        .expect("unacknowledged launch should reconcile without execution");
    assert!(matches!(
        reconciled.outcome(),
        ProcessReconciliationOutcome::StillUnknown
    ));
}

#[test]
fn output_cap_kills_the_tree_and_returns_only_a_typed_bounded_failure() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap = executable_on_path("bwrap");
    let setsid = executable_on_path("setsid");
    let shell = executable_on_path("sh");
    let sleep = executable_on_path("sleep");
    let heartbeat = repository.path().join("bounded-heartbeat");
    let command = format!(
        "'{}' '{}' -c 'while :; do printf x >> /workspace/bounded-heartbeat; \"{}\" 0.02; done' & while [ ! -s /workspace/bounded-heartbeat ]; do \"{}\" 0.01; done; printf 0123456789; wait",
        setsid.display(),
        shell.display(),
        sleep.display(),
        sleep.display()
    );
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");
    let authority = fixture_authority(
        shell.to_string_lossy().as_ref(),
        &["-c", &command],
        Duration::from_secs(4),
        4,
        1_000_000,
    );

    let adapter = LinuxProcessAdapter::new(config.clone());
    let outcome = adapter
        .execute(authority, &Default::default())
        .expect("stdout termination should return typed reconciliation authority");

    assert!(matches!(
        outcome,
        ProcessDispatchOutcome::OutputLimitExceeded(_)
    ));
    let stopped_size = fs::metadata(&heartbeat)
        .expect("descendant heartbeat")
        .len();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::metadata(&heartbeat)
            .expect("heartbeat remains inspectable")
            .len(),
        stopped_size,
        "over-cap return must imply descendant shutdown"
    );
    let replay = LinuxProcessAdapter::new(config).execute(
        fixture_authority(
            shell.to_string_lossy().as_ref(),
            &["-c", &command],
            Duration::from_secs(4),
            4,
            1_000_000,
        ),
        &Default::default(),
    );
    assert_eq!(
        replay.expect_err("definitive output failure must reject identity reuse"),
        LinuxProcessError::OperationAlreadyTerminal
    );
}

#[test]
fn stderr_has_an_independent_exact_output_cap() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", "printf 12345 >&2"],
                Duration::from_secs(1),
                100,
                4,
            ),
            &Default::default(),
        )
        .expect("stderr termination should return typed reconciliation authority");

    assert!(matches!(
        outcome,
        ProcessDispatchOutcome::OutputLimitExceeded(_)
    ));
}

#[test]
fn stdout_and_stderr_are_drained_concurrently_without_pipe_deadlock() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(5),
    )
    .expect("trusted adapter configuration");
    let script = concat!(
        "i=0; while [ $i -lt 10000 ]; do printf 0123456789; i=$((i+1)); done; ",
        "i=0; while [ $i -lt 10000 ]; do printf 9876543210 >&2; i=$((i+1)); done"
    );

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", script],
                Duration::from_secs(4),
                100_000,
                100_000,
            ),
            &Default::default(),
        )
        .expect("both streams should drain without blocking the child");

    let ProcessDispatchOutcome::Completed(completed) = outcome else {
        panic!("large dual-stream process should complete")
    };
    assert_eq!(completed.stdout().as_bytes().len(), 100_000);
    assert_eq!(completed.stderr().as_bytes().len(), 100_000);
}

#[test]
fn bubblewrap_spawn_failure_is_definitive_typed_and_content_free() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap_directory = tempfile::tempdir().expect("temporary Bubblewrap directory");
    let bubblewrap = bubblewrap_directory.path().join("bwrap");
    fs::write(&bubblewrap, b"not executable").expect("write non-executable Bubblewrap fixture");
    fs::set_permissions(&bubblewrap, fs::Permissions::from_mode(0o600))
        .expect("remove execute permission");
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let secret_marker = "raw-command-secret-marker";
    let adapter = LinuxProcessAdapter::new(config.clone());

    let outcome = adapter
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", secret_marker],
                Duration::from_secs(1),
                100,
                100,
            ),
            &Default::default(),
        )
        .expect("spawn refusal is a definitive adapter outcome");

    let ProcessDispatchOutcome::SpawnFailed(failure) = outcome else {
        panic!("non-executable Bubblewrap should be a spawn failure")
    };
    assert_eq!(failure.code(), ProcessSpawnFailureCode::PermissionDenied);
    assert!(!format!("{failure:?}").contains(secret_marker));
    let replay = LinuxProcessAdapter::new(config).execute(
        fixture_authority(
            "/bin/sh",
            &["-c", secret_marker],
            Duration::from_secs(1),
            100,
            100,
        ),
        &Default::default(),
    );
    assert_eq!(
        replay.expect_err("durable spawn failure must reject identity reuse"),
        LinuxProcessError::OperationAlreadyTerminal
    );
}

#[test]
fn malformed_bubblewrap_is_classified_as_executable_unavailable() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap_directory = tempfile::tempdir().expect("temporary Bubblewrap directory");
    let bubblewrap = bubblewrap_directory.path().join("bwrap");
    fs::write(&bubblewrap, b"not an executable format")
        .expect("write malformed Bubblewrap fixture");
    fs::set_permissions(&bubblewrap, fs::Permissions::from_mode(0o700))
        .expect("make malformed fixture executable");
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", "printf never"],
                Duration::from_secs(1),
                100,
                100,
            ),
            &Default::default(),
        )
        .expect("malformed executable is a definitive spawn outcome");

    let ProcessDispatchOutcome::SpawnFailed(failure) = outcome else {
        panic!("malformed Bubblewrap should fail before launch")
    };
    assert_eq!(
        failure.code(),
        ProcessSpawnFailureCode::ExecutableUnavailable
    );
}

#[test]
fn unacknowledged_bubblewrap_setup_exit_is_outcome_unknown() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let bubblewrap_directory = tempfile::tempdir().expect("temporary Bubblewrap directory");
    let bubblewrap = bubblewrap_directory.path().join("bwrap");
    fs::write(&bubblewrap, b"#!/bin/sh\nexit 1\n").expect("write failing Bubblewrap fixture");
    fs::set_permissions(&bubblewrap, fs::Permissions::from_mode(0o700))
        .expect("make failing Bubblewrap executable");
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", "printf must-not-run"],
                Duration::from_secs(1),
                100,
                100,
            ),
            &Default::default(),
        )
        .expect("unacknowledged launcher setup exit should remain typed");

    assert!(matches!(outcome, ProcessDispatchOutcome::OutcomeUnknown(_)));
}

#[test]
fn target_exec_failure_cannot_masquerade_as_target_completion() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        executable_on_path("bwrap"),
        Duration::from_secs(2),
    )
    .expect("trusted real Bubblewrap configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority(
                "/definitely/missing/tiber-command",
                &[],
                Duration::from_secs(1),
                100,
                100,
            ),
            &Default::default(),
        )
        .expect("target exec refusal is definitive");

    assert!(matches!(outcome, ProcessDispatchOutcome::SpawnFailed(_)));
}

#[test]
fn real_bubblewrap_enforces_mount_network_cwd_and_environment_boundaries() {
    let repository = tempfile::tempdir().expect("temporary repository");
    fs::create_dir_all(repository.path().join("subdir")).expect("repository working directory");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let host_secret_directory = tempfile::tempdir().expect("host-only secret directory");
    let host_secret = host_secret_directory.path().join("secret");
    fs::write(&host_secret, b"host-only").expect("host-only fixture");
    let listener = TcpListener::bind("127.0.0.1:0").expect("host loopback listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking host listener");
    let address = listener.local_addr().expect("listener address");
    let server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            match listener.accept() {
                Ok((mut stream, _peer)) => {
                    let _write_result = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_source) => return false,
            }
        }
        false
    });
    let curl = executable_on_path("curl");
    let env = executable_on_path("env");
    let shell = executable_on_path("sh");
    let command = format!(
        "printf '%s\\n%s\\n' \"$PWD\" \"$TOKEN\"; [ -e '{}' ] && printf host-leak; '{}' | while IFS= read -r entry; do case \"$entry\" in HOME=*|PATH=*) printf env-leak ;; esac; done; '{}' --silent --max-time 0.5 'http://{}' >/dev/null 2>&1 && printf network-leak || :",
        host_secret.display(),
        env.display(),
        curl.display(),
        address
    );
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        executable_on_path("bwrap"),
        Duration::from_secs(4),
    )
    .expect("trusted real Bubblewrap configuration");

    let outcome = LinuxProcessAdapter::new(config)
        .execute(
            fixture_authority_with_environment(
                shell.to_string_lossy().as_ref(),
                &["-c", &command],
                "subdir",
                &[("TOKEN", "fixed")],
            ),
            &Default::default(),
        )
        .expect("contained command should complete");

    let ProcessDispatchOutcome::Completed(completed) = outcome else {
        panic!("real Bubblewrap containment command should complete")
    };
    assert_eq!(completed.stdout().as_bytes(), b"/workspace/subdir\nfixed\n");
    assert!(
        !server
            .join()
            .expect("host listener thread should not panic")
    );
}

#[test]
fn restart_uncertainty_only_mints_exact_read_only_reconciliation() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let marker = repository.path().join("launch-marker");
    let command = format!("printf x >> '{}'", marker.display());
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let (authority, mut history, stream) = fixture_preparation(
        "/bin/sh",
        &["-c", &command],
        Duration::from_secs(1),
        100,
        100,
    );
    let prepared_identity = match history.get(1).map(ProcessEvent::fact) {
        Some(ProcessFact::Prepared(identity)) => identity.clone(),
        _ => panic!("fixture history should contain a prepared identity"),
    };
    let adapter = LinuxProcessAdapter::new(config.clone());
    assert!(matches!(
        adapter.execute(authority, &Default::default()),
        Ok(ProcessDispatchOutcome::Completed(_))
    ));
    let journal = fs::read_dir(state.path())
        .expect("generated journal directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("completed operation should have terminal journal evidence");
    fs::remove_file(journal).expect("simulate terminal-evidence loss after launch");

    let restart = LinuxProcessAdapter::new(config.clone())
        .execute(
            fixture_authority(
                "/bin/sh",
                &["-c", &command],
                Duration::from_secs(1),
                100,
                100,
            ),
            &Default::default(),
        )
        .expect("reserved operation without terminal evidence is uncertain");
    let ProcessDispatchOutcome::OutcomeUnknown(unknown) = restart else {
        panic!("restart must not redispatch an uncertain operation")
    };
    assert_eq!(unknown.identity(), &prepared_identity);
    assert_eq!(fs::read(&marker).expect("launch marker"), b"x");

    let unknown_publication = decide_record_unknown(
        &history,
        stream.clone(),
        ProcessUnknown::new(prepared_identity),
    )
    .expect("service should durably represent adapter uncertainty");
    let (unknown_events, _) = unknown_publication.into_events_and_consistency_streams();
    history.extend(unknown_events);
    let capability = recover_process_reconciliation(&history, &stream)
        .expect("unknown history should be valid")
        .expect("unreconciled unknown should mint one read-only capability");

    let reconciled = LinuxProcessAdapter::new(config)
        .reconcile(capability)
        .expect("reconciliation should inspect private state read-only");

    assert!(matches!(
        reconciled.outcome(),
        ProcessReconciliationOutcome::StillUnknown
    ));
    assert_eq!(fs::read(&marker).expect("launch marker"), b"x");
}

#[test]
fn unsupported_private_journal_schema_fails_reconciliation_closed() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let (authority, mut history, stream) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf done"],
        Duration::from_secs(1),
        100,
        100,
    );
    let prepared_identity = match history.get(1).map(ProcessEvent::fact) {
        Some(ProcessFact::Prepared(identity)) => identity.clone(),
        _ => panic!("fixture history should contain a prepared identity"),
    };
    assert!(matches!(
        LinuxProcessAdapter::new(config.clone()).execute(authority, &Default::default()),
        Ok(ProcessDispatchOutcome::Completed(_))
    ));
    let journal = fs::read_dir(state.path())
        .expect("generated journal directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("completed operation should have a journal");
    let mut generated: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal).expect("read generated journal"))
            .expect("generated journal should be JSON");
    generated["schema_version"] = serde_json::json!(999);
    fs::write(
        &journal,
        serde_json::to_vec(&generated).expect("encode altered generated journal"),
    )
    .expect("inject unsupported schema as a restart fixture");
    let publication = decide_record_unknown(
        &history,
        stream.clone(),
        ProcessUnknown::new(prepared_identity),
    )
    .expect("service should represent restart uncertainty");
    let (unknown_events, _) = publication.into_events_and_consistency_streams();
    history.extend(unknown_events);
    let capability = recover_process_reconciliation(&history, &stream)
        .expect("unknown history should be valid")
        .expect("unknown history should mint read-only capability");

    let failure = LinuxProcessAdapter::new(config)
        .reconcile(capability)
        .expect_err("unsupported private schema must fail closed");

    assert_eq!(failure, LinuxProcessError::StateUnavailable);
}

#[test]
fn private_journal_for_another_process_identity_fails_reconciliation_closed() {
    let repository = tempfile::tempdir().expect("temporary repository");
    let state = tempfile::tempdir().expect("temporary state root");
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))
        .expect("private state root permissions");
    let (_bubblewrap_directory, bubblewrap) = fake_bubblewrap();
    let config = adapter_config(
        repository.path().to_path_buf(),
        state.path().to_path_buf(),
        bubblewrap,
        Duration::from_secs(2),
    )
    .expect("trusted adapter configuration");
    let (authority_a, mut history_a, stream_a) = fixture_preparation(
        "/bin/sh",
        &["-c", "printf process-a"],
        Duration::from_secs(1),
        100,
        100,
    );
    let identity_a = match history_a.get(1).map(ProcessEvent::fact) {
        Some(ProcessFact::Prepared(identity)) => identity.clone(),
        _ => panic!("fixture A should contain a prepared identity"),
    };
    assert!(matches!(
        LinuxProcessAdapter::new(config.clone()).execute(authority_a, &Default::default()),
        Ok(ProcessDispatchOutcome::Completed(_))
    ));
    let journal_a = fs::read_dir(state.path())
        .expect("journal directory after A")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("process A journal");
    let authority_b = fixture_authority(
        "/bin/sh",
        &["-c", "printf process-b"],
        Duration::from_secs(1),
        100,
        100,
    );
    assert!(matches!(
        LinuxProcessAdapter::new(config.clone()).execute(authority_b, &Default::default()),
        Ok(ProcessDispatchOutcome::Completed(_))
    ));
    let journal_b = fs::read_dir(state.path())
        .expect("journal directory after B")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .find(|path| path != &journal_a)
        .expect("process B journal");
    fs::copy(&journal_b, &journal_a).expect("inject B's generated receipt at A's path");
    let publication = decide_record_unknown(
        &history_a,
        stream_a.clone(),
        ProcessUnknown::new(identity_a),
    )
    .expect("service should represent A uncertainty");
    let (unknown_events, _) = publication.into_events_and_consistency_streams();
    history_a.extend(unknown_events);
    let capability = recover_process_reconciliation(&history_a, &stream_a)
        .expect("A unknown history should be valid")
        .expect("A should mint a read-only capability");

    let failure = LinuxProcessAdapter::new(config)
        .reconcile(capability)
        .expect_err("B's receipt must not reconcile A");

    assert_eq!(failure, LinuxProcessError::StateUnavailable);
}
