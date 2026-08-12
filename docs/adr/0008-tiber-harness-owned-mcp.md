# ADR-0008: Make Tiber the third-party MCP client

## Status

Accepted

## Date

2026-08-10

## Context

Host-managed MCP discovery or invocation would bypass Tiber's assignment,
policy, approval, cancellation, audit, and reconciliation contracts.

## Decision

Use a pinned official Rust RMCP client owned by Tiber. Initially support
absolute direct-argv stdio and localhost Streamable HTTP, capability
negotiation, tools, changes, progress, logs, cancellation, roots, and optional
resources/prompts. Exclude sampling, elicitation, and MCP tasks.

## Consequences

Tiber can intersect all authority policies and treat MCP data as untrusted.
Protocol compatibility and ambiguous mutations become Tiber responsibilities.

## Alternatives considered

Delegating to Codex MCP configuration was rejected because it violates isolated
authority.

## Revisit when

The initial MCP omissions block a measured workflow and can be added without
ceding authority.
