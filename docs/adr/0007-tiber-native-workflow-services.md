# ADR-0007: Use native task and workflow services

## Status

Accepted

## Date

2026-08-10

## Context

Calling Tiber's own CLI or MCP adapters from its core creates unnecessary
serialization, process, authorization, and recovery boundaries.

## Decision

Extract `tiber-tasks-core`, `tiber-tasks-service`, `tiber-workflow-core`, and
`tiber-workflow-service`. The first workflow extraction now provides a pure,
serializable trampoline and semantic identities in `tiber-workflow-core`, with
one closed `Infer` effect. `tiber-workflow-service` exposes only
command-specific EventCore decisions to initialize a workflow, request its
effect, record an observation, and advance it. Observation recording is its own
durable transaction: only a later advance decision may invoke the trampoline to
request, complete, or stop. The service provides neither a generic workflow
append nor an effect executor.

CLI, MCP, and TUI integration points are adapters. Internal task and workflow
actions call native typed services and never loop back through MCP or shell.
The CLI now interprets the closed inference effect through app-server, records
the observation and terminal advance durably, and restores the TUI projection
on relaunch. Declared app-server tool requests remain inert until a Tiber-owned
typed boundary handles them. This does not add a general scheduler or generic
effect executor.

## Consequences

All adapters share one domain contract and EventCore history. Extraction work
must preserve existing behavior and store compatibility. Broader scheduling
and operator-directed resolution of uncertain effects remain follow-on work
rather than reasons to reopen a generic mutation surface.

## Alternatives considered

Internal MCP and CLI loopback were rejected as slower, less typed, and harder to
reconcile.

## Revisit when

An external service becomes the intentional task or workflow authority.
