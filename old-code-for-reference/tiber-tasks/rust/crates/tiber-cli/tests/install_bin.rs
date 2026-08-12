pub mod support;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use support::{assert_success, assert_success_ref, TempRepo};

#[test]
fn install_bin_dry_run_previews_without_writing_and_apply_installs_working_command() {
    let repo = TempRepo::initialized();
    let target_dir = repo.path().join("bin");
    let launcher = repo.path().join("plugin/bin/tiber");
    let helper = repo.path().join("plugin/scripts/helper.sh");
    fs::create_dir_all(launcher.parent().expect("launcher parent")).expect("create launcher dir");
    fs::create_dir_all(helper.parent().expect("helper parent")).expect("create helper dir");
    fs::write(
        &launcher,
        "#!/usr/bin/env bash\nset -euo pipefail\nplugin_root=\"$(cd -- \"$(dirname -- \"${BASH_SOURCE[0]}\")/..\" && pwd -P)\"\nsource \"$plugin_root/scripts/helper.sh\"\n",
    )
    .expect("write fake launcher");
    fs::write(&helper, "printf 'installed tiber works\\n'\n").expect("write helper");
    #[cfg(unix)]
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755))
        .expect("make launcher executable");

    let dry_run = repo.tiber_with_env(
        [
            "install-bin",
            "--target-dir",
            target_dir.to_str().expect("target dir utf8"),
            "--dry-run",
        ],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_success_ref(&dry_run);
    assert_eq!(
        String::from_utf8(dry_run.stdout).expect("dry-run output should be utf8"),
        format!(
            "would install {} -> {}\n",
            target_dir.join("tiber").display(),
            launcher.display()
        )
    );
    assert!(!target_dir.join("tiber").exists());

    let apply = repo.tiber_with_env(
        [
            "install-bin",
            "--target-dir",
            target_dir.to_str().expect("target dir utf8"),
            "--apply",
        ],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert_success(apply);

    let installed = Command::new(target_dir.join("tiber"))
        .current_dir(repo.path())
        .output()
        .expect("run installed tiber");
    assert_success_ref(&installed);
    assert_eq!(
        String::from_utf8(installed.stdout).expect("installed output should be utf8"),
        "installed tiber works\n"
    );
}

#[test]
fn install_bin_apply_refuses_to_replace_an_existing_target() {
    let repo = TempRepo::initialized();
    let target_dir = repo.path().join("bin");
    let installed = target_dir.join("tiber");
    let launcher = repo.path().join("plugin/bin/tiber");
    fs::create_dir_all(&target_dir).expect("create target dir");
    fs::create_dir_all(launcher.parent().expect("launcher parent")).expect("create launcher dir");
    fs::write(&installed, "keep this command\n").expect("write existing command");
    fs::write(&launcher, "#!/usr/bin/env bash\n").expect("write fake launcher");

    let apply = repo.tiber_with_env(
        [
            "install-bin",
            "--target-dir",
            target_dir.to_str().expect("target dir utf8"),
            "--apply",
        ],
        [(
            "TIBER_LAUNCHER_PATH",
            launcher.to_str().expect("launcher path utf8"),
        )],
    );

    assert!(!apply.status.success(), "occupied target should be refused");
    assert!(
        String::from_utf8_lossy(&apply.stderr).contains("install_target_exists"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read_to_string(installed).expect("read existing command"),
        "keep this command\n"
    );
}
