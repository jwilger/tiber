pub mod support;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;
use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn concurrent_clones_elect_one_ci_recovery_owner_and_one_waiter() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);

    let seed = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(seed.path())
            .output()
            .expect("add origin remote"),
    );
    seed.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    assert_success(seed.tiber(["init"]));
    assert_success(seed.tiber(["sync"]));

    let first = clone_repo(&origin);
    let second = clone_repo(&origin);
    install_push_barrier(&origin);
    let claim_args = [
        "ci-recovery",
        "claim",
        "--run-id",
        "991",
        "--run-url",
        "https://forge.example.invalid/runs/991",
        "--failed-sha",
        "0123456789abcdef",
        "--workflow",
        "CI",
        "--ref",
        "refs/heads/main",
    ];

    let first_claim = ci_recovery_claim_command(&first, &claim_args, "first-host", "first-session")
        .spawn()
        .expect("start first claim");
    let second_claim =
        ci_recovery_claim_command(&second, &claim_args, "second-host", "second-session")
            .spawn()
            .expect("start second claim");
    let first_claim = first_claim.wait_with_output().expect("finish first claim");
    assert_success_ref(&first_claim);
    let second_claim = second_claim
        .wait_with_output()
        .expect("finish second claim");
    assert_success_ref(&second_claim);

    let first_result: serde_json::Value =
        serde_json::from_slice(&first_claim.stdout).expect("first claim returns JSON");
    let second_result: serde_json::Value =
        serde_json::from_slice(&second_claim.stdout).expect("second claim returns JSON");

    let roles = [&first_result["role"], &second_result["role"]];
    assert_eq!(roles.iter().filter(|role| ***role == "owner").count(), 1);
    assert_eq!(roles.iter().filter(|role| ***role == "waiting").count(), 1);
    assert_eq!(first_result["incident_id"], second_result["incident_id"]);
}

#[test]
fn ci_recovery_claim_refuses_an_unknown_session_identity() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let repo = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(repo.path())
            .output()
            .expect("add origin remote"),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args([
            "ci-recovery",
            "claim",
            "--run-id",
            "992",
            "--run-url",
            "https://forge.example.invalid/runs/992",
            "--failed-sha",
            "fedcba9876543210",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ])
        .env_remove("TIBER_CLAIM_SESSION")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CLAUDE_SESSION_ID")
        .current_dir(repo.path())
        .output()
        .expect("run claim without session identity");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ci_recovery_session_required"));
}

#[test]
fn ci_recovery_rejects_credential_material_without_echoing_it() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let repo = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(repo.path())
            .output()
            .expect("add origin remote"),
    );
    repo.git(["push", "origin", "main"]);
    let credential = "token=should-not-be-echoed";
    let output = repo.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "credential",
            "--run-url",
            credential,
            "--failed-sha",
            "cafebabe",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ci_recovery_field_invalid field=run_url"));
    assert!(!stderr.contains(credential));
}

#[test]
fn ci_recovery_assertion_allows_only_the_current_owner_epoch() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let waiter = clone_repo(&origin);
    let claim_args = [
        "ci-recovery",
        "claim",
        "--run-id",
        "993",
        "--run-url",
        "https://forge.example.invalid/runs/993",
        "--failed-sha",
        "abcdef0123456789",
        "--workflow",
        "CI",
        "--ref",
        "refs/heads/main",
    ];
    assert_success(owner.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    ));
    assert_success(waiter.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "waiter-host"),
            ("TIBER_CLAIM_SESSION", "waiter-session"),
        ],
    ));

    let waiter_assertion = waiter.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-993",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "waiter-host"),
            ("TIBER_CLAIM_SESSION", "waiter-session"),
        ],
    );
    assert!(!waiter_assertion.status.success());
    assert!(String::from_utf8_lossy(&waiter_assertion.stderr).contains("ci_recovery_not_owner"));

    let stale_owner_assertion = owner.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-993",
            "--epoch",
            "0",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert!(!stale_owner_assertion.status.success());
    assert!(
        String::from_utf8_lossy(&stale_owner_assertion.stderr).contains("ci_recovery_stale_epoch")
    );

    let owner_assertion = owner.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-993",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert_success_ref(&owner_assertion);
    let assertion: serde_json::Value =
        serde_json::from_slice(&owner_assertion.stdout).expect("owner assertion returns JSON");
    assert_eq!(assertion["allowed"], true);
}

