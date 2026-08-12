# ADR-0011: Cut over Tiber commands and packages immediately

## Status

Accepted

## Date

2026-08-10

## Context

Tiber Tasks currently occupies an ambiguous `tiber` command surface while the
standalone harness needs that product name.

## Decision

Atomically make `tiber` the only executable, default it to the interactive
TUI, and place native tasks under `tiber tasks …`. Rename ambiguous crates to
`tiber-tasks-*`. Update native crates, CLI/TUI adapters, scripts, docs,
packaging, and CI together. Preserve EventCore history and the `tiber` branch,
but provide no aliases, compatibility crates, deprecated paths, or transition
window.

## Consequences

The interface is coherent immediately and migration cost is concentrated in one
verified increment. Existing command users must update atomically.

## Alternatives considered

A compatibility window and a second product name were rejected because both
prolong ambiguity and duplicate support.

## Revisit when

Only if implementation evidence proves the atomic migration cannot preserve
authoritative history.
