pub mod support;

use std::fs;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn scaffold_repo_dry_run_previews_and_apply_writes_files() {
    let repo = TempRepo::initialized();

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let dry_run_stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(!dry_run_stdout.contains(".gitignore"));
    assert!(dry_run_stdout.contains("skipped hook-dispatch"));
    assert!(dry_run_stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!repo.path().join(".gitignore").exists());
    assert!(!repo
        .path()
        .join(".githooks")
        .join("post-commit.tiber")
        .exists());
    assert!(!repo
        .path()
        .join(".github")
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert!(!repo.path().join(".gitignore").exists());
    assert!(!repo.path().join(".githooks/post-commit.tiber").exists());
    assert!(repo
        .path()
        .join(".github")
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());
    let workflow = fs::read_to_string(
        repo.path()
            .join(".github/workflows/tiber-close-from-trailers.yml"),
    )
    .expect("read generated workflow");
    assert!(workflow.contains("permissions:\n  contents: write\n"));
    assert!(!workflow.contains("write-all"));
    assert!(workflow.contains("actions/checkout@"));
    assert!(!workflow.contains("actions/checkout@v4"));
    assert!(
        workflow.contains("git -C .tiber-src checkout 68823dd13951586e62108dac1602ce4a45560aaf")
    );
    assert!(workflow.contains("cargo install --locked --path"));
}

#[test]
fn scaffold_repo_targets_the_repository_publication_branch() {
    let repo = TempRepo::initialized();
    repo.git(["branch", "-m", "release,canary"]);

    assert_success(repo.tiber(["scaffold", "repo", "--apply"]));

    let workflow = fs::read_to_string(
        repo.path()
            .join(".github/workflows/tiber-close-from-trailers.yml"),
    )
    .expect("read generated workflow");
    assert!(workflow.contains("branches: ['release,canary']"));
}

#[test]
fn scaffold_repo_reports_that_an_undispatched_hook_snippet_is_inert() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("conflict hook-dispatch"));
    assert!(stdout.contains(".githooks/post-commit"));
    assert!(stdout.contains("must invoke .githooks/post-commit.tiber"));

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);
    assert!(
        !apply.status.success(),
        "inert automation must not be installed"
    );
    assert!(!repo.path().join(".githooks/post-commit.tiber").exists());
    assert!(!repo.path().join(".gitignore").exists());
}

#[test]
fn scaffold_repo_accepts_an_active_hook_that_dispatches_the_tiber_snippet() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    fs::create_dir_all(repo.path().join(".githooks")).expect("create hooks directory");
    fs::write(
        repo.path().join(".githooks/post-commit"),
        "#!/usr/bin/env bash\n.githooks/post-commit.tiber\n",
    )
    .expect("write dispatcher");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            repo.path().join(".githooks/post-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("make dispatcher executable");
    }

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .githooks/post-commit.tiber"));
    assert!(!stdout.contains("conflict hook-dispatch"));
}

#[test]
fn scaffold_repo_rejects_inert_mentions_of_the_tiber_snippet() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    fs::create_dir_all(repo.path().join(".githooks")).expect("create hooks directory");
    fs::write(
        repo.path().join(".githooks/post-commit"),
        "#!/usr/bin/env bash\n# TODO: invoke .githooks/post-commit.tiber\necho .githooks/post-commit.tiber\nexit 0; .githooks/post-commit.tiber\nfalse && .githooks/post-commit.tiber\nif false; then\n  .githooks/post-commit.tiber\nfi\n",
    )
    .expect("write inert hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            repo.path().join(".githooks/post-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("make hook executable");
    }

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("conflict hook-dispatch"));
    let apply = repo.tiber(["scaffold", "repo", "--apply"]);
    assert!(!apply.status.success());
    assert!(!repo.path().join(".githooks/post-commit.tiber").exists());
    assert!(!repo.path().join(".gitignore").exists());
}

