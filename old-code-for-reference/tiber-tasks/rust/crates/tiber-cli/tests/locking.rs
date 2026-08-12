pub mod support;

use std::fs;
use std::fs::OpenOptions;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use support::{assert_success, TempRepo};

#[test]
fn write_commands_fail_when_tiber_lock_is_held() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    let git_common_dir = repo.git_output(["rev-parse", "--git-common-dir"]);
    assert_success(git_common_dir.clone());
    let git_common_dir =
        String::from_utf8(git_common_dir.stdout).expect("git common dir should be utf8");
    let lock_dir = repo.path().join(git_common_dir.trim()).join("tiber");
    fs::create_dir_all(&lock_dir).expect("create tiber lock dir");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_dir.join("tiber.advisory.lock"))
        .expect("open lock file");
    lock_file.lock().expect("hold lock file");

    let create = repo.tiber_with_env(
        ["create", "Blocked by lock"],
        [("TIBER_LOCK_RETRY_TIMEOUT_MS", "0")],
    );

    assert!(!create.status.success(), "write command should fail");
    let stderr = String::from_utf8(create.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("tiber_lock_busy"));
    let tree = repo.tiber(["list", "--status", "backlog"]);
    assert_success(tree.clone());
    assert!(!String::from_utf8(tree.stdout)
        .expect("tree output should be utf8")
        .contains("blocked-by-lock"));
    drop(lock_file);
}

#[test]
fn write_commands_retry_when_tiber_lock_is_released() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    let git_common_dir = repo.git_output(["rev-parse", "--git-common-dir"]);
    assert_success(git_common_dir.clone());
    let git_common_dir =
        String::from_utf8(git_common_dir.stdout).expect("git common dir should be utf8");
    let lock_dir = repo.path().join(git_common_dir.trim()).join("tiber");
    fs::create_dir_all(&lock_dir).expect("create tiber lock dir");
    let lock_path = lock_dir.join("tiber.advisory.lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open lock file");
    lock_file.lock().expect("hold lock file");

    let releaser = {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            drop(lock_file);
        })
    };

    let create = repo.tiber_with_env(
        ["create", "Retried after lock"],
        [
            ("TIBER_LOCK_RETRY_TIMEOUT_MS", "1000"),
            ("TIBER_LOCK_RETRY_INTERVAL_MS", "20"),
        ],
    );

    releaser.join().expect("lock releaser should finish");
    assert_success(create);
    let tree = repo.tiber(["list", "--status", "backlog"]);
    assert_success(tree.clone());
    assert!(String::from_utf8(tree.stdout)
        .expect("tree output should be utf8")
        .contains("retried-after-lock"));
}

#[test]
fn write_commands_remain_excluded_by_a_live_legacy_sentinel() {
    let repo = TempRepo::initialized();
    assert_success(repo.tiber(["init"]));
    let git_common_dir = repo.git_output(["rev-parse", "--git-common-dir"]);
    assert_success(git_common_dir.clone());
    let git_common_dir =
        String::from_utf8(git_common_dir.stdout).expect("git common dir should be utf8");
    let lock_dir = repo.path().join(git_common_dir.trim()).join("tiber");
    fs::create_dir_all(&lock_dir).expect("create tiber lock dir");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_secs();
    fs::write(
        lock_dir.join("tiber.lock"),
        format!("pid={}\ntimestamp={timestamp}\n", std::process::id()),
    )
    .expect("write live legacy lock");

    let create = repo.tiber_with_env(
        ["create", "Blocked by legacy client"],
        [("TIBER_LOCK_RETRY_TIMEOUT_MS", "0")],
    );

    assert!(
        !create.status.success()
            && String::from_utf8(create.stderr)
                .expect("stderr should be utf8")
                .contains("tiber_lock_busy"),
        "a live 0.11-style sentinel must exclude a 0.12 client"
    );
}
