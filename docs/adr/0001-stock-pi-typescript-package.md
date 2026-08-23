# ADR 0001: Stock Pi TypeScript package

Status: Accepted

## Context

The Rust product embeds Codex, is not operational for the desired workflow, and
cannot extend an unmodified Pi installation as one package.

## Decision

Replace it without compatibility or data migration. Ship `@jwilger/tiber` as a
strict-TypeScript Pi package running in Pi's Node.js process. Bundle extensions,
skills, prompts, workflows, and themes. Require no Tiber launcher, daemon,
native binary, Pi fork, or MCP bridge. Use Pi packages as peers and Node
built-ins at runtime. License the result under `MIT OR Apache-2.0`.

## Consequences

The Rust/Codex workspace and obsolete history-facing documentation leave the
active tree. Stock-Pi contracts become release blockers. Stable installation is
through npm; Git remains the collaboration protocol.
