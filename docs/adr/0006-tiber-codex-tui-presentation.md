# ADR-0006: Fork Codex TUI presentation without runtime authority

## Status

Accepted

## Date

2026-08-10

## Context

Users benefit from Codex's terminal interaction, but Codex runtime state and
tools would bypass Tiber authority.

## Decision

Adapt the presentation layer from `codex-tui` commit
`d06dc73290729d2bcb464b955a4cfd9992abc35d`. Preserve licenses and notices;
remove runtime configuration, plugin, tool, sandbox, workflow, and session
dependencies. The TUI consumes projections and emits intents only. Extract
vertical presentation slices instead of importing the complete Codex runtime
dependency graph; record each adapted upstream area and modification in
`third_party/codex-tui/README.md`.

## Consequences

Familiar interaction is retained while authority stays in Tiber. Upstream UI
changes require deliberate review and attribution.

## Alternatives considered

`codex --remote` was rejected because it bypasses Tiber. A new TUI was
rejected because it discards mature interaction behavior.

## Revisit when

An upstream presentation crate exposes a stable authority-neutral API.