#[test]
fn ci_recovery_transfer_fences_the_old_owner_and_authorizes_the_recipient() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let recipient = clone_repo(&origin);
    let claim_args = [
        "ci-recovery",
        "claim",
        "--run-id",
        "994",
        "--run-url",
        "https://forge.example.invalid/runs/994",
        "--failed-sha",
        "1234567890abcdef",
        "--workflow",
        "CI",
        "--ref",
        "refs/heads/main",
    ];
    assert_success(owner.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    ));
    assert_success(recipient.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "recipient-host"),
            ("TIBER_CLAIM_SESSION", "recipient-session"),
        ],
    ));

    let unauthorized = recipient.tiber_with_env(
        [
            "ci-recovery",
            "transfer",
            "--incident-id",
            "ci-994",
            "--epoch",
            "1",
            "--to-host",
            "recipient-host",
            "--to-session",
            "recipient-session",
        ],
        [
            ("TIBER_CLAIM_HOST", "recipient-host"),
            ("TIBER_CLAIM_SESSION", "recipient-session"),
        ],
    );
    assert!(!unauthorized.status.success());
    assert!(String::from_utf8_lossy(&unauthorized.stderr).contains("ci_recovery_not_owner"));

    let transfer = owner.tiber_with_env(
        [
            "ci-recovery",
            "transfer",
            "--incident-id",
            "ci-994",
            "--epoch",
            "1",
            "--to-host",
            "recipient-host",
            "--to-session",
            "recipient-session",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert_success_ref(&transfer);
    let transfer: serde_json::Value =
        serde_json::from_slice(&transfer.stdout).expect("transfer returns JSON");
    assert_eq!(transfer["epoch"], 2);

    let old_owner = owner.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-994",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert!(!old_owner.status.success());
    assert!(String::from_utf8_lossy(&old_owner.stderr).contains("ci_recovery_stale_epoch"));

    let new_owner = recipient.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-994",
            "--epoch",
            "2",
        ],
        [
            ("TIBER_CLAIM_HOST", "recipient-host"),
            ("TIBER_CLAIM_SESSION", "recipient-session"),
        ],
    );
    assert_success(new_owner);
}

#[test]
fn ci_recovery_takeover_requires_an_expired_lease_and_increments_the_epoch() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let successor = clone_repo(&origin);
    let claim_args = [
        "ci-recovery",
        "claim",
        "--run-id",
        "995",
        "--run-url",
        "https://forge.example.invalid/runs/995",
        "--failed-sha",
        "456789abcdef0123",
        "--workflow",
        "CI",
        "--ref",
        "refs/heads/main",
    ];
    assert_success(owner.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "1000"),
        ],
    ));

    let early = successor.tiber_with_env(
        [
            "ci-recovery",
            "takeover",
            "--incident-id",
            "ci-995",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "successor-host"),
            ("TIBER_CLAIM_SESSION", "successor-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "4599"),
        ],
    );
    assert!(!early.status.success());
    assert!(String::from_utf8_lossy(&early.stderr).contains("ci_recovery_lease_active"));

    let takeover = successor.tiber_with_env(
        [
            "ci-recovery",
            "takeover",
            "--incident-id",
            "ci-995",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "successor-host"),
            ("TIBER_CLAIM_SESSION", "successor-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "4600"),
        ],
    );
    assert_success_ref(&takeover);
    let takeover: serde_json::Value =
        serde_json::from_slice(&takeover.stdout).expect("takeover returns JSON");
    assert_eq!(takeover["epoch"], 2);

    let successor_assertion = successor.tiber_with_env(
        [
            "ci-recovery",
            "assert-owner",
            "--incident-id",
            "ci-995",
            "--epoch",
            "2",
        ],
        [
            ("TIBER_CLAIM_HOST", "successor-host"),
            ("TIBER_CLAIM_SESSION", "successor-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "4600"),
        ],
    );
    assert_success(successor_assertion);
    let successor_env = [
        ("TIBER_CLAIM_HOST", "successor-host"),
        ("TIBER_CLAIM_SESSION", "successor-session"),
        ("TIBER_CI_RECOVERY_TEST_NOW", "4600"),
    ];
    assert_success(successor.tiber_with_env(
        [
            "ci-recovery",
            "diagnose",
            "--incident-id",
            "ci-995",
            "--epoch",
            "2",
            "--job",
            "test",
            "--step",
            "Run tests",
            "--log-evidence",
            "runner disconnected",
            "--cause",
            "runner interruption",
            "--classification",
            "transient",
        ],
        successor_env,
    ));
    assert_success(successor.tiber_with_env(
        [
            "ci-recovery",
            "choose-action",
            "--incident-id",
            "ci-995",
            "--epoch",
            "2",
            "--kind",
            "rerun",
            "--description",
            "rerun unchanged revision",
        ],
        successor_env,
    ));
    assert_success(successor.tiber_with_env(
        [
            "ci-recovery",
            "record-replacement",
            "--incident-id",
            "ci-995",
            "--epoch",
            "2",
            "--run-id",
            "995-r1",
            "--run-url",
            "https://forge.example.invalid/runs/995-r1",
            "--sha",
            "456789abcdef0123",
            "--status",
            "running",
        ],
        successor_env,
    ));
    assert_success(successor.tiber_with_env(
        [
            "ci-recovery",
            "resolve",
            "--incident-id",
            "ci-995",
            "--replacement-run-id",
            "995-r1",
            "--replacement-run-url",
            "https://forge.example.invalid/runs/995-r1",
            "--sha",
            "456789abcdef0123",
            "--terminal-status",
            "success",
        ],
        successor_env,
    ));
}

