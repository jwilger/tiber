#![forbid(unsafe_code)]

#[cfg(test)]
#[expect(
    clippy::implicit_return,
    clippy::non_ascii_literal,
    reason = "fixed terminal snapshots use fail-fast setup and retain the exact fork-derived glyphs under test"
)]
mod tests {

    use ratatui::{
        Terminal,
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    };
    use serde_json::json;
    use tiber_tui::{ComposerIntent, ConversationProjection, ProjectionEvent, render};

    fn rendered(projection: &ConversationProjection) -> String {
        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, projection))
            .expect("projection should render");
        terminal.backend().to_string()
    }

    #[test]
    fn transcript_stream_and_inert_tool_are_projection_only() {
        let mut projection = ConversationProjection::new();
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "Inspect the repository".to_owned(),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "I found ".to_owned(),
        });

        let streaming = rendered(&projection);
        assert!(streaming.contains("│you"));
        assert!(streaming.contains("│  Inspect the repository"));
        assert!(streaming.contains("│tiber · streaming"));
        assert!(streaming.contains("│  I found"));
        assert!(streaming.contains("streaming · ctrl+c quit · tools are inert"));
        assert_eq!(
            projection.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ComposerIntent::Quit
        );

        projection.apply(ProjectionEvent::AssistantDelta {
            text: "one candidate.".to_owned(),
        });
        projection.apply(ProjectionEvent::InertToolRequested {
            arguments: json!({ "path": "src/lib.rs" }),
            call_id: "call-1".to_owned(),
            tool: "tiber_effect".to_owned(),
        });
        projection.apply(ProjectionEvent::TurnCompleted);

        let completed = rendered(&projection);
        assert!(completed.contains("I found one candidate."));
        assert!(completed.contains("tool proposal · not executed"));
        assert!(completed.contains("tiber_effect · call-1"));
        assert!(completed.contains(r#"{"path":"src/lib.rs"}"#));
        assert!(completed.contains("ready"));
        assert!(!completed.contains("tool proposals are not executed"));
    }

    #[test]
    fn composer_emits_only_typed_submit_and_quit_intents() {
        let mut projection = ConversationProjection::new();
        for character in "hello".chars() {
            assert_eq!(
                projection.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
                ComposerIntent::None
            );
        }
        assert_eq!(
            projection.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ComposerIntent::Submit("hello".to_owned())
        );
        assert_eq!(
            projection.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ComposerIntent::Quit
        );
    }

    #[test]
    fn typed_failure_preserves_partial_transcript_and_reenables_composer() {
        let mut projection = ConversationProjection::new();
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "Start".to_owned(),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "partial".to_owned(),
        });
        projection.apply(ProjectionEvent::TurnFailed {
            code: "app_server_stream_closed".to_owned(),
            message: "app-server stream closed".to_owned(),
            retryable: true,
        });

        let failed = rendered(&projection);
        assert!(failed.contains("partial"));
        assert!(failed.contains("app_server_stream_closed · retryable"));
        assert!(failed.contains("ready"));
    }

    #[test]
    fn terminal_controls_are_escaped_and_long_transcripts_follow_the_tail() {
        let mut projection = ConversationProjection::new();
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "first marker".to_owned(),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "\u{1b}[2Junsafe\n".repeat(80),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "last marker".to_owned(),
        });

        let frame = rendered(&projection);
        assert!(!frame.contains('\u{1b}'));
        assert!(frame.contains("last marker"));
        assert!(!frame.contains("first marker"));
    }

    #[test]
    #[expect(
        clippy::separated_literal_suffix,
        reason = "the workspace enables conflicting separated and unseparated suffix restriction lints; explicit usize bounds avoid numeric fallback"
    )]
    fn transcript_retention_is_bounded_and_only_the_active_response_streams() {
        let mut projection = ConversationProjection::new();
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "historical prompt".to_owned(),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "historical response".to_owned(),
        });
        projection.apply(ProjectionEvent::TurnCompleted);
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "active prompt".to_owned(),
        });

        let two_turns = rendered(&projection);
        assert_eq!(two_turns.matches("tiber · streaming").count(), 1);
        assert!(two_turns.contains("historical response"));

        for turn in 0_usize..200_usize {
            projection.apply(ProjectionEvent::AssistantDelta {
                text: format!("response {turn}"),
            });
            projection.apply(ProjectionEvent::TurnCompleted);
            projection.apply(ProjectionEvent::PromptSubmitted {
                text: format!("prompt {turn}"),
            });
        }
        let bounded_tail = rendered(&projection);
        assert!(!bounded_tail.contains("historical prompt"));
        assert!(bounded_tail.contains("prompt 199"));
    }

    #[test]
    #[expect(
        clippy::separated_literal_suffix,
        reason = "the workspace enables conflicting separated and unseparated suffix restriction lints; explicit usize bounds avoid numeric fallback"
    )]
    fn active_assistant_survives_tool_proposal_retention_pressure() {
        let mut projection = ConversationProjection::new();
        projection.apply(ProjectionEvent::PromptSubmitted {
            text: "active prompt".to_owned(),
        });
        for call in 0_usize..300_usize {
            projection.apply(ProjectionEvent::InertToolRequested {
                arguments: json!({ "call": call }),
                call_id: format!("call-{call}"),
                tool: "tiber_effect".to_owned(),
            });
        }
        projection.apply(ProjectionEvent::AssistantDelta {
            text: "active response survived".to_owned(),
        });

        let backend = TestBackend::new(72, 2_000);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, &projection))
            .expect("projection should render");
        let frame = terminal.backend().to_string();
        assert!(frame.contains("active response survived"));
        assert_eq!(frame.matches("tiber · streaming").count(), 1);
    }
}