#[test]
fn scaffold_repo_rejects_a_dispatch_after_an_earlier_exit() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    fs::create_dir_all(repo.path().join(".githooks")).expect("create hooks directory");
    fs::write(
        repo.path().join(".githooks/post-commit"),
        "#!/usr/bin/env bash\nexit 0\n.githooks/post-commit.tiber\n",
    )
    .expect("write unreachable dispatcher");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            repo.path().join(".githooks/post-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("make hook executable");
    }

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("conflict hook-dispatch"));
}

#[test]
fn scaffold_repo_refuses_unsigned_workflow_for_signed_publication_policy() {
    let repo = TempRepo::initialized();
    repo.git(["config", "commit.gpgsign", "true"]);

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("conflict signed-publication"));
    assert!(stdout.contains("generated GitHub workflow cannot access a signing key"));
    assert!(!stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);
    assert!(!apply.status.success());
    assert!(!repo.path().join(".gitignore").exists());
}

#[test]
fn scaffold_repo_leaves_existing_gitignore_untouched() {
    let repo = TempRepo::initialized();
    let existing = "target/\n.env\ncoverage/\n";
    fs::write(repo.path().join(".gitignore"), existing).expect("write existing gitignore");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    let gitignore = fs::read_to_string(repo.path().join(".gitignore")).expect("read gitignore");
    assert_eq!(gitignore, existing);
}

#[test]
fn scaffold_repo_ignores_non_utf8_gitignore() {
    let repo = TempRepo::initialized();
    let existing = b"target/\n\xff\n";
    fs::write(repo.path().join(".gitignore"), existing).expect("write non-utf8 gitignore");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert_eq!(
        fs::read(repo.path().join(".gitignore")).expect("read unchanged gitignore"),
        existing
    );
}

#[test]
fn scaffold_repo_detects_an_equivalent_existing_workflow() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    let workflow =
        "name: close tasks\non: push\njobs:\n  close:\n    steps:\n      - run: tiber close-from-trailers\n";
    fs::write(&workflow_path, workflow).expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
    assert!(stdout.contains("skipped hook-dispatch"));

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert_eq!(
        fs::read_to_string(&workflow_path).expect("read existing workflow"),
        workflow
    );
    assert!(!repo
        .path()
        .join(".github")
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());
    assert!(!repo
        .path()
        .join(".githooks")
        .join("post-commit.tiber")
        .exists());
}

#[test]
fn scaffold_repo_does_not_treat_a_manual_workflow_as_equivalent() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("manual-close.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        workflow_path,
        "name: manual close\non: workflow_dispatch\njobs:\n  close:\n    steps:\n      - run: tiber close-from-trailers\n",
    )
    .expect("write manual workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/manual-close.yaml"));
}

#[test]
fn scaffold_repo_bounds_block_event_detection_to_the_on_mapping() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("manual-close.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        workflow_path,
        "name: manual close\non:\n  workflow_dispatch:\njobs:\n  close:\n    env:\n      push: enabled\n    steps:\n      - run: tiber close-from-trailers\n",
    )
    .expect("write manual workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/manual-close.yaml"));
}

#[test]
fn scaffold_repo_detects_push_in_a_block_event_sequence() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non:\n  - push\n  - pull_request\njobs:\n  close:\n    steps:\n      - run: tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_detects_a_task_closer_behind_a_nix_wrapper() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non: push\njobs:\n  close:\n    steps:\n      - run: nix develop -c tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_detects_a_task_closer_in_a_command_list() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non: push\njobs:\n  close:\n    steps:\n      - run: tiber close-from-trailers && echo closed\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_detects_a_task_closer_after_an_earlier_command() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non: push\njobs:\n  close:\n    steps:\n      - run: echo preparing && tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_does_not_split_an_escaped_command_separator() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("build.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: build\non: push\njobs:\n  build:\n    steps:\n      - run: echo preparing \\; tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/build.yaml"));
}

#[test]
fn scaffold_repo_preserves_an_escaped_hash_before_the_task_closer() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non: push\njobs:\n  close:\n    steps:\n      - run: |\n          echo marker\\ # && tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_treats_a_hash_after_an_operator_as_a_comment() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("build.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: build\non: push\njobs:\n  build:\n    steps:\n      - run: |\n          echo preparing;# disabled && tiber close-from-trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/build.yaml"));
}

