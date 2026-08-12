pub mod support;

use std::fs;
use support::{assert_success, assert_success_ref, task_stem, TempRepo};

#[test]
fn create_stores_course_shaped_task_in_backlog_and_list_prints_ordered_summary() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));

    let create = repo.tiber(["create", "Write tiber docs"]);

    assert_success_ref(&create);
    let stem = task_stem(&repo, "backlog", "write-tiber-docs");
    assert_eq!(
        String::from_utf8(create.stdout).expect("create output should be utf8"),
        format!("created {stem}\n")
    );
    let file_name = format!("{stem}.md");
    assert!(file_name.ends_with("-write-tiber-docs.md"));
    let (date, rest) = stem
        .split_once('-')
        .expect("task stem should contain date and random code");
    let (code, nickname) = rest
        .split_once('-')
        .expect("task stem should contain random code and nickname");
    assert_eq!(date.len(), 8, "task id date should be YYYYMMDD");
    assert!(date.chars().all(|character| character.is_ascii_digit()));
    assert_eq!(code.len(), 4, "task id random code should be four chars");
    assert!(code
        .chars()
        .all(|character| "abcdefghijkmnpqrstuvwxyz23456789".contains(character)));
    assert_eq!(nickname, "write-tiber-docs");

    let task = repo.task_file("backlog", &stem);
    assert!(task.starts_with(
        "---\ntitle: Write tiber docs\nblocked_by: []\nblocks: []\ntags: []\npr_mr_url: \npr_mr_status: \n---\n"
    ));
    assert!(task.contains("## Summary\n\n"));
    assert!(task.contains("## Context / Why\n\n"));
    assert!(task.contains("## Acceptance criteria\n\n"));
    assert!(task.contains("## Subtasks\n\n"));
    assert!(task.contains("## Notes / Log\n"));

    assert_eq!(repo.order_file(), format!("{stem}\n"));

    let list = repo.tiber(["list"]);

    assert_success_ref(&list);
    assert_eq!(
        String::from_utf8(list.stdout).expect("list output should be utf8"),
        format!("{stem}\tWrite tiber docs\n")
    );
}

#[test]
fn list_filters_completed_tasks_by_status() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Finished historical work"]));
    assert_success(repo.tiber(["transition", "finished-historical-work", "done"]));
    assert_success(repo.tiber(["create", "Still queued work"]));
    let stem = task_stem(&repo, "done", "finished-historical-work");
    task_stem(&repo, "backlog", "still-queued-work");

    let list = repo.tiber(["list", "--status", "done"]);

    assert_success_ref(&list);
    assert_eq!(
        String::from_utf8(list.stdout).expect("list output should be utf8"),
        format!("{stem}\tFinished historical work\n")
    );
}

#[test]
fn task_reads_and_writes_work_from_a_coordination_only_primary_checkout() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Existing shared task"]));
    repo.git(["config", "core.worktree", ".."]);
    repo.git(["config", "core.bare", "true"]);

    let list = repo.tiber(["list"]);

    assert_success_ref(&list);
    assert!(
        String::from_utf8(list.stdout)
            .expect("list output should be utf8")
            .contains("Existing shared task"),
        "coordination checkout should read the shared task board"
    );

    let create = repo.tiber(["create", "Created from coordination checkout"]);

    assert_success_ref(&create);
    task_stem(&repo, "backlog", "created-from-coordination-checkout");
}

#[test]
fn true_bare_repository_named_dot_git_is_not_a_coordination_checkout() {
    let seed = TempRepo::initialized();
    let parent = TempRepo::new();
    assert_success(parent.command(
        "git",
        [
            "clone",
            "--bare",
            seed.path().to_str().expect("seed path should be utf8"),
            ".git",
        ],
    ));
    fs::write(parent.path().join("README.md"), "# unrelated file\n")
        .expect("write colliding parent file");
    let init = parent.tiber_at(parent.path(), ["init"]);
    assert!(
        !init.status.success(),
        "tiber init should refuse a true bare repository"
    );
    assert!(
        String::from_utf8(init.stderr)
            .expect("init stderr should be utf8")
            .contains("tiber.repository_root_unresolved"),
        "failure should explain that no checkout root could be resolved"
    );
}

