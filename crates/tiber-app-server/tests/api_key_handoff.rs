#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::std_instead_of_core,
    reason = "the bounded child-lifecycle fixture uses fail-fast setup and a disposable direct-argv executable"
)]
mod tests {
    use std::{
        env, fs,
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
        process,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use tiber_app_server::AppServerConfig;

    const ISOLATED_CONFIG: &str = include_str!("../../../config/app-server.toml");

    #[test]
    fn api_key_handoff_reaps_a_child_that_exceeds_its_configured_deadline() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "tiber-api-key-handoff-timeout-{}-{nonce}",
            process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory should be created");
        let executable = root.join("codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
if [ "$1" != "login" ] || [ "$2" != "--with-api-key" ]; then
  exit 17
fi
printf '%s' "$$" > "${0%/*}/pid"
while :; do :; done
"#,
        )
        .expect("fixture executable should be written");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("fixture executable should be executable");
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace should canonicalize");
        let config = AppServerConfig::new(
            executable,
            Vec::new(),
            root.join("codex-home"),
            workspace,
            Duration::from_millis(50),
        )
        .expect("fixture configuration should be valid");

        let error = config
            .login_with_api_key_from_stdin(ISOLATED_CONFIG)
            .expect_err("a bounded API-key handoff must time out");
        assert_eq!(error.code(), "app_server_api_key_login_timed_out");
        let process_id = fs::read_to_string(root.join("pid"))
            .expect("fixture child should record its process identity");
        let process_path = PathBuf::from(format!("/proc/{}", process_id.trim()));
        assert!(
            !process_path.exists(),
            "timed-out API-key login child should have been reaped"
        );

        fs::remove_dir_all(root).expect("fixture directory should be removed");
    }
}