#[test]
fn scaffold_repo_treats_a_hash_after_a_group_operator_as_a_comment() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("build.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: build\non: push\njobs:\n  build:\n    steps:\n      - run: |\n          (# disabled && tiber close-from-trailers\n            true\n          )\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/build.yaml"));
}

#[test]
fn scaffold_repo_detects_equivalent_workflow_with_inline_comments() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("close-tasks.yaml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        &workflow_path,
        "name: close tasks\non: push\njobs: # task automation\n  close:\n    steps: # close tasks\n      - run: tiber close-from-trailers # reconcile trailers\n",
    )
    .expect("write existing workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .github/workflows/close-tasks.yaml"));
    assert!(!stdout.contains(".github/workflows/tiber-close-from-trailers.yml"));
    assert!(stdout.contains("skipped hook-dispatch"));
}

#[test]
fn scaffold_repo_reports_the_path_of_a_non_utf8_workflow() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("generated.yml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(&workflow_path, b"jobs:\n\xff\n").expect("write non-utf8 workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert!(!dry_run.status.success());
    let stderr = String::from_utf8(dry_run.stderr).expect("stderr should be utf8");
    assert!(stderr.contains(".github/workflows/generated.yml"));
}

#[test]
fn scaffold_repo_does_not_treat_workflow_comments_as_automation() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("build.yml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        workflow_path,
        "name: build\n# tiber close-from-trailers\njobs:\n  build:\n    steps:\n      - run: echo build\n",
    )
    .expect("write unrelated workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/build.yml"));
}

#[test]
fn scaffold_repo_does_not_treat_action_inputs_as_automation() {
    let repo = TempRepo::initialized();
    let workflow_path = repo
        .path()
        .join(".github")
        .join("workflows")
        .join("action-input.yml");
    fs::create_dir_all(workflow_path.parent().expect("workflow parent"))
        .expect("create workflow directory");
    fs::write(
        workflow_path,
        "name: action input\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: example/action@v1\n        with:\n          run: tiber close-from-trailers\n",
    )
    .expect("write unrelated workflow");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("already configured .github/workflows/action-input.yml"));
}

#[test]
fn scaffold_repo_detects_an_equivalent_existing_hook() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    let hook_path = repo.path().join(".githooks").join("post-commit");
    fs::create_dir_all(hook_path.parent().expect("hook parent")).expect("create hook directory");
    let hook = "#!/usr/bin/env bash\nset -euo pipefail\ntiber close-from-trailers\n";
    fs::write(&hook_path, hook).expect("write existing hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .githooks/post-commit"));
    assert!(!stdout.contains(".githooks/post-commit.tiber"));
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert_eq!(
        fs::read_to_string(&hook_path).expect("read existing hook"),
        hook
    );
    assert!(!repo
        .path()
        .join(".githooks")
        .join("post-commit.tiber")
        .exists());
    assert!(repo
        .path()
        .join(".github")
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());
}

#[test]
fn scaffold_repo_detects_equivalent_hook_with_an_inline_comment() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    let hook_path = repo.path().join(".githooks").join("post-commit");
    fs::create_dir_all(hook_path.parent().expect("hook parent")).expect("create hook directory");
    fs::write(
        &hook_path,
        "#!/usr/bin/env bash\ntiber close-from-trailers # reconcile trailers\n",
    )
    .expect("write existing hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured .githooks/post-commit"));
    assert!(!stdout.contains(".githooks/post-commit.tiber"));
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
}

#[test]
fn scaffold_repo_does_not_treat_an_inactive_hook_file_as_automation() {
    let repo = TempRepo::initialized();
    repo.git(["config", "core.hooksPath", ".githooks"]);
    let hook_path = repo.path().join(".githooks").join("post-commit");
    fs::create_dir_all(hook_path.parent().expect("hook parent")).expect("create hook directory");
    fs::write(
        hook_path,
        "#!/usr/bin/env bash\ntiber close-from-trailers\n",
    )
    .expect("write non-executable hook");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("conflict hook-dispatch"));
    assert!(!stdout.contains("already configured .githooks/post-commit"));
}

