pub mod support;

use std::fs;
use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn init_creates_only_the_tiber_event_branch() {
    let repo = TempRepo::initialized();

    let output = repo.tiber(["init"]);

    assert_success(output);
    assert_success(repo.git_output(["show-ref", "--verify", "refs/heads/tiber"]));
    assert!(!repo
        .git_output(["show-ref", "--verify", "refs/heads/tasks"])
        .status
        .success());
    assert!(
        !repo.path().join(".tasks").exists(),
        "tiber should not keep a persistent .tasks checkout"
    );
    let status = repo.git_output(["status", "--short", "--", ".tasks"]);
    assert_success_ref(&status);
    assert_eq!(
        String::from_utf8(status.stdout).expect("status output should be utf8"),
        "",
        ".tasks should not appear as source-branch worktree state"
    );

    let tree = repo.git_output(["ls-tree", "-r", "--name-only", "tiber"]);
    assert_success_ref(&tree);
    let tree_names = String::from_utf8(tree.stdout).expect("git tree output is utf8");
    assert!(tree_names
        .lines()
        .all(|line| line.starts_with("eventstore/events/")));
}

#[test]
fn init_ignores_an_existing_source_tree_tasks_system_without_mutation() {
    let repo = TempRepo::initialized();
    fs::create_dir_all(repo.path().join(".tasks/backlog")).expect("create existing task system");
    fs::write(
        repo.path().join(".tasks/backlog/existing.md"),
        "# Existing task\n",
    )
    .expect("write existing task");
    let before = repo.git_output(["status", "--short"]);
    assert_success_ref(&before);

    let output = repo.tiber(["init"]);

    assert_success_ref(&output);
    assert_eq!(
        fs::read(repo.path().join(".tasks/backlog/existing.md")).unwrap(),
        b"# Existing task\n"
    );
    let task_ref = repo.git_output(["show-ref", "--verify", "refs/heads/tiber"]);
    assert_success_ref(&task_ref);
    let after = repo.git_output(["status", "--short"]);
    assert_success_ref(&after);
    assert_eq!(
        after.stdout, before.stdout,
        "init must not mutate source-tree task files"
    );
}

#[test]
fn codex_sandbox_preview_prefers_narrow_git_prefixes() {
    let repo = TempRepo::initialized();

    let output = repo.tiber(["codex-sandbox", "--dry-run"]);

    assert_success_ref(&output);
    let stdout = String::from_utf8(output.stdout).expect("preview output should be utf8");
    assert!(stdout.contains("Tiber Codex sandbox setup preview"));
    assert!(stdout.contains("Prefer the narrowest approval"));
    assert!(stdout.contains("Couldn't get agent socket?"));
    assert!(stdout.contains("forwards SSH_AUTH_SOCK"));
    assert!(stdout.contains("env_vars = [\"SSH_AUTH_SOCK\"]"));
    assert!(stdout.contains("plugin MCP policy overlays do not change transport env"));
    assert!(stdout.contains("preserves the absolute installed launcher"));
    assert!(stdout.contains("Never forward SSH_AUTH_SOCK to a PATH-resolved"));
    assert!(stdout.contains("publish event transactions to origin/tiber"));
    assert!(stdout.contains(
        "Persist approval only when the harness can scope it to the exact Tiber-internal operation"
    ));
    assert!(stdout.contains(
        "Never persist a raw git, wildcard git, bash, sh, or whole-MCP-server permission"
    ));
    assert!(stdout.contains("retry the same structured Tiber MCP operation"));
    assert!(stdout.contains("Do not run the whole Tiber MCP server outside the sandbox"));
    assert!(
        stdout.contains("Do not ask the user to rerun an equivalent tiber CLI command manually")
    );
}
