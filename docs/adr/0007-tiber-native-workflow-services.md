# ADR-0007: Use native task and workflow services

## Status

Accepted

## Date

2026-08-10

## Context

Calling Tiber's own CLI or MCP adapters from its core creates unnecessary
serialization, process, authorization, and recovery boundaries.

## Decision

Extract `tiber-tasks-core`, `tiber-tasks-service`,
`development-workflow-core`, and `development-workflow-service`. CLI, MCP,
and TUI integration points are adapters. Internal task and workflow actions
call native typed services and never loop back through MCP or shell.

## Consequences

All adapters share one domain contract and EventCore history. Extraction work
must preserve existing behavior and store compatibility.

## Alternatives considered

Internal MCP and CLI loopback were rejected as slower, less typed, and harder to
reconcile.

## Revisit when

An external service becomes the intentional task or workflow authority.
