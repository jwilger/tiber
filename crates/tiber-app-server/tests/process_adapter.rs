#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::default_numeric_fallback,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::indexing_slicing,
    clippy::map_unwrap_or,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::std_instead_of_core,
    clippy::too_many_lines,
    reason = "black-box fixture setup and assertions use fail-fast test ergonomics without entering shipping library code"
)]
mod tests {

    use std::{
        fs,
        path::PathBuf,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use ratatui::{Terminal, backend::TestBackend};
    use tiber_app_server::{
        AccountStatus, AppServerClient, AppServerConfig, TiberEffectRequestId, TiberEffectResult,
        TurnEvent,
    };
    use tiber_process_core::{ConfiguredCommandId, MAX_CONFIGURED_COMMANDS};
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
        if mode == Some("configured-command-tool-contract") {
            arguments.push("--configuration-secret=must-not-cross-tool-schema".to_owned());
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

    fn pending_effect_fixture_config(mode: Option<&str>) -> AppServerConfig {
        pending_effect_fixture_config_with_paths(mode, &[])
    }

    fn pending_effect_fixture_config_with_paths(
        mode: Option<&str>,
        paths: &[&std::path::Path],
    ) -> AppServerConfig {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("test repository should canonicalize");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        let codex_home =
            std::env::temp_dir().join(format!("tiber-app-server-pending-effect-test-{nonce}"));
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
                .join("crates/tiber-app-server/tests/fixtures/pending-tiber-effect.mjs")
                .to_string_lossy()
                .into_owned(),
        );
        if let Some(mode) = mode {
            arguments.push(mode.to_owned());
        }
        arguments.extend(paths.iter().map(|path| path.to_string_lossy().into_owned()));
        AppServerConfig::new(
            node,
            arguments,
            codex_home,
            repository,
            Duration::from_secs(2),
        )
        .expect("pending-effect fixture configuration should be valid")
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
    fn declared_tiber_effect_waits_for_one_exact_caller_completion() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        let early_response_marker =
            std::env::temp_dir().join(format!("tiber-effect-early-response-{nonce}"));
        let completion_authorization =
            std::env::temp_dir().join(format!("tiber-effect-completion-authorized-{nonce}"));
        let mut client = AppServerClient::start(
            pending_effect_fixture_config_with_paths(
                Some("sequenced"),
                &[&early_response_marker, &completion_authorization],
            ),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");
        let turn = client
            .start_turn("request the declared Tiber effect")
            .expect("fixture turn should start");

        let TurnEvent::TiberEffectRequested(request) = client
            .next_turn_event(&turn)
            .expect("declared Tiber effect should remain pending")
        else {
            panic!("exact declared tool must surface as a pending Tiber effect");
        };
        assert_eq!(
            request.request_id(),
            &TiberEffectRequestId::String("effect-request-1".to_owned())
        );
        assert_eq!(request.call_id(), "effect-call-1");
        assert_eq!(request.thread_id(), "thread-effect");
        assert_eq!(request.turn_id(), "turn-1");
        assert_eq!(request.tool(), "tiber_effect");
        assert_eq!(
            request.arguments(),
            &serde_json::json!({ "operation": "record_receipt", "sequence": 1 })
        );

        thread::sleep(Duration::from_millis(50));
        assert!(
            !early_response_marker.exists(),
            "app-server must receive no response before caller authorization"
        );
        let pending_poll = client
            .poll_turn_event(&turn, Duration::from_millis(10))
            .expect_err("pending effect must block further protocol polling");
        assert_eq!(pending_poll.code(), "app_server_effect_completion_required");
        let second_turn = client
            .start_turn("must not start while the effect is pending")
            .expect_err("pending effect must block a second turn");
        assert_eq!(second_turn.code(), "app_server_effect_completion_required");
        let unrelated_request = client
            .account_status()
            .expect_err("pending effect must block unrelated protocol requests");
        assert_eq!(
            unrelated_request.code(),
            "app_server_effect_completion_required"
        );

        let mut other =
            AppServerClient::start(pending_effect_fixture_config(None), ISOLATED_CONFIG)
                .expect("second fixture app-server should initialize");
        let other_turn = other
            .start_turn("request a different client's effect")
            .expect("second fixture turn should start");
        let TurnEvent::TiberEffectRequested(other_request) = other
            .next_turn_event(&other_turn)
            .expect("second client should expose its pending effect")
        else {
            panic!("second client must expose a pending Tiber effect");
        };
        let wrong_client = client
            .complete_tiber_effect(
                &turn,
                &other_request,
                other_request.call_id(),
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect_err("another client's request must be rejected");
        assert_eq!(wrong_client.code(), "app_server_effect_client_mismatch");

        let wrong_call = client
            .complete_tiber_effect(
                &turn,
                &request,
                "different-call",
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect_err("a different call identity must be rejected");
        assert_eq!(wrong_call.code(), "app_server_effect_call_mismatch");

        for invalid in [
            TiberEffectResult::Success {
                output: String::new(),
            },
            TiberEffectResult::Success {
                output: "x".repeat(20_000),
            },
            TiberEffectResult::Success {
                output: "hidden\u{1b}[31mcontrol".to_owned(),
            },
            TiberEffectResult::Failure {
                code: "NOT SEMANTIC".to_owned(),
                message: "failed".to_owned(),
                retryable: false,
            },
            TiberEffectResult::Failure {
                code: "effect_failed".to_owned(),
                message: String::new(),
                retryable: false,
            },
            TiberEffectResult::Failure {
                code: "effect_failed".to_owned(),
                message: "x".repeat(20_000),
                retryable: false,
            },
            TiberEffectResult::Failure {
                code: "effect_failed".to_owned(),
                message: "hidden\u{1b}[31mcontrol".to_owned(),
                retryable: false,
            },
        ] {
            let error = client
                .complete_tiber_effect(&turn, &request, request.call_id(), invalid)
                .expect_err("invalid effect result must be rejected before transport write");
            assert_eq!(error.code(), "app_server_effect_result_invalid");
        }

        fs::write(&completion_authorization, "authorized\n")
            .expect("test should authorize the exact completion");

        client
            .complete_tiber_effect(
                &turn,
                &request,
                request.call_id(),
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect("exact pending effect should complete once");
        let duplicate = client
            .complete_tiber_effect(
                &turn,
                &request,
                request.call_id(),
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect_err("duplicate completion must be rejected");
        assert_eq!(duplicate.code(), "app_server_effect_already_completed");
        assert_eq!(
            client.next_turn_event(&turn),
            Ok(TurnEvent::AssistantDelta("completion observed".to_owned()))
        );
        assert_eq!(client.next_turn_event(&turn), Ok(TurnEvent::Completed));

        let next_turn = client
            .start_turn("request a second declared Tiber effect")
            .expect("a new turn may start after exact completion");
        let TurnEvent::TiberEffectRequested(next_request) = client
            .next_turn_event(&next_turn)
            .expect("second turn should expose its own pending effect")
        else {
            panic!("second turn must expose a pending Tiber effect");
        };
        let wrong_turn = client
            .complete_tiber_effect(
                &turn,
                &next_request,
                next_request.call_id(),
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect_err("a request must not complete through an earlier turn handle");
        assert_eq!(wrong_turn.code(), "app_server_effect_turn_mismatch");
        client
            .complete_tiber_effect(
                &next_turn,
                &next_request,
                next_request.call_id(),
                TiberEffectResult::Success {
                    output: "effect completed".to_owned(),
                },
            )
            .expect("second turn's exact effect should complete");
        assert_eq!(
            client.next_turn_event(&next_turn),
            Ok(TurnEvent::AssistantDelta("completion observed".to_owned()))
        );
        assert_eq!(client.next_turn_event(&next_turn), Ok(TurnEvent::Completed));
        fs::remove_file(&completion_authorization)
            .expect("test completion authorization should be removable");
        assert!(!early_response_marker.exists());
    }

    #[test]
    fn malformed_declared_tiber_effect_requests_fail_at_the_public_boundary() {
        for (mode, expected_code) in [
            ("invalid-request-id", "app_server_effect_request_invalid"),
            ("control-call-id", "app_server_effect_request_invalid"),
            ("oversized-call-id", "app_server_effect_request_invalid"),
            ("missing-call-id", "app_server_effect_request_invalid"),
            ("non-string-turn-id", "app_server_effect_request_invalid"),
            ("missing-arguments", "app_server_effect_request_invalid"),
            ("oversized-arguments", "app_server_effect_request_too_large"),
        ] {
            let mut client =
                AppServerClient::start(pending_effect_fixture_config(Some(mode)), ISOLATED_CONFIG)
                    .expect("malformed-request fixture should initialize");
            let turn = client
                .start_turn("reject malformed declared effect correlation")
                .expect("fixture turn should start");

            let error = client
                .next_turn_event(&turn)
                .expect_err("malformed declared effect must fail before becoming pending");
            assert_eq!(error.code(), expected_code, "fixture mode {mode}");
        }
    }

    #[test]
    fn typed_failure_completion_preserves_bounded_retry_policy() {
        let mut client = AppServerClient::start(
            pending_effect_fixture_config(Some("failure")),
            ISOLATED_CONFIG,
        )
        .expect("failure-completion fixture should initialize");
        let turn = client
            .start_turn("complete the declared effect with a typed failure")
            .expect("fixture turn should start");
        let TurnEvent::TiberEffectRequested(request) = client
            .next_turn_event(&turn)
            .expect("declared Tiber effect should remain pending")
        else {
            panic!("failure fixture must expose a pending Tiber effect");
        };

        client
            .complete_tiber_effect(
                &turn,
                &request,
                request.call_id(),
                TiberEffectResult::Failure {
                    code: "effect_denied".to_owned(),
                    message: "policy denied".to_owned(),
                    retryable: true,
                },
            )
            .expect("bounded typed failure should complete the exact request");
        assert_eq!(
            client.next_turn_event(&turn),
            Ok(TurnEvent::AssistantDelta("completion observed".to_owned()))
        );
        assert_eq!(client.next_turn_event(&turn), Ok(TurnEvent::Completed));
    }

    #[test]
    fn foreign_exact_effect_is_rejected_without_disrupting_the_active_turn() {
        let mut client = AppServerClient::start(
            pending_effect_fixture_config(Some("foreign-exact")),
            ISOLATED_CONFIG,
        )
        .expect("foreign-effect fixture should initialize");
        let turn = client
            .start_turn("continue after rejecting a foreign exact effect")
            .expect("fixture turn should start");

        assert_eq!(
            client.next_turn_event(&turn),
            Ok(TurnEvent::AssistantDelta(
                "foreign exact effect rejected".to_owned()
            ))
        );
        assert_eq!(client.next_turn_event(&turn), Ok(TurnEvent::Completed));
    }

    #[test]
    fn pending_repository_proposal_rejects_a_correlated_process_effect_without_dispatch() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow Unix epoch")
            .as_nanos();
        let rejection = std::env::temp_dir().join(format!(
            "tiber-exclusive-effect-response-{}-{nonce}.json",
            std::process::id()
        ));
        let mut client = AppServerClient::start(
            pending_effect_fixture_config_with_paths(
                Some("repository-then-process"),
                &[rejection.as_path()],
            ),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");
        let turn = client
            .start_turn("keep owner decisions exclusive")
            .expect("fixture turn should start");
        let event = client
            .next_turn_event(&turn)
            .expect("repository proposal should reach the public boundary");
        let TurnEvent::RepositoryProposalRequested(proposal) = event else {
            panic!("expected repository proposal, received {event:?}");
        };
        assert!(
            client
                .poll_turn_event(&turn, Duration::from_millis(50))
                .expect("conflicting process request must be rejected in-band")
                .is_none(),
            "a process effect escaped while repository owner authority was pending"
        );

        client
            .complete_repository_proposal(
                &turn,
                &proposal,
                TiberEffectResult::Success {
                    output: r#"{"path":"README.md","status":"applied"}"#.to_owned(),
                },
            )
            .expect("the pending repository owner flow must remain usable");
        assert!(matches!(
            client
                .next_turn_event(&turn)
                .expect("turn should complete after the owner decision"),
            TurnEvent::Completed
        ));

        let response: serde_json::Value = serde_json::from_slice(
            &fs::read(&rejection).expect("conflicting process request must be answered"),
        )
        .expect("rejection must be valid JSON-RPC");
        assert_eq!(
            response.pointer("/id").and_then(serde_json::Value::as_str),
            Some("conflicting-effect-request")
        );
        assert_eq!(
            response
                .pointer("/result/success")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            response
                .pointer("/result/contentItems/0/text")
                .and_then(serde_json::Value::as_str),
            Some(
                "Tiber rejected a configured process while a repository owner decision is pending."
            )
        );
    }

    #[test]
    fn pending_process_effect_defers_repository_proposal_until_process_completion() {
        let mut client = AppServerClient::start(
            pending_effect_fixture_config(Some("process-then-repository")),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");
        let turn = client
            .start_turn("keep process authority exclusive")
            .expect("fixture turn should start");
        let event = client
            .next_turn_event(&turn)
            .expect("configured process should reach the public boundary");
        let TurnEvent::TiberEffectRequested(effect) = event else {
            panic!("expected configured process, received {event:?}");
        };
        let blocked_poll = client
            .poll_turn_event(&turn, Duration::from_millis(50))
            .expect_err("pending process authority must prevent concurrent proposal polling");
        assert_eq!(blocked_poll.code(), "app_server_effect_completion_required");

        client
            .complete_tiber_effect(
                &turn,
                &effect,
                effect.call_id(),
                TiberEffectResult::Success {
                    output: r#"{"command":"focused-test","status":"completed"}"#.to_owned(),
                },
            )
            .expect("the pending process flow must remain completable");
        let sequential = client
            .next_turn_event(&turn)
            .expect("proposal may be considered only after process completion");
        let TurnEvent::RepositoryProposalRequested(proposal) = sequential else {
            panic!("expected sequential repository proposal, received {sequential:?}");
        };
        client
            .complete_repository_proposal(
                &turn,
                &proposal,
                TiberEffectResult::Success {
                    output: r#"{"path":"README.md","status":"applied"}"#.to_owned(),
                },
            )
            .expect("sequential repository owner flow must remain usable");
        assert!(matches!(
            client
                .next_turn_event(&turn)
                .expect("turn should complete after both sequential effects"),
            TurnEvent::Completed
        ));
    }

    #[test]
    fn outbound_turn_declares_closed_bounded_tiber_tool_contracts() {
        let mut client = AppServerClient::start(
            fixture_config(Some("repository-tool-contract")),
            ISOLATED_CONFIG,
        )
        .expect("fixture app-server should initialize");

        client
            .start_turn("inspect the outbound closed tool contract")
            .expect("thread/start must advertise only each tool's accepted input shape");
    }

    #[test]
    fn outbound_turn_exposes_only_bounded_configured_command_identities() {
        let command_ids = ["format", "focused-test"].map(|identity| {
            ConfiguredCommandId::parse(identity).expect("fixture command identity should be valid")
        });
        let config = fixture_config(Some("configured-command-tool-contract"))
            .with_configured_command_ids(command_ids)
            .expect("bounded semantic command identities should be accepted");
        let mut client = AppServerClient::start(config, ISOLATED_CONFIG)
            .expect("fixture app-server should initialize");

        client
            .start_turn("inspect discoverable configured command identities")
            .expect("thread/start must expose only the sorted semantic command identities");
    }

    #[test]
    fn configured_command_identity_view_rejects_empty_duplicate_and_oversized_catalogs() {
        let command_id = |identity: &str| {
            ConfiguredCommandId::parse(identity).expect("fixture command identity should be valid")
        };
        for invalid in [Vec::new(), vec![command_id("same"), command_id("same")]] {
            let error = fixture_config(None)
                .with_configured_command_ids(invalid)
                .expect_err("an invalid identity view must be rejected before transport startup");
            assert_eq!(
                error.code(),
                "app_server_configured_command_catalog_invalid"
            );
        }

        let oversized = (0..=MAX_CONFIGURED_COMMANDS)
            .map(|index| command_id(&format!("command-{index}")))
            .collect::<Vec<_>>();
        let error = fixture_config(None)
            .with_configured_command_ids(oversized)
            .expect_err("an oversized identity view must be rejected before transport startup");
        assert_eq!(
            error.code(),
            "app_server_configured_command_catalog_invalid"
        );
    }

    #[test]
    fn repository_proposals_wait_for_the_exact_owner_decision() {
        let mut client =
            AppServerClient::start(fixture_config(Some("repository-edit")), ISOLATED_CONFIG)
                .expect("repository-proposal fixture should initialize");
        let turn = client
            .start_turn("keep repository mutation behind its separate owner boundary")
            .expect("fixture turn should start");

        let request = loop {
            match client
                .next_turn_event(&turn)
                .expect("repository proposal should remain observable and pending")
            {
                TurnEvent::AssistantDelta(_delta) => {}
                TurnEvent::RepositoryProposalRequested(request) => break request,
                TurnEvent::InertToolRequested(_request) => {
                    panic!("closed repository proposal must not be rejected as inert");
                }
                TurnEvent::TiberEffectRequested(_request) => {
                    panic!("repository proposal must not enter the Tiber effect path");
                }
                TurnEvent::Completed => {
                    panic!("repository proposal must be observed before turn completion");
                }
            }
        };
        assert_eq!(request.call_id(), "call-fixture");
        client
            .complete_repository_proposal(
                &turn,
                &request,
                TiberEffectResult::Success {
                    output: r#"{"path":"README.md","status":"applied"}"#.to_owned(),
                },
            )
            .expect("the exact pending proposal should complete once");
        assert_eq!(client.next_turn_event(&turn), Ok(TurnEvent::Completed));
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
                TurnEvent::TiberEffectRequested(_request) => {
                    panic!("split-stream fixture must keep its unknown tool inert");
                }
                TurnEvent::RepositoryProposalRequested(_request) => {
                    panic!("split-stream fixture must not request a repository proposal");
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
