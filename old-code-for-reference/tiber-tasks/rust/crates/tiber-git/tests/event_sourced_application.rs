use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn task_state_uses_only_the_tiber_event_branch() {
    let dir = TempDir::new().unwrap();
    let repository = dir.path().join("repo");
    git(dir.path(), ["init", repository.to_str().unwrap()]);
    git(&repository, ["config", "user.name", "Tiber Test"]);
    git(
        &repository,
        ["config", "user.email", "tiber@example.invalid"],
    );
    git(&repository, ["config", "commit.gpgsign", "false"]);

    tiber_git::init_repository_at(&repository).unwrap();
    tiber_git::init_repository_at(&repository).unwrap();
    let created = tiber_git::create_task_at(&repository, "Event sourced task").unwrap();
    let tasks = tiber_git::list_tasks_at(&repository).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].path, created.path);
    assert!(show_ref(&repository, "refs/heads/tiber"));
    assert!(!show_ref(&repository, "refs/heads/tasks"));
    assert!(!show_ref(&repository, "refs/heads/tiber-coordination"));
    let tree = output(
        &repository,
        ["ls-tree", "-r", "--name-only", "refs/heads/tiber"],
    );
    assert!(tree
        .lines()
        .all(|path| path.starts_with("eventstore/events/")));
    let events = event_documents(&repository);
    assert_eq!(
        events
            .matches("\"event\":\"repository_initialized\"")
            .count(),
        1,
        "replayed initialization intent must not emit a second durable fact"
    );
    assert!(events.contains("\"event\":\"task_created\""));
}

#[test]
fn create_task_records_board_authority_facts_and_enforces_backlog_capacity() {
    let dir = TempDir::new().unwrap();
    let repository = dir.path().join("repo");
    git(dir.path(), ["init", repository.to_str().unwrap()]);
    git(&repository, ["config", "user.name", "Tiber Test"]);
    git(
        &repository,
        ["config", "user.email", "tiber@example.invalid"],
    );
    git(&repository, ["config", "commit.gpgsign", "false"]);
    std::fs::write(
        repository.join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .unwrap();

    tiber_git::init_repository_at(&repository).unwrap();
    let created = tiber_git::create_task_at(&repository, "First queued task").unwrap();
    let refusal = tiber_git::create_task_at(&repository, "Second queued task").unwrap_err();

    assert!(refusal
        .to_string()
        .contains("tiber.backlog_capacity_exceeded queued=1 max_queued=1"));
    assert_eq!(
        tiber_git::list_tasks_at(&repository).unwrap()[0].path,
        created.path
    );
    assert!(has_event_in_stream(
        &repository,
        "task_created",
        "tiber:board"
    ));
}

#[test]
fn task_commands_ignore_legacy_refs_and_root_tasks_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let repository = dir.path().join("repo");
    git(dir.path(), ["init", repository.to_str().unwrap()]);
    git(&repository, ["config", "user.name", "Tiber Test"]);
    git(
        &repository,
        ["config", "user.email", "tiber@example.invalid"],
    );
    git(&repository, ["config", "commit.gpgsign", "false"]);
    std::fs::write(repository.join("legacy.md"), b"legacy branch bytes\n\0tail").unwrap();
    git(&repository, ["add", "legacy.md"]);
    git(&repository, ["commit", "-m", "seed legacy refs"]);
    git(&repository, ["branch", "tasks"]);
    git(&repository, ["branch", "tiber-coordination"]);
    let tasks_ref = output(&repository, ["rev-parse", "refs/heads/tasks"]);
    let coordination_ref = output(&repository, ["rev-parse", "refs/heads/tiber-coordination"]);

    std::fs::create_dir(repository.join(".tasks")).unwrap();
    let root_tasks = b"root task bytes\n\0not markdown";
    std::fs::write(repository.join(".tasks/opaque.bin"), root_tasks).unwrap();

    tiber_git::init_repository_at(&repository).unwrap();
    let created = tiber_git::create_task_at(&repository, "Typed task").unwrap();
    tiber_git::transition_task_at(&repository, &created.path, "in-progress").unwrap();
    tiber_git::list_tasks_at(&repository).unwrap();

    assert_eq!(
        output(&repository, ["rev-parse", "refs/heads/tasks"]),
        tasks_ref
    );
    assert_eq!(
        output(&repository, ["rev-parse", "refs/heads/tiber-coordination"]),
        coordination_ref
    );
    assert_eq!(
        std::fs::read(repository.join(".tasks/opaque.bin")).unwrap(),
        root_tasks
    );
    assert!(
        has_event_in_stream(&repository, "task_transitioned", "tiber:board"),
        "expected the transition fact in the board authority stream"
    );
}

