pub mod support;

use std::fs;

use support::{assert_success, TempRepo};

#[test]
fn update_without_a_field_is_a_parser_error() {
    let repo = TempRepo::initialized();

    let output = repo.tiber(["update", "missing-task"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
}

#[test]
fn update_option_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber(["update", "original-title", "--summary", "--tags"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n--tags\n"),
        "invalid invocation must not write an option token as a summary"
    );
}

#[test]
fn assigned_update_option_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber([
        "update",
        "original-title",
        "--summary",
        "--tags=should-not-be-summary",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n--tags=should-not-be-summary\n"),
        "invalid invocation must not write an assigned option token as a summary"
    );
}

#[test]
fn update_help_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber(["update", "original-title", "--summary", "--help"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n--help\n"),
        "invalid invocation must not write help as a summary"
    );
}

#[test]
fn assigned_update_help_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber(["update", "original-title", "--summary", "--help=topic"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("--summary requires a value; use --summary=--help=topic"),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n--help=topic\n"),
        "invalid invocation must not write assigned help as a summary"
    );
}

#[test]
fn update_short_help_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber(["update", "original-title", "--summary", "-h"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n-h\n"),
        "invalid invocation must not write short help as a summary"
    );
}

#[test]
fn attached_short_update_help_cannot_be_consumed_as_a_missing_summary_value() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Original title"]));

    let output = repo.tiber(["update", "original-title", "--summary", "-hfoo"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("--summary requires a value; use --summary=-hfoo"),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber update"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "original-title"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("## Summary\n\n-hfoo\n"),
        "invalid invocation must not write attached short help as a summary"
    );
}

#[test]
fn subtask_after_option_requires_a_positional_title() {
    let repo = TempRepo::initialized();

    let output = repo.tiber(["subtask", "add", "missing-task", "--after", "s1"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber subtask add"),
        "missing parser-generated usage: {stderr}"
    );
}

#[test]
fn subtask_after_option_requires_a_value_after_the_title() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    assert_success(repo.tiber(["create", "Example"]));

    let output = repo.tiber(["subtask", "add", "example", "Normal title", "--after"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber subtask add"),
        "missing parser-generated usage: {stderr}"
    );
    let shown = repo.tiber(["show", "example"]);
    assert!(
        !String::from_utf8(shown.stdout)
            .expect("show output should be utf8")
            .contains("Normal title"),
        "invalid invocation must not write a subtask"
    );
}

#[test]
fn install_mode_flag_cannot_be_consumed_as_a_reordered_target_value() {
    let repo = TempRepo::initialized();
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher directory");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let output = repo.tiber_with_env(
        ["install-bin", "--apply", "--target-dir", "--dry-run"],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("Usage: tiber install-bin"),
        "missing parser-generated usage: {stderr}"
    );
    assert!(
        !repo.path().join("--dry-run/tiber").exists(),
        "invalid invocation must not install a launcher"
    );
}

#[test]
fn install_help_flag_cannot_be_consumed_as_a_target_value() {
    let repo = TempRepo::initialized();
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher directory");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let output = repo.tiber_with_env(
        ["install-bin", "--target-dir", "--help", "--apply"],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains(
            "--target-dir requires a value; use --target-dir=--help for that literal path"
        ),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber install-bin"),
        "missing parser-generated usage: {stderr}"
    );
    assert!(
        !repo.path().join("--help/tiber").exists(),
        "invalid invocation must not install a launcher"
    );
}

#[test]
fn install_short_help_flag_cannot_be_consumed_as_a_target_value() {
    let repo = TempRepo::initialized();
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher directory");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let output = repo.tiber_with_env(
        ["install-bin", "--target-dir", "-h", "--apply"],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains("--target-dir requires a value; use --target-dir=-h for that literal path"),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber install-bin"),
        "missing parser-generated usage: {stderr}"
    );
    assert!(
        !repo.path().join("-h/tiber").exists(),
        "invalid invocation must not install a launcher"
    );
}

#[test]
fn attached_short_install_help_cannot_be_consumed_as_a_target_value() {
    let repo = TempRepo::initialized();
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher directory");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let output = repo.tiber_with_env(
        ["install-bin", "--target-dir", "-hfoo", "--apply"],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains(
            "--target-dir requires a value; use --target-dir=-hfoo for that literal path"
        ),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber install-bin"),
        "missing parser-generated usage: {stderr}"
    );
    assert!(
        !repo.path().join("-hfoo/tiber").exists(),
        "invalid invocation must not install a launcher"
    );
}

#[test]
fn assigned_install_option_cannot_be_consumed_as_a_target_value() {
    let repo = TempRepo::initialized();
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(launcher.parent().expect("launcher parent"))
        .expect("create launcher directory");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let output = repo.tiber_with_env(
        [
            "install-bin",
            "--target-dir",
            "valid-target",
            "--target-dir",
            "--target-dir=corrected",
            "--apply",
        ],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser error should be utf8");
    assert!(stderr.contains("error:"), "missing parser error: {stderr}");
    assert!(
        stderr.contains(
            "--target-dir requires a value; use --target-dir=--target-dir=corrected for that literal path"
        ),
        "missing recovery guidance: {stderr}"
    );
    assert!(
        !stderr.contains("--target-dir=valid-target for that literal path"),
        "guidance quoted the value from the wrong option occurrence: {stderr}"
    );
    assert!(
        stderr.contains("Usage: tiber install-bin"),
        "missing parser-generated usage: {stderr}"
    );
    assert!(
        !repo.path().join("--target-dir=corrected/tiber").exists(),
        "invalid invocation must not install a launcher"
    );
}
