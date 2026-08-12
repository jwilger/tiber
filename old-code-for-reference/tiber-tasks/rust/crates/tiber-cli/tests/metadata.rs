pub mod support;

use support::{assert_success, assert_success_ref, task_stem, TempRepo};

#[test]
fn metadata_reports_task_commit_time_from_tasks_branch() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber_with_env(
        ["create", "Date stamped task"],
        [
            ("GIT_AUTHOR_DATE", "2024-01-02T03:04:05Z"),
            ("GIT_COMMITTER_DATE", "2024-01-02T03:04:05Z"),
        ],
    ));
    let stem = task_stem(&repo, "backlog", "date-stamped-task");

    let metadata = repo.tiber(["metadata", "date-stamped-task"]);

    assert_success_ref(&metadata);
    assert_eq!(
        String::from_utf8(metadata.stdout).expect("metadata output should be utf8"),
        format!("{stem}\tDate stamped task\tcommitted_at=2024-01-02T03:04:05Z\n")
    );
}