#[test]
fn ci_recovery_uses_the_same_tiber_event_branch() {
    let dir = TempDir::new().unwrap();
    let repository = dir.path().join("repo");
    let origin = dir.path().join("origin.git");
    git(dir.path(), ["init", "--bare", origin.to_str().unwrap()]);
    git(dir.path(), ["init", repository.to_str().unwrap()]);
    git(&repository, ["config", "user.name", "Tiber Test"]);
    git(
        &repository,
        ["config", "user.email", "tiber@example.invalid"],
    );
    git(&repository, ["config", "commit.gpgsign", "false"]);
    git(
        &repository,
        ["remote", "add", "origin", origin.to_str().unwrap()],
    );
    assert!(
        !tiber_git::ci_recovery_hold_at(&repository).unwrap(),
        "an initialized repository without an incident must not block delivery"
    );
    std::env::set_var("TIBER_CLAIM_SESSION", "event-test-session");
    std::env::set_var("TIBER_CLAIM_HOST", "event-test-host");
    let claim = tiber_git::claim_ci_recovery_at(
        &repository,
        tiber_git::CiRecoveryTrigger {
            run_id: "123".into(),
            run_url: "https://example.invalid/runs/123".into(),
            failed_sha: "abcdef0123456789".into(),
            workflow: "CI".into(),
            git_ref: "refs/heads/main".into(),
        },
    )
    .unwrap();
    let status = tiber_git::ci_recovery_status_at(&repository).unwrap();
    assert_eq!(status.incident_id, claim.incident_id);
    assert!(
        tiber_git::ci_recovery_hold_at(&repository).unwrap(),
        "the Development Discipline delivery gate reads this unresolved Tiber fact"
    );
    assert!(show_ref(&repository, "refs/remotes/origin/tiber"));
    assert!(!show_ref(
        &repository,
        "refs/remotes/origin/tiber-coordination"
    ));
    let events = event_documents(&repository);
    assert!(events.contains("\"event\":\"ci_recovery_claimed\""));
    assert!(
        !events.contains("\"event\":\"recovery_state_published\""),
        "new CI recovery writes must not append the legacy duplicate snapshot event"
    );

    std::env::set_var("TIBER_CLAIM_SESSION", "event-test-helper-session");
    std::env::set_var("TIBER_CLAIM_HOST", "event-test-helper-host");
    let joined = tiber_git::claim_ci_recovery_at(
        &repository,
        tiber_git::CiRecoveryTrigger {
            run_id: "123".into(),
            run_url: "https://example.invalid/runs/123".into(),
            failed_sha: "abcdef0123456789".into(),
            workflow: "CI".into(),
            git_ref: "refs/heads/main".into(),
        },
    )
    .unwrap();
    assert!(matches!(joined.role, tiber_git::CiRecoveryRole::Waiting));
    let joined_status = tiber_git::ci_recovery_status_at(&repository).unwrap();
    assert_eq!(joined_status.participants.len(), 2);
    assert!(joined_status.participants.iter().any(|participant| {
        participant.host == "event-test-helper-host"
            && participant.session == "event-test-helper-session"
    }));
    assert!(event_documents(&repository).contains("\"event\":\"ci_recovery_joined\""));
}

#[test]
fn discarded_pending_transaction_requires_reissue_without_lingering_blocker() {
    let dir = TempDir::new().unwrap();
    let repository = dir.path().join("repo");
    let origin = dir.path().join("origin.git");
    git(dir.path(), ["init", "--bare", origin.to_str().unwrap()]);
    git(dir.path(), ["init", repository.to_str().unwrap()]);
    git(&repository, ["config", "user.name", "Tiber Test"]);
    git(
        &repository,
        ["config", "user.email", "tiber@example.invalid"],
    );
    git(&repository, ["config", "commit.gpgsign", "false"]);
    git(
        &repository,
        ["remote", "add", "origin", origin.to_str().unwrap()],
    );
    tiber_git::init_repository_at(&repository).unwrap();

    let hook = origin.join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(tiber_git::create_task_at(&repository, "Discarded task").is_err());
    std::fs::remove_file(hook).unwrap();

    let base = output(&repository, ["rev-parse", "refs/remotes/origin/tiber"]);
    let tree = output(
        &repository,
        ["rev-parse", &format!("{}^{{tree}}", base.trim())],
    );
    let rival = output(
        &repository,
        ["commit-tree", tree.trim(), "-p", base.trim(), "-m", "rival"],
    );
    git(
        &repository,
        [
            "push",
            "origin",
            &format!("{}:refs/heads/tiber", rival.trim()),
        ],
    );

    let error = tiber_git::sync_repository_at(&repository)
        .unwrap_err()
        .to_string();
    assert!(error.contains("event_transaction_discarded"));
    assert!(error.contains("reissue_required=true"));
    assert!(!repository.join(".git/tiber/workflow-blocker.json").exists());
    tiber_git::create_task_at(&repository, "Reissued task").unwrap();
    assert_eq!(tiber_git::list_tasks_at(&repository).unwrap().len(), 1);
}

fn event_documents(repository: &Path) -> String {
    let reference = if show_ref(repository, "refs/heads/tiber") {
        "refs/heads/tiber"
    } else {
        "refs/remotes/origin/tiber"
    };
    output(
        repository,
        ["grep", "-h", "event", reference, "--", "eventstore/events"],
    )
}

fn has_event_in_stream(repository: &Path, name: &str, stream: &str) -> bool {
    event_documents(repository).lines().any(|line| {
        let Ok(document) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        document["stream_id"] == stream && document["event_data"]["event"] == name
    })
}

fn show_ref(repository: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["show-ref", "--verify", reference])
        .status()
        .unwrap()
        .success()
}

fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn output<const N: usize>(repository: &Path, arguments: [&str; N]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
