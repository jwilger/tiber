pub mod support;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn subtask_add_check_and_uncheck_update_task_checklist() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Ship feature"]));

    let add = repo.tiber(["subtask", "add", "ship-feature", "Write tests"]);

    assert_success(add);
    let task = repo.tiber(["show", "ship-feature"]);
    assert_success_ref(&task);
    assert!(String::from_utf8(task.stdout)
        .expect("task should be utf8")
        .contains("## Subtasks\n\n- [ ] (s1) Write tests\n"));

    assert_success(repo.tiber(["subtask", "add", "ship-feature", "Wire UI", "--after", "s1"]));
    let task = repo.tiber(["show", "ship-feature"]);
    assert_success_ref(&task);
    assert!(String::from_utf8(task.stdout)
        .expect("task should be utf8")
        .contains("- [ ] (s2) Wire UI — after: s1\n"));

    assert_success(repo.tiber(["subtask", "check", "ship-feature", "s1"]));
    let task = repo.tiber(["show", "ship-feature"]);
    assert_success_ref(&task);
    assert!(String::from_utf8(task.stdout)
        .expect("task should be utf8")
        .contains("## Subtasks\n\n- [x] (s1) Write tests\n"));

    assert_success(repo.tiber(["subtask", "uncheck", "ship-feature", "s1"]));
    let task = repo.tiber(["show", "ship-feature"]);
    assert_success_ref(&task);
    assert!(String::from_utf8(task.stdout)
        .expect("task should be utf8")
        .contains("## Subtasks\n\n- [ ] (s1) Write tests\n"));
}

#[test]
fn subtask_predecessor_list_trims_whitespace_and_ignores_empty_entries() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Normalize predecessors"]));
    assert_success(repo.tiber([
        "subtask",
        "add",
        "normalize-predecessors",
        "First dependency",
    ]));
    assert_success(repo.tiber([
        "subtask",
        "add",
        "normalize-predecessors",
        "Second dependency",
    ]));

    let add = repo.tiber([
        "subtask",
        "add",
        "normalize-predecessors",
        "Dependent task",
        "--after",
        "s1, s2, ,",
    ]);

    assert_success(add);
    let task = repo.tiber(["show", "normalize-predecessors"]);
    assert_success_ref(&task);
    assert!(String::from_utf8(task.stdout)
        .expect("task should be utf8")
        .contains("- [ ] (s3) Dependent task — after: s1, s2\n"));
}

#[test]
fn subtask_check_only_updates_subtasks_section() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Scoped subtask check"]));
    assert_success(repo.tiber(["subtask", "add", "scoped-subtask-check", "Real subtask"]));
    assert_success(repo.tiber([
        "acceptance",
        "add",
        "scoped-subtask-check",
        "Acceptance item with matching marker (s1)",
    ]));
    let task = support::task_stem(&repo, "backlog", "scoped-subtask-check");

    assert_success(repo.tiber(["subtask", "check", "scoped-subtask-check", "s1"]));

    let updated = repo.task_file("backlog", &task);
    assert!(updated
        .contains("## Acceptance criteria\n\n- [ ] Acceptance item with matching marker (s1)"));
    assert!(updated.contains("## Subtasks\n\n- [x] (s1) Real subtask"));
}
