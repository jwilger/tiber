pub mod support;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn update_edits_title_summary_context_and_tags_without_raw_markdown_paths() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let update = repo.tiber([
        "update",
        "original-title",
        "--title",
        "Updated title",
        "--summary",
        "The new summary.",
        "--context",
        "The new context.",
        "--tags",
        "alpha,beta",
    ]);

    assert_success(update);
    let show = repo.tiber(["show", "original-title"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(task.contains("title: Updated title\n"));
    assert!(task.contains("tags: [alpha, beta]\n"));
    assert!(
        task.contains("## Summary\n\nThe new summary.\n\n## Context / Why\n\nThe new context.\n")
    );
}

#[test]
fn update_preserves_multiline_summaries_and_literal_backslashes() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Migration summary"]));
    let summary =
        "First migration paragraph.\n\n## Embedded detail\n\nSecond line keeps literal \\n text.";

    let update = repo.tiber(["update", "migration-summary", "--summary", summary]);

    assert_success(update);
    let show = repo.tiber(["show", "migration-summary"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(task.contains(&format!("## Summary\n\n{summary}\n\n## Context / Why")));

    let replacement = "Replacement line one.\n\nReplacement line two.";
    assert_success(repo.tiber(["update", "migration-summary", "--summary", replacement]));
    let replaced = repo.tiber(["show", "migration-summary"]);
    assert_success_ref(&replaced);
    let replaced = String::from_utf8(replaced.stdout).expect("show output should be utf8");
    assert!(replaced.contains(&format!("## Summary\n\n{replacement}\n\n## Context / Why")));
    assert!(!replaced.contains("Embedded detail"));
}

#[test]
fn failed_summary_update_preserves_the_existing_ticket() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Atomic summary"]));
    assert_success(repo.tiber(["update", "atomic-summary", "--summary", "Original summary"]));
    let before = repo.tiber(["show", "atomic-summary"]);
    assert_success_ref(&before);

    let update = repo.tiber([
        "update",
        "atomic-summary",
        "--title",
        "Title that must not persist",
        "--summary",
        "invalid\u{7}summary",
    ]);

    assert!(!update.status.success());
    let invalid_stderr = String::from_utf8_lossy(&update.stderr);
    assert!(invalid_stderr.contains("section_invalid=true"));
    assert!(invalid_stderr.contains("remove control characters other than newline or tab"));
    let after = repo.tiber(["show", "atomic-summary"]);
    assert_success_ref(&after);
    assert_eq!(after.stdout, before.stdout);

    let reserved_heading = repo.tiber([
        "update",
        "atomic-summary",
        "--summary",
        "valid text\n\n## Context / Why\n\nambiguous text",
    ]);
    assert!(!reserved_heading.status.success());
    let reserved_stderr = String::from_utf8_lossy(&reserved_heading.stderr);
    assert!(reserved_stderr.contains("section_reserved_heading=true"));
    assert!(reserved_stderr.contains("demote or rename the embedded heading"));
    let after_reserved_heading = repo.tiber(["show", "atomic-summary"]);
    assert_success_ref(&after_reserved_heading);
    assert_eq!(after_reserved_heading.stdout, before.stdout);
}

#[test]
fn update_tag_list_trims_whitespace_and_ignores_empty_entries() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Normalize tags"]));

    let update = repo.tiber(["update", "normalize-tags", "--tags", "alpha, beta, ,"]);

    assert_success(update);
    let show = repo.tiber(["show", "normalize-tags"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(
        task.contains("tags: [alpha, beta]\n"),
        "unexpected tags: {task}"
    );
}

#[test]
fn repeated_update_fields_keep_the_last_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Repeated fields"]));

    let update = repo.tiber([
        "update",
        "repeated-fields",
        "--title",
        "First title",
        "--title",
        "Final title",
        "--tags",
        "old",
        "--tags",
        "new, final",
    ]);

    assert_success(update);
    let show = repo.tiber(["show", "repeated-fields"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(
        task.contains("title: Final title\n"),
        "unexpected title: {task}"
    );
    assert!(
        task.contains("tags: [new, final]\n"),
        "unexpected tags: {task}"
    );
}

#[test]
fn update_edits_pr_mr_tracking_frontmatter() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Review tracked"]));

    let update = repo.tiber([
        "update",
        "review-tracked",
        "--pr-mr-url",
        "https://github.com/example/repo/pull/42",
        "--pr-mr-status",
        "review-required",
    ]);

    assert_success(update);
    let show = repo.tiber(["show", "review-tracked"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("show output should be utf8");
    assert!(task.contains("pr_mr_url: https://github.com/example/repo/pull/42\n"));
    assert!(task.contains("pr_mr_status: review-required\n"));

    assert_success(repo.tiber([
        "update",
        "review-tracked",
        "--pr-mr-url",
        "",
        "--pr-mr-status",
        "",
    ]));
    let cleared = repo.tiber(["show", "review-tracked"]);
    assert_success_ref(&cleared);
    let task = String::from_utf8(cleared.stdout).expect("show output should be utf8");
    assert!(task.contains("pr_mr_url: \n"));
    assert!(task.contains("pr_mr_status: \n"));
    assert!(!task.contains("pr_mr_url: unknown\n"));
}

#[test]
fn acceptance_add_check_uncheck_and_remove_edits_acceptance_section() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Acceptance task"]));

    assert_success(repo.tiber([
        "acceptance",
        "add",
        "acceptance-task",
        "First observable condition",
    ]));
    assert_success(repo.tiber(["acceptance", "check", "acceptance-task", "1"]));
    let checked = repo.tiber(["show", "acceptance-task"]);
    assert_success_ref(&checked);
    assert!(String::from_utf8(checked.stdout)
        .expect("checked task should be utf8")
        .contains("## Acceptance criteria\n\n- [x] First observable condition\n"));

    assert_success(repo.tiber(["acceptance", "uncheck", "acceptance-task", "1"]));
    assert_success(repo.tiber(["acceptance", "remove", "acceptance-task", "1"]));
    let removed = repo.tiber(["show", "acceptance-task"]);
    assert_success_ref(&removed);
    assert!(String::from_utf8(removed.stdout)
        .expect("removed task should be utf8")
        .contains("## Acceptance criteria\n\n## Subtasks\n"));
}

#[test]
fn note_add_appends_dated_note_to_notes_log() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Noted task"]));

    assert_success(repo.tiber(["note", "add", "noted-task", "Made progress."]));

    let show = repo.tiber(["show", "noted-task"]);
    assert_success_ref(&show);
    let task = String::from_utf8(show.stdout).expect("task should be utf8");
    assert!(task.contains("## Notes / Log\n\n- 20"));
    assert!(task.contains(": Made progress.\n"));
}

#[test]
fn acceptance_criteria_and_notes_remain_single_line() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Single-line entries"]));
    let before = repo.tiber(["show", "single-line-entries"]);
    assert_success_ref(&before);

    let acceptance = repo.tiber([
        "acceptance",
        "add",
        "single-line-entries",
        "criterion\n## Notes / Log",
    ]);
    assert!(!acceptance.status.success());
    assert!(String::from_utf8_lossy(&acceptance.stderr).contains("acceptance_invalid=true"));

    let note = repo.tiber([
        "note",
        "add",
        "single-line-entries",
        "progress\n## Acceptance criteria",
    ]);
    assert!(!note.status.success());
    assert!(String::from_utf8_lossy(&note.stderr).contains("note_invalid=true"));

    let after = repo.tiber(["show", "single-line-entries"]);
    assert_success_ref(&after);
    assert_eq!(after.stdout, before.stdout);
}