#[test]
fn ci_recovery_hold_releases_only_after_exact_terminal_success_proof() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let observer = clone_repo(&origin);
    let owner_env = [
        ("TIBER_CLAIM_HOST", "owner-host"),
        ("TIBER_CLAIM_SESSION", "owner-session"),
        ("TIBER_CI_RECOVERY_TEST_NOW", "1000"),
    ];
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996",
            "--run-url",
            "https://forge.example.invalid/runs/996",
            "--failed-sha",
            "789abcdef0123456",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        owner_env,
    ));
    assert_success(observer.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996",
            "--run-url",
            "https://forge.example.invalid/runs/996",
            "--failed-sha",
            "789abcdef0123456",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    ));
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "diagnose",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--job",
            "test",
            "--step",
            "Run tests",
            "--log-evidence",
            "runner disconnected after checkout",
            "--cause",
            "host runner interruption",
            "--classification",
            "transient",
        ],
        owner_env,
    ));
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "choose-action",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--kind",
            "rerun",
            "--description",
            "rerun the unchanged revision without an intervening push",
        ],
        owner_env,
    ));
    let wrong_rerun_sha = owner.tiber_with_env(
        [
            "ci-recovery",
            "record-replacement",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--run-id",
            "996-wrong",
            "--run-url",
            "https://forge.example.invalid/runs/996-wrong",
            "--sha",
            "0000000000000000",
            "--status",
            "running",
        ],
        owner_env,
    );
    assert!(!wrong_rerun_sha.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_rerun_sha.stderr).contains("ci_recovery_rerun_sha_mismatch")
    );
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "record-replacement",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--run-id",
            "996-r0",
            "--run-url",
            "https://forge.example.invalid/runs/996-r0",
            "--sha",
            "789abcdef0123456",
            "--status",
            "failed",
        ],
        owner_env,
    ));
    let failed_release = observer.tiber_with_env(
        [
            "ci-recovery",
            "resolve",
            "--incident-id",
            "ci-996",
            "--replacement-run-id",
            "996-r0",
            "--replacement-run-url",
            "https://forge.example.invalid/runs/996-r0",
            "--sha",
            "789abcdef0123456",
            "--terminal-status",
            "success",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    );
    assert!(!failed_release.status.success());
    assert!(
        String::from_utf8_lossy(&failed_release.stderr).contains("ci_recovery_replacement_failed")
    );
    let failed_replacement_claim = observer.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996-r0",
            "--run-url",
            "https://forge.example.invalid/runs/996-r0",
            "--failed-sha",
            "789abcdef0123456",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    );
    assert_success_ref(&failed_replacement_claim);
    let failed_replacement_claim: serde_json::Value =
        serde_json::from_slice(&failed_replacement_claim.stdout)
            .expect("failed replacement claim returns JSON");
    assert_eq!(failed_replacement_claim["incident_id"], "ci-996");
    assert_eq!(failed_replacement_claim["role"], "waiting");
    let status = observer.tiber(["ci-recovery", "status"]);
    assert_success_ref(&status);
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status returns JSON");
    assert_eq!(status["trigger_count"], 2);
    assert_eq!(status["trigger"]["run_id"], "996-r0");
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "diagnose",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--job",
            "test",
            "--step",
            "Run tests",
            "--log-evidence",
            "runner disconnected after checkout",
            "--cause",
            "host runner interruption",
            "--classification",
            "transient",
        ],
        owner_env,
    ));
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "choose-action",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--kind",
            "rerun",
            "--description",
            "rerun the unchanged revision without an intervening push",
        ],
        owner_env,
    ));
    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "record-replacement",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--run-id",
            "996-r1",
            "--run-url",
            "https://forge.example.invalid/runs/996-r1",
            "--sha",
            "789abcdef0123456",
            "--status",
            "running",
        ],
        owner_env,
    ));

    let takeover = observer.tiber_with_env(
        [
            "ci-recovery",
            "takeover",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "5000"),
        ],
    );
    assert_success_ref(&takeover);
    let takeover: serde_json::Value =
        serde_json::from_slice(&takeover.stdout).expect("waiting-ci takeover returns JSON");
    assert_eq!(takeover["epoch"], 2);

    assert_success(observer.tiber_with_env(
        [
            "ci-recovery",
            "resolve",
            "--incident-id",
            "ci-996",
            "--replacement-run-id",
            "996-r1",
            "--replacement-run-url",
            "https://forge.example.invalid/runs/996-r1",
            "--sha",
            "789abcdef0123456",
            "--terminal-status",
            "success",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    ));
    let status = observer.tiber(["ci-recovery", "status"]);
    assert_success_ref(&status);
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status returns JSON");
    assert_eq!(status["state"], "resolved");
    assert_eq!(status["hold_released"], true);
    assert_eq!(status["owner"]["host"], "observer-host");
    assert_eq!(status["triggers"][0]["failed_sha"], "789abcdef0123456");
    assert_eq!(status["diagnosis"]["classification"], "transient");
    assert_eq!(status["next_action"]["kind"], "rerun");
    assert_eq!(status["replacement"]["run_id"], "996-r1");
    assert_eq!(status["release_proof"]["terminal_status"], "success");

    let resolved_mutation = observer.tiber_with_env(
        [
            "ci-recovery",
            "heartbeat",
            "--incident-id",
            "ci-996",
            "--epoch",
            "2",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    );
    assert!(!resolved_mutation.status.success());
    assert!(String::from_utf8_lossy(&resolved_mutation.stderr)
        .contains("ci_recovery_incident_resolved"));

    let next_claim = observer.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "next-995",
            "--run-url",
            "https://forge.example.invalid/runs/next-995",
            "--failed-sha",
            "fedcba9876543210",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "observer-host"),
            ("TIBER_CLAIM_SESSION", "observer-session"),
        ],
    );
    assert_success_ref(&next_claim);
    let next_claim: serde_json::Value =
        serde_json::from_slice(&next_claim.stdout).expect("new incident claim returns JSON");
    assert_eq!(next_claim["incident_id"], "ci-next-995");
    assert_eq!(next_claim["role"], "owner");
    assert_eq!(next_claim["epoch"], 1);
}

