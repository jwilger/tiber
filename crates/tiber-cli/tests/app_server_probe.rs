#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, process::Command};

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the fixture must exist in this black-box test, and the Result adapters are clearest as expression closures"
    )]
    fn authority_probe_accepts_the_reviewed_codex_0_147_control_surface() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tiber-app-server/tests/fixtures/codex-0.147.0-authority-surface.json")
            .canonicalize()
            .expect("authority-surface fixture should exist in the workspace");
        let output = Command::new(env!("CARGO_BIN_EXE_tiber"))
            .arg("app-server-probe")
            .arg(fixture)
            .output();

        assert_eq!(
            output
                .map(|result| {
                    (
                        result.status.success(),
                        String::from_utf8_lossy(&result.stdout).into_owned(),
                    )
                })
                .map_err(|error| error.to_string()),
            Ok((
                true,
                "app-server protocol exposes the reviewed Tiber control surface; runtime policy must cover: thread-item:commandExecution:runtime-policy-controlled, thread-item:fileChange:runtime-policy-controlled\n".to_owned()
            ))
        );
    }
}
