#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::map_unwrap_or,
    clippy::shadow_reuse,
    clippy::std_instead_of_core,
    reason = "black-box fixture setup and assertions use fail-fast test ergonomics without entering shipping library code"
)]
mod tests {

    use std::{
        path::PathBuf,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use ratatui::{Terminal, backend::TestBackend};
    use tiber_app_server::{AccountStatus, AppServerClient, AppServerConfig, TurnEvent};
    use tiber_tui::{ConversationProjection, ProjectionEvent};

    const ISOLATED_CONFIG: &str = include_str!("../../../config/app-server.toml");

    fn fixture_config(mode: Option<&str>) -> AppServerConfig {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("test repository should canonicalize");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        let codex_home = std::env::temp_dir().join(format!("tiber-app-server-test-{nonce}"));
        let node = std::env::var_os("TIBER_TEST_NODE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/env"));
        let mut arguments = if node.ends_with("env") {
            vec!["node".to_owned()]
        } else {
            Vec::new()
        };
        arguments.push(
            repository
                .join("scripts/tests/fake-app-server.mjs")
                .to_string_lossy()
                .into_owned(),
        );
        if let Some(mode) = mode {
            arguments.push(format!("--mode={mode}"));
        }
        AppServerConfig::new(
            node,
            arguments,
            codex_home,
            repository,
            Duration::from_secs(2),
        )
        .expect("fixture configuration should satisfy semantic invariants")
    }

    #[test]
    fn isolated_adapter_delegates_auth_streams_text_and_keeps_tools_inert() {
        let mut client = AppServerClient::start(fixture_config(None), ISOLATED_CONFIG)
            .expect("fixture app-server should initialize");

        assert_eq!(client.account_status(), Ok(AccountStatus::SignedOut));
        let handoff = client
            .start_chatgpt_login()
            .expect("fixture should provide a browser login handoff");
        assert_eq!(handoff.login_id, "login-fixture");
        assert_eq!(handoff.auth_url, "https://example.invalid/login");
        client
            .await_chatgpt_login(&handoff.login_id)
            .expect("fixture browser login should complete");
        assert_eq!(
            client.account_status(),
            Ok(AccountStatus::ChatGpt { email: None })
        );

        let result = client
            .converse("request the declared Tiber tool")
            .expect("fixture conversation should complete");
        assert_eq!(result.text, "hello from Tiber");
        assert_eq!(result.inert_tool_requests.len(), 1);
        assert_eq!(result.inert_tool_requests[0].tool, "tiber_authority_probe");

        client.logout().expect("fixture logout should succeed");
        assert_eq!(client.account_status(), Ok(AccountStatus::SignedOut));
    }