#[test]
fn ci_recovery_owner_assigns_bounded_helper_capabilities_and_renews_its_lease() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let helper = clone_repo(&origin);
    let intruder = clone_repo(&origin);
    let claim_args = [
        "ci-recovery",
        "claim",
        "--run-id",
        "997",
        "--run-url",
        "https://forge.example.invalid/runs/997",
        "--failed-sha",
        "abcdef7890123456",
        "--workflow",
        "CI",
        "--ref",
        "refs/heads/main",
    ];
    assert_success(owner.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "1000"),
        ],
    ));
    assert_success(helper.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    ));
    assert_success(intruder.tiber_with_env(
        claim_args,
        [
            ("TIBER_CLAIM_HOST", "intruder-host"),
            ("TIBER_CLAIM_SESSION", "intruder-session"),
        ],
    ));

    let forbidden = owner.tiber_with_env(
        [
            "ci-recovery",
            "assign",
            "--incident-id",
            "ci-997",
            "--epoch",
            "1",
            "--to-host",
            "helper-host",
            "--to-session",
            "helper-session",
            "--capabilities",
            "inspect,push",
            "--scope",
            "inspect failed test logs",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "1000"),
        ],
    );
    assert!(!forbidden.status.success());
    assert!(String::from_utf8_lossy(&forbidden.stderr).contains("ci_recovery_capability_invalid"));

    let assignment = owner.tiber_with_env(
        [
            "ci-recovery",
            "assign",
            "--incident-id",
            "ci-997",
            "--epoch",
            "1",
            "--to-host",
            "helper-host",
            "--to-session",
            "helper-session",
            "--capabilities",
            "inspect,test",
            "--scope",
            "inspect and reproduce the failed test",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "1000"),
        ],
    );
    assert_success_ref(&assignment);
    let assignment: serde_json::Value =
        serde_json::from_slice(&assignment.stdout).expect("assignment returns JSON");
    assert_eq!(assignment["assignment_id"], "a1");

    let wrong_assignee = intruder.tiber_with_env(
        [
            "ci-recovery",
            "report",
            "--incident-id",
            "ci-997",
            "--assignment-id",
            "a1",
            "--summary",
            "should not be accepted",
            "--evidence",
            "wrong participant",
        ],
        [
            ("TIBER_CLAIM_HOST", "intruder-host"),
            ("TIBER_CLAIM_SESSION", "intruder-session"),
        ],
    );
    assert!(!wrong_assignee.status.success());
    assert!(String::from_utf8_lossy(&wrong_assignee.stderr)
        .contains("ci_recovery_assignment_not_assignee"));

    assert_success(helper.tiber_with_env(
        [
            "ci-recovery",
            "report",
            "--incident-id",
            "ci-997",
            "--assignment-id",
            "a1",
            "--summary",
            "reproduced the runner disconnect",
            "--evidence",
            "local reproduction exits at the same step",
        ],
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    ));

    let attached_failure = helper.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996-secondary",
            "--run-url",
            "https://ci.example.test/runs/996-secondary",
            "--failed-sha",
            "deadbeef996b",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    );
    assert!(!attached_failure.status.success());
    assert!(String::from_utf8_lossy(&attached_failure.stderr)
        .contains("ci_recovery_distinct_trigger_requires_separate_incident"));
    let status = owner.tiber(["ci-recovery", "status"]);
    assert_success_ref(&status);
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status returns JSON");
    assert_eq!(status["trigger_count"], 1);

    let heartbeat = owner.tiber_with_env(
        [
            "ci-recovery",
            "heartbeat",
            "--incident-id",
            "ci-997",
            "--epoch",
            "1",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "2000"),
        ],
    );
    assert_success_ref(&heartbeat);
    let heartbeat: serde_json::Value =
        serde_json::from_slice(&heartbeat.stdout).expect("heartbeat returns JSON");
    assert_eq!(heartbeat["lease_expires_at"], 5600);

    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "transfer",
            "--incident-id",
            "ci-997",
            "--epoch",
            "1",
            "--to-host",
            "helper-host",
            "--to-session",
            "helper-session",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
            ("TIBER_CI_RECOVERY_TEST_NOW", "2000"),
        ],
    ));
    let stale_report = helper.tiber_with_env(
        [
            "ci-recovery",
            "report",
            "--incident-id",
            "ci-997",
            "--assignment-id",
            "a1",
            "--summary",
            "stale report",
            "--evidence",
            "old owner epoch",
        ],
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    );
    assert!(!stale_report.status.success());
    assert!(String::from_utf8_lossy(&stale_report.stderr).contains("ci_recovery_assignment_stale"));
}

