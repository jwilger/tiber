pub mod support;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn sync_publishes_the_single_tiber_event_branch() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let repo = TempRepo::initialized();
    repo.git(["remote", "add", "origin", origin.path().to_str().unwrap()]);

    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Published task"]));
    assert_success(repo.tiber(["sync"]));

    assert_success(origin.git_output(["show-ref", "--verify", "refs/heads/tiber"]));
    assert!(!origin
        .git_output(["show-ref", "--verify", "refs/heads/tasks"])
        .status
        .success());
    let tree = origin.git_output(["ls-tree", "-r", "--name-only", "tiber"]);
    assert_success_ref(&tree);
    assert!(String::from_utf8(tree.stdout)
        .unwrap()
        .lines()
        .all(|path| path.starts_with("eventstore/events/")));
}

#[test]
fn reads_refresh_remote_event_state_without_explicit_sync() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let writer = TempRepo::initialized();
    writer.git(["remote", "add", "origin", origin.path().to_str().unwrap()]);
    assert_success(writer.tiber(["init"]));

    let reader = TempRepo::initialized();
    reader.git(["remote", "add", "origin", origin.path().to_str().unwrap()]);
    assert_success(reader.tiber(["sync"]));

    assert_success(writer.tiber(["create", "Shared event task"]));
    let list = reader.tiber(["list"]);
    assert_success_ref(&list);
    assert!(String::from_utf8(list.stdout)
        .unwrap()
        .contains("Shared event task"));
}

#[test]
fn invalid_status_is_rejected_before_a_read() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    let list = repo.tiber(["list", "--status", "not-a-status"]);
    assert!(!list.status.success());
    assert!(String::from_utf8(list.stderr)
        .unwrap()
        .contains("invalid_status"));
}
