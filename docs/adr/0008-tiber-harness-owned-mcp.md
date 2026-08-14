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
external-service validation claim. Hindsight S2 and the pure S3 audit-fact
boundary are available without changing that execution scope.

S3's provider-neutral DTOs bind policy outcomes to trusted authorization and
configured identities, retain reconciliation identity, and redact tool
arguments, transport/configuration, and server payloads. Observed payloads are
represented only by a byte count and domain-separated digest. Deterministic
local fake-server tests prove that policy denial produces no server I/O and
that observed, ambiguous, and reconciled outcomes stay sanitized. These facts
are not EventCore publications or durable receipts, and add no workflow,
scheduler, CLI, TUI, app-server, or runner wiring.

## Alternatives considered

Delegating to Codex MCP configuration was rejected because it violates isolated
authority.

## Revisit when

The initial MCP omissions block a measured workflow and can be added without
ceding authority.
