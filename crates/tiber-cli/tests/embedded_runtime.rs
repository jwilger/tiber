#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::std_instead_of_core,
    reason = "the packaged-binary PTY regression fails fast in an isolated owner terminal"
)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        io::Write as _,
        os::unix::{fs::PermissionsExt as _, process::CommandExt as _},
        path::PathBuf,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use rustix::process::{Pid, Signal, kill_process_group};
    use tempfile::TempDir;

    struct ProcessGroupChild(Option<Child>);

    impl ProcessGroupChild {
        fn terminate(&mut self) {
            let Some(mut child) = self.0.take() else {
                return;
            };
            let _kill_result = kill_process_group(Pid::from_child(&child), Signal::KILL);
            let _wait_result = child.wait();
        }
    }

    impl Drop for ProcessGroupChild {
        fn drop(&mut self) {
            self.terminate();
        }
    }

    struct Fixture {
        _directory: TempDir,
        bin: PathBuf,
        codex_home: PathBuf,
        hostile_invocation: PathBuf,
        repository: PathBuf,
        state_home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("fixture directory should initialize");
            let repository = directory.path().join("repository");
            let bin = directory.path().join("bin");
            let state_home = directory.path().join("state");
            let codex_home = directory.path().join("codex-home");
            fs::create_dir_all(&repository).expect("fixture repository should initialize");
            fs::create_dir_all(&bin).expect("fixture bin should initialize");
            fs::create_dir_all(&state_home).expect("fixture state should initialize");
            fs::create_dir_all(&codex_home).expect("fixture Codex home should initialize");
            let hostile_invocation = repository.join("hostile-codex-invoked");
            let hostile_codex = bin.join("codex");
            fs::write(
                &hostile_codex,
                format!(
                    "#!/bin/sh\nprintf invoked > '{}'\nexit 73\n",
                    hostile_invocation.display()
                ),
            )
            .expect("hostile Codex fixture should be written");
            fs::set_permissions(&hostile_codex, fs::Permissions::from_mode(0o755))
                .expect("hostile Codex fixture should be executable");
            Self {
                _directory: directory,
                bin,
                codex_home,
                hostile_invocation,
                repository,
                state_home,
            }
        }

        fn path(&self) -> OsString {
            let mut entries = vec![self.bin.clone()];
            entries.extend(env::split_paths(
                &env::var_os("PATH").expect("test PATH should be configured"),
            ));
            env::join_paths(entries).expect("fixture PATH should be valid")
        }
    }

    #[test]
    fn bare_tiber_launches_embedded_codex_without_invoking_path_codex() {
        let fixture = Fixture::new();
        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .current_dir(&fixture.repository)
            .env("CODEX_HOME", &fixture.codex_home)
            .env("PATH", fixture.path())
            .env("XDG_STATE_HOME", &fixture.state_home)
            .output()
            .expect("bare Tiber should enter the embedded Codex TUI");
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(!fixture.hostile_invocation.exists(), "{stderr}");
        assert!(!stderr.contains("app_server_version_incompatible"));
        assert!(
            output.status.success() || stderr.starts_with("codex_tui_start_failed:"),
            "bare Tiber did not reach the embedded TUI boundary: {stderr}"
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the PTY regression reports the rendered startup boundary on early exit"
    )]
    fn embedded_codex_stays_running_past_arg0_runtime_initialization() {
        let fixture = Fixture::new();
        let capture = fixture.repository.join("embedded-codex-terminal.txt");
        let mut command = Command::new("script");
        command
            .process_group(0)
            .args([
                "--quiet",
                "--flush",
                "--return",
                "--command",
                &format!("stty rows 24 cols 80; {}", env!("CARGO_BIN_EXE_tiber")),
            ])
            .arg(&capture)
            .current_dir(&fixture.repository)
            .env("CODEX_HOME", &fixture.codex_home)
            .env("PATH", fixture.path())
            .env("XDG_STATE_HOME", &fixture.state_home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = ProcessGroupChild(Some(
            command.spawn().expect("embedded Codex PTY should start"),
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let rendered = fs::read_to_string(&capture).unwrap_or_default();
            if rendered.contains("OpenAI Codex")
                || (rendered.contains("Welcome") && rendered.contains("Codex"))
            {
                break;
            }
            if child
                .0
                .as_mut()
                .expect("child should remain live")
                .try_wait()
                .expect("embedded Codex status should remain readable")
                .is_some()
            {
                panic!("embedded Codex exited during startup: {rendered}");
            }
            assert!(Instant::now() < deadline, "startup timed out: {rendered}");
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(500));
        let rendered = fs::read_to_string(&capture).unwrap_or_default();
        let early_status = child
            .0
            .as_mut()
            .expect("child should remain live")
            .try_wait()
            .expect("embedded Codex status should remain readable");
        child.terminate();

        assert!(early_status.is_none(), "embedded Codex exited: {rendered}");
        assert!(!rendered.contains("Codex executable path is not configured"));
        assert!(!fixture.hostile_invocation.exists());
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the PTY regression reports the rendered terminal boundary on early exit"
    )]
    fn embedded_codex_gracefully_exits_and_restores_the_terminal() {
        let fixture = Fixture::new();
        let capture = fixture.repository.join("embedded-codex-graceful-exit.txt");
        let terminal_before = fixture.repository.join("terminal-before.txt");
        let terminal_after = fixture.repository.join("terminal-after.txt");
        let mut command = Command::new("script");
        command
            .process_group(0)
            .args([
                "--quiet",
                "--flush",
                "--return",
                "--command",
                &format!(
                    "stty rows 24 cols 80; stty -g > \"$TIBER_TERMINAL_BEFORE\"; {}; result=$?; stty -g > \"$TIBER_TERMINAL_AFTER\"; exit $result",
                    env!("CARGO_BIN_EXE_tiber")
                ),
            ])
            .arg(&capture)
            .current_dir(&fixture.repository)
            .env("CODEX_HOME", &fixture.codex_home)
            .env("PATH", fixture.path())
            .env("TIBER_TERMINAL_AFTER", &terminal_after)
            .env("TIBER_TERMINAL_BEFORE", &terminal_before)
            .env("XDG_STATE_HOME", &fixture.state_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = ProcessGroupChild(Some(
            command.spawn().expect("embedded Codex PTY should start"),
        ));
        let startup_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let rendered = fs::read_to_string(&capture).unwrap_or_default();
            if rendered.contains("OpenAI Codex")
                || (rendered.contains("Welcome") && rendered.contains("Codex"))
            {
                break;
            }
            if child
                .0
                .as_mut()
                .expect("child should remain live")
                .try_wait()
                .expect("embedded Codex status should remain readable")
                .is_some()
            {
                panic!("embedded Codex exited during startup: {rendered}");
            }
            assert!(
                Instant::now() < startup_deadline,
                "startup timed out: {rendered}"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let child_process = child.0.as_mut().expect("child should remain live");
        let input = child_process
            .stdin
            .as_mut()
            .expect("PTY controller should retain an input pipe");
        input.write_all(b"\x03").expect("first Ctrl-C should send");
        input.flush().expect("first Ctrl-C should flush");
        thread::sleep(Duration::from_millis(150));
        input.write_all(b"\x03").expect("second Ctrl-C should send");
        input.flush().expect("second Ctrl-C should flush");

        let exit_deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child_process
                .try_wait()
                .expect("embedded Codex status should remain readable")
            {
                break status;
            }
            let rendered = fs::read_to_string(&capture).unwrap_or_default();
            assert!(
                Instant::now() < exit_deadline,
                "graceful exit timed out: {rendered}"
            );
            thread::sleep(Duration::from_millis(10));
        };
        let rendered = fs::read_to_string(&capture).unwrap_or_default();
        child.0.take();

        assert!(
            status.success(),
            "embedded Codex did not exit successfully: {rendered}"
        );
        let before = fs::read_to_string(&terminal_before)
            .expect("pre-Codex terminal mode should be captured");
        let after = fs::read_to_string(&terminal_after)
            .expect("post-Codex terminal mode should be captured");
        assert_eq!(
            after.trim(),
            before.trim(),
            "Codex did not restore the exact PTY mode: {rendered}"
        );
        assert!(!fixture.hostile_invocation.exists());
    }
}
