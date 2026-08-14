# ADR-0008: Make Tiber the third-party MCP client

## Status

Accepted

## Date

2026-08-10

## Context

Host-managed MCP discovery or invocation would bypass Tiber's assignment,
policy, approval, cancellation, audit, and reconciliation contracts.

## Decision

Use a pinned official Rust RMCP client owned by Tiber. The real but still
unconnected S1 boundary consists of `tiber-external-tools-core` and
`tiber-rmcp-client`, pinned to RMCP 3.1.2. It supports bounded absolute
direct-argv stdio and loopback Streamable HTTP, capability negotiation,
configured tool list/call, Tiber-owned roots, optional resource list/read,
optional prompt list/get, bounded changes/progress/logs, and cancellation.

The core intersects the global, workflow-mode, agent-role, session, assignment,
and effect-policy grants for the configured `IntegrationId` before minting an
opaque authorization. Roots are available only through a dedicated root
authorization; server metadata and resource/prompt data are bounded and
untrusted. Mutations require stable idempotency and an unknown outcome enters
reconciliation rather than replay.

The adapter uses no proxy, redirect, automatic replay or reinitialization, or
SSE resume. It refuses sampling, elicitation, MCP tasks, resource templates,
subscriptions, cache directives, and interactive continuations.

For bounded safety, the adapter rejects a negotiated protocol version at or
above `ProtocolVersion::STANDARD_HEADERS` (`2026-07-28`) before an operational
request. In RMCP 3.1.2, standard-header mode retains tool schemas without a
Tiber bound for later request-header construction. This is a deliberate
compatibility ceiling pending an upstream-safe bounded path, not a retry or a
generic protocol change.

## Consequences

Tiber can intersect all authority policies and treat MCP data as untrusted.
Protocol compatibility and ambiguous mutations become Tiber responsibilities.
The S1 boundary adds no workflow `TiberEffect`, EventCore, CLI, TUI,
app-server, scheduler, or runner integration and makes no live
external-service validation claim. Hindsight S2 and audit/integration S3 remain
pending.

## Alternatives considered

Delegating to Codex MCP configuration was rejected because it violates isolated
authority.

## Revisit when

The initial MCP omissions block a measured workflow and can be added without
ceding authority.
