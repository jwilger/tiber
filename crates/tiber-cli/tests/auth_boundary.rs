#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    reason = "black-box process fixtures use fail-fast setup and assertions around the public CLI boundary"
)]
mod tests {
    use std::{
        env, fs,
        io::Write as _,
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
        process::{self, Command, Output, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };

    const FIXTURE_STDIN: &[u8] = b"fixture-stdin-token\n";
    const FIXTURE_STDIN_SHA256: &str =
        "9c112ac2bd07b5f9abc3bc075784156eaafe281e1399527f305f43de0616e1f8";

    struct AppServerFixture {
        api_key_login_sentinel: PathBuf,
        app_server_start_sentinel: PathBuf,
        auth_state: PathBuf,
        executable_directory: PathBuf,
        fake_server: PathBuf,
        root: PathBuf,
    }

    impl AppServerFixture {
        fn auth(&self, operation: &str, mode: &str) -> Output {
            self.command(operation, mode)
                .output()
                .expect("Tiber authentication command should execute")
        }

        fn auth_with_stdin(&self, operation: &str, mode: &str, stdin: &[u8]) -> Output {
            let mut child = self
                .command(operation, mode)
                .stderr(Stdio::piped())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("Tiber authentication command should start");
            let mut child_stdin = child
                .stdin
                .take()
                .expect("Tiber authentication command should accept stdin");
            child_stdin
                .write_all(stdin)
                .expect("fixture stdin should reach Tiber unchanged");
            drop(child_stdin);
            child
                .wait_with_output()
                .expect("Tiber authentication command should finish")
        }

        fn codex_home(&self) -> PathBuf {
            self.root.join("state/tiber/codex")
        }

        fn command(&self, operation: &str, mode: &str) -> Command {
            let original_path = env::var_os("PATH").expect("test PATH should be configured");
            let mut path_entries = vec![self.executable_directory.clone()];
            path_entries.extend(env::split_paths(&original_path));
            let path = env::join_paths(path_entries).expect("fixture PATH should be valid");
            let mut command = Command::new(env!("CARGO_BIN_EXE_tiber"));
            command
                .args(["auth", operation])
                .env("ANTHROPIC_API_KEY", "parent-key-must-not-reach-codex")
                .env("OPENAI_API_KEY", "parent-key-must-not-reach-codex")
                .env("PATH", path)
                .env("TIBER_API_KEY_LOGIN_SENTINEL", &self.api_key_login_sentinel)
                .env("TIBER_APP_SERVER_FIXTURE", &self.fake_server)
                .env(
                    "TIBER_APP_SERVER_START_SENTINEL",
                    &self.app_server_start_sentinel,
                )
                .env("TIBER_FIXTURE_AUTH_STATE", &self.auth_state)
                .env("TIBER_FIXTURE_EXPECTED_STDIN_SHA256", FIXTURE_STDIN_SHA256)
                .env("TIBER_FIXTURE_MODE", mode)
                .env("XDG_STATE_HOME", self.root.join("state"));
            command
        }

        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock should follow Unix epoch")
                .as_nanos();
            let root =
                env::temp_dir().join(format!("tiber-cli-auth-boundary-{}-{nonce}", process::id()));
            let executable_directory = root.join("bin");
            fs::create_dir_all(&executable_directory)
                .expect("fixture executable directory should be created");
            let api_key_login_sentinel = root.join("api-key-login-started");
            let app_server_start_sentinel = root.join("app-server-started");
            let auth_state = root.join("auth-state");
            let fake_server = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripts/tests/fake-app-server.mjs")
                .canonicalize()
                .expect("workspace fake app-server should exist");
            let codex = executable_directory.join("codex");
            fs::write(
                &codex,
                r#"#!/bin/sh
if [ "$1" = "login" ]; then
  printf login > "$TIBER_API_KEY_LOGIN_SENTINEL"
else
  printf app-server > "$TIBER_APP_SERVER_START_SENTINEL"
fi
exec node "$TIBER_APP_SERVER_FIXTURE" "$@"
"#,
            )
            .expect("fixture Codex wrapper should be written");
            fs::set_permissions(&codex, fs::Permissions::from_mode(0o755))
                .expect("fixture Codex wrapper should be executable");
            Self {
                api_key_login_sentinel,
                app_server_start_sentinel,
                auth_state,
                executable_directory,
                fake_server,
                root,
            }
        }

        fn read_auth_state(&self) -> String {
            fs::read_to_string(&self.auth_state)
                .expect("fixture Codex should record its credential-safe receipt")
        }

        fn remove(self) {
            fs::remove_dir_all(self.root).expect("fixture directory should be removed");
        }
    }

    #[test]
    fn api_key_login_hands_stdin_to_isolated_codex_and_verifies_app_server_state() {
        let fixture = AppServerFixture::new();
        let output = fixture.auth_with_stdin("login-api-key", "success", FIXTURE_STDIN);

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "authenticated: api-key\n"
        );
        assert!(String::from_utf8_lossy(&output.stderr).is_empty());
        assert!(fixture.api_key_login_sentinel.is_file());
        assert!(fixture.app_server_start_sentinel.is_file());
        assert!(fixture.codex_home().join("config.toml").is_file());
        let receipt = fixture.read_auth_state();
        assert!(receipt.contains("account_type=apiKey"));
        assert!(receipt.contains("account_read=true"));
        assert!(receipt.contains("anthropic_api_key_present=false"));
        assert!(receipt.contains("app_server_anthropic_api_key_present=false"));
        assert!(receipt.contains("app_server_openai_api_key_present=false"));
        assert!(receipt.contains("argv_contains_fixture_input=false"));
        assert!(receipt.contains("environment_contains_fixture_input=false"));
        assert!(receipt.contains("openai_api_key_present=false"));
        assert!(receipt.contains(&format!("codex_home={}", fixture.codex_home().display())));
        assert!(receipt.contains(&format!("stdin_sha256={FIXTURE_STDIN_SHA256}")));
        assert!(!receipt.contains("fixture-stdin-token"));

        fixture.remove();
    }

    #[test]
    fn api_key_login_failure_is_sanitized_and_does_not_start_app_server() {
        let fixture = AppServerFixture::new();
        let output =
            fixture.auth_with_stdin("login-api-key", "api-key-login-failure", FIXTURE_STDIN);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr,
            "app_server_api_key_login_failed: Codex API-key login did not complete\n"
        );
        assert!(!stderr.contains("fixture-stdin-token"));
        assert!(!stderr.contains("fixture-api-key-login-failure"));
        assert!(fixture.api_key_login_sentinel.is_file());
        assert!(!fixture.app_server_start_sentinel.exists());

        fixture.remove();
    }

    #[test]
    fn api_key_login_requires_app_server_to_confirm_api_key_state() {
        let fixture = AppServerFixture::new();
        let output = fixture.auth_with_stdin(
            "login-api-key",
            "api-key-login-without-account",
            FIXTURE_STDIN,
        );

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "app_server_api_key_login_unverified: app-server did not report API-key authentication after login\n"
        );
        assert!(fixture.api_key_login_sentinel.is_file());
        assert!(fixture.app_server_start_sentinel.is_file());

        fixture.remove();
    }

    #[test]
    fn browser_login_remains_app_server_managed() {
        let fixture = AppServerFixture::new();
        let output = fixture.auth("login", "success");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "open https://example.invalid/login\nwaiting for login id: login-fixture\n"
        );
        assert!(fixture.app_server_start_sentinel.is_file());
        assert!(!fixture.api_key_login_sentinel.exists());
        assert!(fixture.codex_home().join("config.toml").is_file());

        fixture.remove();
    }
}
