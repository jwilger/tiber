pub mod support;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn free_text_and_path_values_may_start_with_hyphens() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "-Leading title"]));
    assert_success(repo.tiber(["subtask", "add", "leading-title", "-Leading subtask"]));
    assert_success(repo.tiber(["subtask", "add", "leading-title", "--after"]));
    assert_success(repo.tiber(["subtask", "add", "leading-title", "--after=Literal title"]));
    assert_success(repo.tiber(["acceptance", "add", "leading-title", "-Leading criterion"]));
    assert_success(repo.tiber(["note", "add", "leading-title", "-Leading note"]));
    assert_success(repo.tiber([
        "update",
        "leading-title",
        "--title",
        "-Updated title",
        "--summary",
        "-Leading summary",
        "--context",
        "-Leading context",
        "--tags",
        "-alpha, -beta",
        "--pr-mr-url",
        "-relative-review-url",
        "--pr-mr-status",
        "-draft",
    ]));
    assert_success(repo.tiber(["install-bin", "--target-dir", "-relative-bin", "--dry-run"]));
    assert_success(repo.tiber(["install-bin", "--target-dir=--dry-run", "--dry-run"]));

    let show = repo.tiber(["show", "leading-title"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    for expected in [
        "title: -Updated title\n",
        "tags: [-alpha, -beta]\n",
        "pr_mr_url: -relative-review-url\n",
        "pr_mr_status: -draft\n",
        "## Summary\n\n-Leading summary\n",
        "## Context / Why\n\n-Leading context\n",
        "- [ ] -Leading criterion\n",
        "- [ ] (s1) -Leading subtask\n",
        "- [ ] (s2) --after\n",
        "- [ ] (s3) --after=Literal title\n",
        ": -Leading note\n",
    ] {
        assert!(
            task.contains(expected),
            "missing {expected:?} in task:\n{task}"
        );
    }
}

#[test]
fn adjacent_assigned_update_options_remain_valid() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Assigned fields"]));

    let update = repo.tiber([
        "update",
        "assigned-fields",
        "--summary=Assigned summary",
        "--tags=alpha,beta",
    ]);

    assert_success(update);
    let show = repo.tiber(["show", "assigned-fields"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(
        task.contains("tags: [alpha, beta]\n"),
        "unexpected task: {task}"
    );
    assert!(
        task.contains("## Summary\n\nAssigned summary\n"),
        "unexpected task: {task}"
    );
}

#[test]
fn explicit_update_option_like_value_is_stored_literally() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Literal option value"]));

    let update = repo.tiber(["update", "literal-option-value", "--summary=--tags"]);

    assert_success(update);
    let show = repo.tiber(["show", "literal-option-value"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(
        task.contains("## Summary\n\n--tags\n"),
        "explicit option-like value was not stored literally: {task}"
    );
}