    #[test]
    fn turn_events_are_observable_before_completion_and_tools_stay_inert() {
        let mut client =
            AppServerClient::start(fixture_config(Some("split-stream")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let turn = client
            .start_turn("show streaming presentation")
            .expect("fixture turn should start");

        assert_eq!(
            client.next_turn_event(&turn),
            Ok(TurnEvent::AssistantDelta("hello ".to_owned()))
        );
        assert_eq!(
            client.next_turn_event(&turn),
            Ok(TurnEvent::AssistantDelta("from Tiber".to_owned()))
        );
        let tool = client
            .next_turn_event(&turn)
            .expect("tool request should be returned as data");
        assert!(matches!(tool, TurnEvent::InertToolRequested(_)));
        assert_eq!(client.next_turn_event(&turn), Ok(TurnEvent::Completed));
    }

    #[test]
    fn outbound_turn_declares_every_closed_tiber_tool_the_cli_recognizes() {
        let mut client = AppServerClient::start(
            fixture_config(Some("repository-tool-contract")),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");

        client
            .start_turn("inspect the outbound closed tool contract")
            .expect("thread/start must advertise the repository proposal tool");
    }

    #[test]
    fn turn_polling_is_nonterminal_until_an_observation_arrives() {
        let mut client =
            AppServerClient::start(fixture_config(Some("delayed-stream")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let turn = client
            .start_turn("wait for a delayed observation")
            .expect("fixture turn should start");

        assert_eq!(
            client.poll_turn_event(&turn, Duration::from_millis(10)),
            Ok(None)
        );
        assert_eq!(
            client.poll_turn_event(&turn, Duration::from_millis(250)),
            Ok(Some(TurnEvent::AssistantDelta(
                "hello from Tiber".to_owned()
            )))
        );
    }

    #[test]
    fn oversized_incoming_line_fails_at_the_transport_bound() {
        let mut client =
            AppServerClient::start(fixture_config(Some("oversized-line")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let error = client
            .start_turn("reject oversized output")
            .expect_err("oversized unterminated output must fail closed");
        assert_eq!(error.code(), "app_server_message_too_large");
        let follow_up = client
            .start_turn("do not repeat a poisoned record")
            .expect_err("fatal framing failure must close the reader");
        assert_eq!(follow_up.code(), "app_server_stream_closed");
    }

    #[test]
    fn owner_cancellation_interrupts_a_stalled_turn_start() {
        let mut client =
            AppServerClient::start(fixture_config(Some("delayed-start")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let cancellation = client.cancellation_handle();
        let operation = thread::spawn(move || client.start_turn("cancel startup"));
        thread::sleep(Duration::from_millis(75));
        cancellation.cancel();

        let error = operation
            .join()
            .expect("fixture operation should not panic")
            .expect_err("cancelled startup must fail promptly");
        assert_eq!(error.code(), "app_server_cancelled");
    }

    #[test]
    fn isolated_fake_server_stream_renders_through_the_tui_projection() {
        let mut client =
            AppServerClient::start(fixture_config(Some("split-stream")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let mut projection = ConversationProjection::new();
        let prompt = "render the isolated stream";
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: prompt.to_owned(),
        });
        let turn = client
            .start_turn(prompt)
            .expect("fixture turn should start");
        loop {
            match client
                .next_turn_event(&turn)
                .expect("fixture observation should remain typed")
            {
                TurnEvent::AssistantDelta(text) => {
                    projection.apply(ProjectionEvent::AssistantDelta { text });
                }
                TurnEvent::InertToolRequested(request) => {
                    projection.apply(ProjectionEvent::InertToolRequested {
                        arguments: request.arguments,
                        call_id: request.call_id,
                        tool: request.tool,
                    });
                }
                TurnEvent::Completed => {
                    projection.apply(ProjectionEvent::TurnCompleted);
                    break;
                }
            }
        }

        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| tiber_tui::render(frame, &projection))
            .expect("completed projection should render");
        let frame = terminal.backend().to_string();
        assert!(frame.contains("hello from Tiber"));
        assert!(frame.contains("tool proposal \u{b7} not executed"));
        assert!(frame.contains("tiber_authority_probe \u{b7} call-fixture"));
        assert!(frame.contains("ready"));
    }

    #[test]
    fn silent_child_returns_a_retryable_typed_timeout() {
        let error = AppServerClient::start(fixture_config(Some("silent")), ISOLATED_CONFIG)
            .err()
            .expect("silent fixture should time out");
        assert_eq!(error.code(), "app_server_timeout");
        assert!(error.is_retryable());
    }

    #[test]
    fn dropping_the_adapter_reaps_the_app_server_child() {
        let client = AppServerClient::start(fixture_config(None), ISOLATED_CONFIG)
            .expect("fixture app-server should initialize");
        let process_id = client.child_process_id();
        assert!(PathBuf::from(format!("/proc/{process_id}")).exists());

        drop(client);

        assert!(!PathBuf::from(format!("/proc/{process_id}")).exists());
    }
    #[test]
    fn chatty_child_cannot_extend_the_operation_deadline() {
        let error = AppServerClient::start(fixture_config(Some("chatty")), ISOLATED_CONFIG)
            .err()
            .expect("chatty fixture should hit the whole-operation deadline");
        assert_eq!(error.code(), "app_server_timeout");
    }

    #[test]
    fn incompatible_runtime_version_fails_closed() {
        let error = AppServerClient::start(fixture_config(Some("wrong-version")), ISOLATED_CONFIG)
            .err()
            .expect("unreviewed Codex version should be rejected");
        assert_eq!(error.code(), "app_server_version_incompatible");
    }

    #[test]
    fn effective_profile_mismatches_fail_closed_before_starting_a_turn() {
        for mode in [
            "wrong-profile",
            "wrong-approval-policy",
            "wrong-sandbox-type",
            "network-enabled",
        ] {
            let mut client = AppServerClient::start(fixture_config(Some(mode)), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
            let error = client
                .start_turn("reject an expanded effective profile")
                .expect_err("effective profile mismatch must stop before turn/start");
            assert_eq!(
                error.code(),
                "app_server_effective_profile_mismatch",
                "fixture mode {mode} should fail with the effective-profile fence"
            );
            assert!(!error.is_retryable());
        }
    }

    #[test]
    fn idless_login_failure_is_reported_without_waiting_for_timeout() {
        let mut client = AppServerClient::start(
            fixture_config(Some("idless-login-failure")),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");
        let handoff = client
            .start_chatgpt_login()
            .expect("fixture should start browser login");
        let error = client
            .await_chatgpt_login(&handoff.login_id)
            .expect_err("idless failure should terminate login");
        assert_eq!(error.code(), "app_server_authentication_failed");
    }

    #[test]
    fn colliding_server_request_is_rejected_before_the_client_response() {
        let mut client =
            AppServerClient::start(fixture_config(Some("id-collision")), ISOLATED_CONFIG)
                .expect("fixture app-server should initialize");
        let result = client
            .converse("exercise the colliding server request")
            .expect("adapter should reject the server request and continue");
        assert_eq!(result.text, "hello from Tiber");
    }

    #[test]
    fn oversized_prompt_is_rejected_before_transport_write() {
        let mut client = AppServerClient::start(fixture_config(None), ISOLATED_CONFIG)
            .expect("fixture app-server should initialize");
        let prompt = "x".repeat(20_000);
        let error = client
            .converse(&prompt)
            .expect_err("oversized prompt must not reach the child pipe");
        assert_eq!(error.code(), "app_server_prompt_too_large");
    }
}
