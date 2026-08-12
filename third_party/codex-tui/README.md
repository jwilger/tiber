# Codex TUI presentation provenance

Tiber's terminal presentation is derived from the interaction, layout, and
test-backend conventions of OpenAI's `codex-tui` source at commit
`d06dc73290729d2bcb464b955a4cfd9992abc35d`.

The initial extracted slice adapts these upstream areas:

- `codex-rs/tui/src/ui_consts.rs`
- `codex-rs/tui/src/status_indicator_widget.rs`
- `codex-rs/tui/src/bottom_pane/chat_composer.rs`
- `codex-rs/tui/src/style.rs`
- `codex-rs/tui/tests/test_backend.rs`

Tiber substantially changed the implementation. It accepts typed projection
events, emits typed composer intents, and removes every dependency on Codex
runtime configuration, plugins, tools, sandboxing, workflow, and session
authority. The adapted implementation lives in
`crates/tiber-tui/src/lib.rs` and carries a prominent modification notice.

OpenAI Codex is licensed under Apache License 2.0. The required license text is
in `LICENSE`, and retained attribution is in `NOTICE`. Ratatui's MIT license is
retained at `third_party/ratatui/LICENSE`.
