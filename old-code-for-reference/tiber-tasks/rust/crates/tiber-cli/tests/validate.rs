pub mod support;

use std::fs;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn validate_fix_preserves_projected_task_state() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Validated task"]));
    assert_success(repo.tiber(["acceptance", "add", "validated-task", "It works"]));

    let validate = repo.tiber(["validate", "--fix"]);
    assert_success_ref(&validate);
    let show = repo.tiber(["show", "validated-task"]);
    assert_success_ref(&show);
    assert!(String::from_utf8(show.stdout)
        .unwrap()
        .contains("- [ ] It works"));
}

#[test]
fn validate_fix_preserves_claims_on_in_progress_tasks() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Claimed task"]));
    assert_success(repo.tiber_with_env(
        ["transition", "claimed-task", "in-progress"],
        [
            ("TIBER_CLAIM_HOST", "validate-host"),
            ("TIBER_CLAIM_SESSION", "validate-session"),
        ],
    ));

    assert_success(repo.tiber(["validate", "--fix"]));
    let show = repo.tiber(["show", "claimed-task"]);
    assert_success_ref(&show);
    assert!(String::from_utf8(show.stdout)
        .unwrap()
        .contains("session: validate-session"));
}

#[test]
fn validate_fix_does_not_publish_when_the_board_needs_no_repairs() {
    let (origin, hook_path) = TempRepo::bare_with_rejecting_hook();
    let rejecting_hook = fs::read(&hook_path).expect("read rejecting hook");
    fs::remove_file(&hook_path).expect("temporarily remove rejecting hook");
    let repo = TempRepo::initialized();
    repo.git([
        "remote",
        "add",
        "origin",
        origin.path().to_str().expect("origin path should be utf8"),
    ]);
    assert_success(repo.tiber(["init"]));
    let before = origin.git_output(["rev-parse", "refs/heads/tiber"]);
    assert_success_ref(&before);

    fs::write(&hook_path, rejecting_hook).expect("restore rejecting hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("make rejecting hook executable");
    }

    assert_success(repo.tiber(["validate", "--fix"]));
    assert_success(repo.tiber(["sync"]));
    let after = origin.git_output(["rev-parse", "refs/heads/tiber"]);
    assert_success_ref(&after);
    assert_eq!(before.stdout, after.stdout, "a healthy board is a no-op");
}