#[test]
fn ci_recovery_wait_is_bounded_and_wakes_for_a_helper_assignment() {
    let origin = TempRepo::new();
    origin.git(["init", "--bare"]);
    let owner = TempRepo::initialized();
    assert_success(
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin.path())
            .current_dir(owner.path())
            .output()
            .expect("add origin remote"),
    );
    owner.git(["push", "origin", "main"]);
    origin.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
    let helper = clone_repo(&origin);

    let owner_claim = owner.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996",
            "--run-url",
            "https://ci.example.test/runs/996",
            "--failed-sha",
            "deadbeef996",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    );
    assert_success_ref(&owner_claim);
    assert_success(helper.tiber_with_env(
        [
            "ci-recovery",
            "claim",
            "--run-id",
            "996",
            "--run-url",
            "https://ci.example.test/runs/996",
            "--failed-sha",
            "deadbeef996",
            "--workflow",
            "CI",
            "--ref",
            "refs/heads/main",
        ],
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    ));

    let timeout = helper.tiber_with_env(
        [
            "ci-recovery",
            "wait",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--timeout-seconds",
            "0",
        ],
        [
            ("TIBER_CLAIM_HOST", "helper-host"),
            ("TIBER_CLAIM_SESSION", "helper-session"),
        ],
    );
    assert_success_ref(&timeout);
    let timeout: serde_json::Value =
        serde_json::from_slice(&timeout.stdout).expect("wait timeout returns JSON");
    assert_eq!(timeout["wake_reason"], "timeout");
    assert_eq!(timeout["assignment_id"], serde_json::Value::Null);

    let wait_ready = helper.path().join("ci-recovery-wait-ready");
    let waiting = Command::new(env!("CARGO_BIN_EXE_tiber"))
        .args([
            "ci-recovery",
            "wait",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--timeout-seconds",
            "5",
        ])
        .env("TIBER_CLAIM_HOST", "helper-host")
        .env("TIBER_CLAIM_SESSION", "helper-session")
        .env("TIBER_CI_RECOVERY_TEST_WAIT_READY", &wait_ready)
        .current_dir(helper.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start bounded helper wait");
    for _ in 0..100 {
        if wait_ready.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        wait_ready.exists(),
        "helper wait should enter its polling loop before assignment"
    );

    assert_success(owner.tiber_with_env(
        [
            "ci-recovery",
            "assign",
            "--incident-id",
            "ci-996",
            "--epoch",
            "1",
            "--to-host",
            "helper-host",
            "--to-session",
            "helper-session",
            "--capabilities",
            "inspect",
            "--scope",
            "inspect the failed job",
        ],
        [
            ("TIBER_CLAIM_HOST", "owner-host"),
            ("TIBER_CLAIM_SESSION", "owner-session"),
        ],
    ));

    let assigned = waiting
        .wait_with_output()
        .expect("finish bounded helper wait");
    assert_success_ref(&assigned);
    let assigned: serde_json::Value =
        serde_json::from_slice(&assigned.stdout).expect("assigned wait returns JSON");
    assert_eq!(assigned["wake_reason"], "assignment");
    assert_eq!(assigned["assignment_id"], "a1");
}