#[test]
fn scaffold_repo_resolves_hooks_from_a_linked_worktree() {
    let repo = TempRepo::initialized();
    let hook_path = repo.path().join(".git").join("hooks").join("post-commit");
    let hook = "#!/usr/bin/env bash\ntiber close-from-trailers\n";
    fs::write(&hook_path, hook).expect("write common hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }
    let linked = repo.path().join(".worktrees").join("feature");
    repo.git(["worktree", "add", ".worktrees/feature", "-b", "feature"]);

    let dry_run = repo.tiber_at(&linked, ["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured"));
    assert!(!stdout.contains(".githooks/post-commit.tiber"));
}

#[test]
fn scaffold_repo_reports_repeated_setup_as_already_configured() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["scaffold", "repo", "--apply"]));

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(!stdout.contains(".gitignore"));
    assert!(stdout.contains("skipped hook-dispatch"));
    assert!(stdout.contains("already configured .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("would write"));
    assert!(!stdout.contains("conflict"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = ".github/workflows/tiber-close-from-trailers.yml";
        fs::set_permissions(repo.path().join(path), fs::Permissions::from_mode(0o444))
            .expect("make generated file read-only");
    }
    let second_apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success_ref(&second_apply);
    let stdout = String::from_utf8(second_apply.stdout).expect("apply output should be utf8");
    assert!(!stdout.contains(".gitignore"));
    assert!(stdout.contains("skipped hook-dispatch"));
    assert!(stdout.contains("already configured .github/workflows/tiber-close-from-trailers.yml"));
    assert!(!stdout.contains("wrote"));
}

#[test]
fn scaffold_repo_reports_conflicts_and_refuses_ambiguous_overwrites_atomically() {
    let repo = TempRepo::initialized();
    configure_default_hook_dispatcher(&repo);
    let gitignore = "target/\n.env\n";
    fs::write(repo.path().join(".gitignore"), gitignore).expect("write existing gitignore");
    let hook_path = repo.path().join(".githooks").join("post-commit.tiber");
    fs::create_dir_all(hook_path.parent().expect("hook parent")).expect("create hook directory");
    let hook = "#!/usr/bin/env bash\necho existing behavior\n";
    fs::write(&hook_path, hook).expect("write ambiguous hook");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(!stdout.contains(".gitignore"));
    assert!(stdout.contains("conflict .githooks/post-commit.tiber resolution=--replace-conflicts"));

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert!(!apply.status.success());
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).expect("read unchanged gitignore"),
        gitignore
    );
    assert_eq!(
        fs::read_to_string(&hook_path).expect("read unchanged hook"),
        hook
    );
    assert!(!repo
        .path()
        .join(".github")
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());
}

#[test]
fn scaffold_repo_preserves_existing_files_when_a_destination_cannot_be_prepared() {
    use std::os::unix::fs::symlink;

    let repo = TempRepo::initialized();
    let existing_gitignore = "target/\n";
    fs::write(repo.path().join(".gitignore"), existing_gitignore).expect("write gitignore");
    symlink(
        repo.path().join("missing-github-directory"),
        repo.path().join(".github"),
    )
    .expect("write dangling github symlink");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert!(!apply.status.success());
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).expect("read unchanged gitignore"),
        existing_gitignore
    );
}

#[test]
fn scaffold_repo_preserves_existing_files_when_a_destination_parent_is_a_live_symlink() {
    use std::os::unix::fs::symlink;

    let repo = TempRepo::initialized();
    let external = TempRepo::new();
    let existing_gitignore = "target/\n";
    fs::write(repo.path().join(".gitignore"), existing_gitignore).expect("write gitignore");
    symlink(external.path(), repo.path().join(".github")).expect("write live github symlink");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert!(!apply.status.success());
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).expect("read unchanged gitignore"),
        existing_gitignore
    );
    assert!(!external
        .path()
        .join("workflows")
        .join("tiber-close-from-trailers.yml")
        .exists());
}

