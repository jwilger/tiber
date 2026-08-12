pub mod support;

use std::fs;
use std::process::Command;

use support::{assert_success, assert_success_ref, task_stem, TempRepo};

#[test]
fn close_from_trailers_moves_closed_tasks_to_done() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Fix bug"]));
    let fix_bug = task_stem(&repo, "backlog", "fix-bug");
    fs::write(repo.path().join("fix.txt"), "fixed\n").expect("write fix file");
    repo.git(["add", "fix.txt"]);
    repo.git(["commit", "-m", "Fix bug\n\nCloses: fix-bug"]);

    let close = repo.tiber(["close-from-trailers"]);

    assert_success_ref(&close);
    assert_eq!(
        String::from_utf8(close.stdout).expect("stdout should be utf8"),
        format!("closed {fix_bug}\n")
    );
    task_stem(&repo, "done", "fix-bug");
    assert_eq!(repo.order_file(), "");
}

#[test]
fn close_from_trailers_is_a_noop_without_current_head_closures() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    let tasks_before = repo.git_output(["rev-parse", "tiber"]).stdout;
    fs::write(repo.path().join("ordinary.txt"), "ordinary\n").expect("write ordinary change");
    repo.git(["add", "ordinary.txt"]);
    repo.git(["commit", "-m", "Ordinary main change"]);

    let close = repo.tiber_with_env(
        ["close-from-trailers"],
        [
            ("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z"),
        ],
    );

    assert_success_ref(&close);
    assert!(close.stdout.is_empty());
    assert_eq!(repo.git_output(["rev-parse", "tiber"]).stdout, tasks_before);
}

#[test]
fn close_from_trailers_fetches_remote_tasks_before_resolving_closures() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);

    let seed = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(seed.path())
            .output()
            .expect("add origin remote"),
    );
    seed.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    assert_success(seed.tiber(["init"]));
    assert_success(seed.tiber(["create", "Publish architecture records"]));
    let architecture = task_stem(&seed, "backlog", "publish-architecture-records");
    assert_success(seed.tiber(["create", "Publish release notes"]));
    let release_notes = task_stem(&seed, "backlog", "publish-release-notes");
    assert_success(seed.tiber(["transition", &architecture, "in-progress"]));
    assert_success(seed.tiber(["transition", &release_notes, "in-progress"]));
    fs::write(seed.path().join("architecture.md"), "published\n").expect("write completed work");
    seed.git(["add", "architecture.md"]);
    seed.git([
        "commit",
        "-m",
        &format!("Publish architecture records\n\nCloses: {architecture}\nCloses: {release_notes}"),
    ]);
    seed.git(["push", "origin", "main"]);

    let automation = TempRepo::new();
    assert_success(
        Command::new("git")
            .args(["clone", origin.path().to_str().expect("origin path utf8")])
            .arg(automation.path())
            .output()
            .expect("clone automation checkout"),
    );
    automation.git(["config", "user.email", "tiber@example.test"]);
    automation.git(["config", "user.name", "Tiber Test"]);
    automation.git(["config", "commit.gpgsign", "false"]);

    let close = automation.tiber(["close-from-trailers"]);

    assert_success_ref(&close);
    let mut expected_closed = [architecture.as_str(), release_notes.as_str()];
    expected_closed.sort();
    assert_eq!(
        String::from_utf8(close.stdout).expect("stdout should be utf8"),
        expected_closed
            .into_iter()
            .map(|task| format!("closed {task}\n"))
            .collect::<String>()
    );
    for task in [architecture, release_notes] {
        let shown = automation.tiber(["show", &task]);
        assert_success_ref(&shown);
        task_stem(&automation, "done", task.splitn(3, '-').nth(2).unwrap());
    }
}

#[test]
fn close_from_trailers_fails_when_a_requested_task_is_missing() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    fs::write(repo.path().join("fix.txt"), "fixed\n").expect("write completed work");
    repo.git(["add", "fix.txt"]);
    repo.git(["commit", "-m", "Fix missing task\n\nCloses: 20260721-miss"]);

    let close = repo.tiber(["close-from-trailers"]);

    assert!(!close.status.success());
    assert_eq!(
        String::from_utf8(close.stderr).expect("stderr should be utf8"),
        "tiber.parse_error task_ref_missing ref=20260721-miss\n"
    );
}

#[test]
fn close_from_trailers_ignores_closures_from_older_commits() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    fs::write(repo.path().join("old.txt"), "old\n").expect("write historical work");
    repo.git(["add", "old.txt"]);
    repo.git(["commit", "-m", "Historical work\n\nCloses: 20260720-gone"]);
    assert_success(repo.tiber(["create", "Current delivery"]));
    let current = task_stem(&repo, "backlog", "current-delivery");
    fs::write(repo.path().join("current.txt"), "current\n").expect("write current work");
    repo.git(["add", "current.txt"]);
    repo.git([
        "commit",
        "-m",
        &format!("Current delivery\n\nCloses: {current}"),
    ]);

    let close = repo.tiber(["close-from-trailers"]);

    assert_success(close);
    task_stem(&repo, "done", "current-delivery");
}