fn clone_repo(origin: &TempRepo) -> TempRepo {
    let clone = TempRepo::new();
    assert_success(
        Command::new("git")
            .arg("clone")
            .arg(origin.path())
            .arg(clone.path())
            .output()
            .expect("clone repository"),
    );
    clone.git(["config", "user.email", "tiber@example.test"]);
    clone.git(["config", "user.name", "Tiber Test"]);
    clone.git(["config", "commit.gpgsign", "false"]);
    clone
}

fn ci_recovery_claim_command(repo: &TempRepo, args: &[&str], host: &str, session: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tiber"));
    command
        .args(args)
        .env("TIBER_CLAIM_HOST", host)
        .env("TIBER_CLAIM_SESSION", session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(repo.path());
    command
}

fn install_push_barrier(origin: &TempRepo) {
    let hook = origin.path().join("hooks/pre-receive");
    fs::write(
        &hook,
        r#"#!/bin/sh
barrier="$GIT_DIR/ci-recovery-barrier"
mkdir -p "$barrier"
touch "$barrier/arrival-$$"
attempt=0
while [ "$(find "$barrier" -type f | wc -l)" -lt 2 ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 200 ]; then
    echo "ci recovery test barrier timed out" >&2
    exit 1
  fi
  sleep 0.01
done
"#,
    )
    .expect("write pre-receive barrier");
    #[cfg(unix)]
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
        .expect("make pre-receive barrier executable");
}