#[test]
fn search_finds_historical_titles_and_descriptions_as_structured_results() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Retire legacy admission path"]));
    assert_success(repo.tiber([
        "update",
        "retire-legacy-admission-path",
        "--summary",
        "Prevent duplicate backlog candidates",
        "--context",
        "Operators need durable history before admitting work",
    ]));
    assert_success(repo.tiber(["transition", "retire-legacy-admission-path", "done"]));
    let completed = task_stem(&repo, "done", "retire-legacy-admission-path");
    for (title, task_ref, status) in [
        (
            "Queued duplicate history",
            "queued-duplicate-history",
            "backlog",
        ),
        (
            "Active duplicate history",
            "active-duplicate-history",
            "in-progress",
        ),
        (
            "Rejected duplicate history",
            "rejected-duplicate-history",
            "abandoned",
        ),
    ] {
        assert_success(repo.tiber(["create", title]));
        assert_success(repo.tiber([
            "update",
            task_ref,
            "--summary",
            "Prevent duplicate backlog candidates",
        ]));
        if status != "backlog" {
            assert_success(repo.tiber(["transition", task_ref, status]));
        }
    }
    let queued = task_stem(&repo, "backlog", "queued-duplicate-history");
    let active = task_stem(&repo, "in-progress", "active-duplicate-history");
    let rejected = task_stem(&repo, "abandoned", "rejected-duplicate-history");
    assert_success(repo.tiber(["create", "Unrelated queued work"]));

    let title_search = repo.tiber(["search", "legacy admission"]);
    assert_success_ref(&title_search);
    let title_results: serde_json::Value =
        serde_json::from_slice(&title_search.stdout).expect("search output should be JSON");
    assert_eq!(
        title_results,
        serde_json::json!([{
            "id": completed,
            "status": "done",
            "title": "Retire legacy admission path",
            "summary": "Prevent duplicate backlog candidates",
            "context": "Operators need durable history before admitting work"
        }])
    );

    let description_search = repo.tiber(["search", "durable HISTORY"]);
    assert_success_ref(&description_search);
    let description_results: serde_json::Value =
        serde_json::from_slice(&description_search.stdout).expect("search output should be JSON");
    assert_eq!(description_results, title_results);

    let summary_search = repo.tiber(["search", "DUPLICATE backlog"]);
    assert_success_ref(&summary_search);
    let summary_results: serde_json::Value =
        serde_json::from_slice(&summary_search.stdout).expect("search output should be JSON");
    let summary_results = summary_results
        .as_array()
        .expect("search output should be an array");
    assert_eq!(
        summary_results
            .iter()
            .map(|result| {
                (
                    result["id"].as_str().expect("result id"),
                    result["status"].as_str().expect("result status"),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (rejected.as_str(), "abandoned"),
            (queued.as_str(), "backlog"),
            (completed.as_str(), "done"),
            (active.as_str(), "in-progress"),
        ]
    );
    assert!(summary_results.iter().all(|result| {
        result["summary"] == "Prevent duplicate backlog candidates"
            && result["title"].is_string()
            && result["context"].is_string()
    }));
}

#[test]
fn create_refuses_when_configured_backlog_capacity_is_full() {
    let repo = TempRepo::initialized();
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = 1\n",
    )
    .expect("write tiber config");
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Keep this work"]));

    let create = repo.tiber(["create", "Overflow work"]);

    assert!(!create.status.success(), "create should refuse overflow");
    let stderr = String::from_utf8(create.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains("backlog_capacity_exceeded"),
        "stderr should identify the refusal: {stderr}"
    );
    assert!(
        stderr.contains("queued=1") && stderr.contains("max_queued=1"),
        "stderr should report the current count and limit: {stderr}"
    );
    assert!(
        stderr.contains("replace") && stderr.contains("combine") && stderr.contains("reject"),
        "stderr should explain the available admission decisions: {stderr}"
    );
    assert!(
        !repo
            .git_output(["ls-tree", "-r", "--name-only", "tasks"])
            .stdout
            .windows(b"overflow-work".len())
            .any(|window| window == b"overflow-work"),
        "refused work should not be stored"
    );
}

#[test]
fn backlog_capacity_is_unlimited_when_project_config_is_absent() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));

    for title in ["First work", "Second work", "Third work"] {
        assert_success(repo.tiber(["create", title]));
    }

    let listing = repo.tiber(["list", "--status", "backlog"]);
    assert_success_ref(&listing);
    assert_eq!(
        String::from_utf8(listing.stdout).unwrap().lines().count(),
        3
    );
}

