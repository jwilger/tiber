pub mod support;

use std::fs;
use support::{assert_success, assert_success_ref, task_stem, TempRepo};

#[test]
fn next_show_transition_and_prioritize_follow_order_md() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Write docs"]));
    assert_success(repo.tiber(["create", "Review docs"]));
    let write_docs = task_stem(&repo, "backlog", "write-docs");
    let review_docs = task_stem(&repo, "backlog", "review-docs");

    let next = repo.tiber(["next"]);
    assert_success_ref(&next);
    assert_eq!(
        String::from_utf8(next.stdout).expect("next output should be utf8"),
        format!("{write_docs}\tWrite docs\n")
    );

    let show = repo.tiber(["show", "write-docs"]);
    assert_success_ref(&show);
    assert!(String::from_utf8(show.stdout)
        .expect("show output should be utf8")
        .contains("title: Write docs"));

    assert_success(repo.tiber(["transition", "write-docs", "in-progress"]));
    task_stem(&repo, "in-progress", "write-docs");
    let in_progress = repo.task_file("in-progress", &write_docs);
    assert!(in_progress.contains("claim:\n"));
    assert!(in_progress.contains("  host: "));
    assert!(in_progress.contains("  session: "));
    assert!(
        !String::from_utf8(repo.tiber(["list", "--status", "backlog"]).stdout)
            .unwrap()
            .contains(&write_docs)
    );

    assert_success(repo.tiber(["prioritize", "review-docs", "--before", "write-docs"]));

    assert_eq!(repo.order_file(), format!("{review_docs}\n{write_docs}\n"));
}

#[test]
fn transition_releases_claim_when_leaving_in_progress() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Claim lifecycle"]));
    let task = task_stem(&repo, "backlog", "claim-lifecycle");

    assert_success(repo.tiber_with_env(
        ["transition", "claim-lifecycle", "in-progress"],
        [
            ("TIBER_CLAIM_HOST", "test-host"),
            ("TIBER_CLAIM_SESSION", "test-session"),
        ],
    ));
    let in_progress = repo.task_file("in-progress", &task);
    assert!(in_progress.contains("claim:\n  host: test-host\n  session: test-session\n"));

    assert_success(repo.tiber(["transition", "claim-lifecycle", "done"]));
    let done = repo.task_file("done", &task);
    assert!(!done.contains("claim:"));
    assert!(!done.contains("test-session"));
}

#[test]
fn transition_refuses_reopening_into_a_full_backlog() {
    let repo = TempRepo::initialized();
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .expect("write tiber config");
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Completed work"]));
    assert_success(repo.tiber(["transition", "completed-work", "done"]));
    assert_success(repo.tiber(["create", "Queued work"]));

    let reopen = repo.tiber(["transition", "completed-work", "backlog"]);

    assert!(!reopen.status.success(), "reopen should refuse overflow");
    let stderr = String::from_utf8(reopen.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains("backlog_capacity_exceeded")
            && stderr.contains("queued=1")
            && stderr.contains("max_queued=1"),
        "stderr should explain the full backlog: {stderr}"
    );
    task_stem(&repo, "done", "completed-work");
}

#[test]
fn transition_refuses_moving_active_work_into_a_full_backlog() {
    let repo = TempRepo::initialized();
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .expect("write tiber config");
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Active work"]));
    assert_success(repo.tiber(["transition", "active-work", "in-progress"]));
    assert_success(repo.tiber(["create", "Queued work"]));

    let move_back = repo.tiber(["transition", "active-work", "backlog"]);

    assert!(
        !move_back.status.success(),
        "move into backlog should refuse overflow"
    );
    let stderr = String::from_utf8(move_back.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains("backlog_capacity_exceeded"),
        "stderr should explain the full backlog: {stderr}"
    );
    task_stem(&repo, "in-progress", "active-work");
}

#[test]
fn over_capacity_projects_can_move_work_out_of_the_backlog() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "First queued work"]));
    assert_success(repo.tiber(["create", "Second queued work"]));
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .expect("write tiber config");

    assert_success(repo.tiber(["transition", "first-queued-work", "in-progress"]));

    task_stem(&repo, "in-progress", "first-queued-work");
    task_stem(&repo, "backlog", "second-queued-work");
}

#[test]
fn next_skips_tasks_blocked_by_open_dependencies() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Blocked task"]));
    assert_success(repo.tiber(["create", "Dependency task"]));
    let blocked = task_stem(&repo, "backlog", "blocked-task");
    let dependency = task_stem(&repo, "backlog", "dependency-task");
    assert_success(repo.tiber(["link", "dependency-task", "blocks", "blocked-task"]));
    assert_success(repo.tiber(["prioritize", "blocked-task", "--before", "dependency-task"]));

    let next = repo.tiber(["next"]);

    assert_success_ref(&next);
    assert_eq!(
        String::from_utf8(next.stdout).expect("next output should be utf8"),
        format!("{dependency}\tDependency task\n")
    );

    assert_success(repo.tiber(["transition", "dependency-task", "done"]));
    let next_after_dependency_done = repo.tiber(["next"]);

    assert_success_ref(&next_after_dependency_done);
    assert_eq!(
        String::from_utf8(next_after_dependency_done.stdout).expect("next output should be utf8"),
        format!("{blocked}\tBlocked task\n")
    );
}

#[test]
fn task_refs_can_use_unique_filename_identity() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Write docs"]));
    assert_success(repo.tiber(["create", "Review docs"]));
    let write_docs = task_stem(&repo, "backlog", "write-docs");
    let review_docs = task_stem(&repo, "backlog", "review-docs");

    let show = repo.tiber(["show", "write-docs"]);
    assert_success_ref(&show);
    assert!(String::from_utf8(show.stdout)
        .expect("show output should be utf8")
        .contains("title: Write docs"));

    assert_success(repo.tiber(["transition", "write-docs", "in-progress"]));
    assert_success(repo.tiber(["prioritize", "review-docs", "--before", "write-docs"]));
    assert_eq!(repo.order_file(), format!("{review_docs}\n{write_docs}\n"));
}