#[test]
fn scaffold_repo_preserves_existing_files_when_a_destination_is_a_dangling_symlink() {
    use std::os::unix::fs::symlink;

    let repo = TempRepo::initialized();
    configure_default_hook_dispatcher(&repo);
    let existing_gitignore = "target/\n";
    fs::write(repo.path().join(".gitignore"), existing_gitignore).expect("write gitignore");
    fs::create_dir(repo.path().join(".githooks")).expect("create hooks directory");
    symlink(
        repo.path().join("missing-hook-target"),
        repo.path().join(".githooks").join("post-commit.tiber"),
    )
    .expect("write dangling hook symlink");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert!(!apply.status.success());
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).expect("read unchanged gitignore"),
        existing_gitignore
    );
}

#[test]
fn scaffold_repo_preserves_an_unowned_atomic_replacement_file() {
    let repo = TempRepo::initialized();
    fs::write(repo.path().join(".gitignore"), "target/\n").expect("write gitignore");
    let unowned = repo.path().join(".tiber-tmp-.gitignore-interrupted");
    fs::write(&unowned, "unrelated user content").expect("write unowned file");

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert_eq!(
        fs::read_to_string(unowned).expect("read unowned file"),
        "unrelated user content"
    );
}

#[test]
fn scaffold_repo_replaces_ambiguous_targets_only_with_an_explicit_choice() {
    let repo = TempRepo::initialized();
    configure_default_hook_dispatcher(&repo);
    fs::write(repo.path().join(".gitignore"), "target/\n").expect("write existing gitignore");
    let hook_path = repo.path().join(".githooks").join("post-commit.tiber");
    fs::create_dir_all(hook_path.parent().expect("hook parent")).expect("create hook directory");
    fs::write(&hook_path, "#!/usr/bin/env bash\necho existing behavior\n")
        .expect("write ambiguous hook");

    let apply = repo.tiber(["scaffold", "repo", "--apply", "--replace-conflicts"]);

    assert_success_ref(&apply);
    assert_eq!(
        fs::read_to_string(&hook_path).expect("read replaced hook"),
        "#!/usr/bin/env bash\nset -euo pipefail\n\ntiber close-from-trailers\n"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).expect("read gitignore"),
        "target/\n"
    );
}

#[test]
fn scaffold_repo_adds_show_tasks_recipe_when_justfile_exists() {
    let repo = TempRepo::initialized();
    fs::write(repo.path().join("justfile"), "test:\n  cargo test\n")
        .expect("write existing justfile");

    let dry_run = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&dry_run);
    let stdout = String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("skipped hook-dispatch"));
    assert!(!stdout.contains(".gitignore"));
    assert!(stdout.contains("would write .github/workflows/tiber-close-from-trailers.yml"));
    assert!(stdout.contains("would write justfile"));
    assert_eq!(
        fs::read_to_string(repo.path().join("justfile")).expect("read justfile"),
        "test:\n  cargo test\n"
    );

    let apply = repo.tiber(["scaffold", "repo", "--apply"]);

    assert_success(apply);
    assert_eq!(
        fs::read_to_string(repo.path().join("justfile")).expect("read justfile"),
        "test:\n  cargo test\n\nshow-tasks:\n  tiber list\n"
    );

    let second_apply = repo.tiber(["scaffold", "repo", "--apply"]);
    assert_success(second_apply);
    assert_eq!(
        fs::read_to_string(repo.path().join("justfile")).expect("read justfile"),
        "test:\n  cargo test\n\nshow-tasks:\n  tiber list\n"
    );

    let repeated_preview = repo.tiber(["scaffold", "repo", "--dry-run"]);

    assert_success_ref(&repeated_preview);
    let stdout = String::from_utf8(repeated_preview.stdout).expect("dry-run output should be utf8");
    assert!(stdout.contains("already configured justfile"));
    assert!(!stdout.contains("would write justfile"));
}

fn configure_default_hook_dispatcher(repo: &TempRepo) {
    let hook = repo.path().join(".git/hooks/post-commit");
    fs::write(
        &hook,
        "#!/usr/bin/env bash\n\"$PWD/.githooks/post-commit.tiber\"\n",
    )
    .expect("write default hook dispatcher");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
            .expect("make default hook dispatcher executable");
    }
}