#[test]
fn active_ticket_does_not_count_toward_backlog_capacity() {
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

    task_stem(&repo, "in-progress", "active-work");
    task_stem(&repo, "backlog", "queued-work");
}

#[test]
fn malformed_project_config_fails_closed_before_task_creation() {
    let repo = TempRepo::initialized();
    fs::write(
        repo.path().join(".tiber.toml"),
        "[backlog]\nmax_queued = \"many\"\n",
    )
    .expect("write malformed tiber config");
    assert_success(repo.tiber(["init"]));

    let create = repo.tiber(["create", "Unsafe admission"]);

    assert!(!create.status.success(), "malformed config should refuse");
    let stderr = String::from_utf8(create.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains("config_invalid") && stderr.contains(".tiber.toml"),
        "error should identify the configuration recovery surface: {stderr}"
    );
    let listing = repo.tiber(["list"]);
    assert_success_ref(&listing);
    assert!(
        !String::from_utf8(listing.stdout)
            .unwrap()
            .contains("unsafe-admission"),
        "invalid configuration must not admit work"
    );
}

#[test]
fn rejected_publication_is_blocking_and_sync_can_publish_the_pending_transaction() {
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
    fs::write(&hook_path, rejecting_hook).expect("restore rejecting hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("make rejecting hook executable");
    }

    let create = repo.tiber(["create", "Release smoke"]);

    assert!(
        !create.status.success(),
        "create should surface sync failure"
    );
    let stderr = String::from_utf8(create.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("event_store_failed"), "{stderr}");
    assert!(
        !stderr.contains("secret@example.invalid"),
        "stderr should not leak token-bearing remote details: {stderr}"
    );
    assert!(
        !stderr.contains("private/repo.git"),
        "stderr should not leak private remote paths: {stderr}"
    );
    assert!(
        !stderr.contains(repo.path().to_str().expect("repo path should be utf8")),
        "stderr should not leak local repository paths: {stderr}"
    );
    fs::remove_file(&hook_path).expect("remove rejecting hook");
    assert_success(repo.tiber(["sync"]));
    task_stem(&repo, "backlog", "release-smoke");
    assert_success(origin.git_output(["show-ref", "--verify", "refs/heads/tiber"]));
}

#[test]
fn create_failure_before_local_task_commit_does_not_report_unrecoverable_ref() {
    let repo = TempRepo::initialized();
    let missing_origin = repo
        .path()
        .join("private")
        .join("user-secret@example.invalid")
        .join("missing-origin.git");
    repo.git([
        "remote",
        "add",
        "origin",
        missing_origin
            .to_str()
            .expect("missing origin path should be utf8"),
    ]);
    let create = repo.tiber(["create", "Lost before sync"]);

    assert!(
        !create.status.success(),
        "create should surface sync failure"
    );
    let stderr = String::from_utf8(create.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains("event_store_failed"),
        "stderr should report the blocking event-store failure: {stderr}"
    );
    assert!(
        !stderr.contains("-lost-before-sync"),
        "stderr should not include an unrecoverable task nickname: {stderr}"
    );
    assert!(
        !stderr.contains("secret@example.invalid"),
        "stderr should not leak token-bearing remote details: {stderr}"
    );
    assert!(
        !stderr.contains("private/missing-origin.git"),
        "stderr should not leak private remote paths: {stderr}"
    );
    assert!(!repo
        .git_output(["show-ref", "--verify", "refs/heads/tiber"])
        .status
        .success());
}

#[test]
fn show_resolves_by_id_nickname_or_full_stem_without_storage_paths() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Write tiber docs"]));
    let stem = task_stem(&repo, "backlog", "write-tiber-docs");
    let id = stem
        .split_once("-write-tiber-docs")
        .map(|(id, _)| id)
        .expect("stem includes nickname")
        .to_string();

    for task_ref in [id.as_str(), "write-tiber-docs", stem.as_str()] {
        let show = repo.tiber(["show", task_ref]);

        assert_success_ref(&show);
        assert!(
            String::from_utf8(show.stdout)
                .expect("show output should be utf8")
                .contains("title: Write tiber docs"),
            "show should print task for ref {task_ref}"
        );
    }
}
