# ADR 0009: Context and memory services

Status: Accepted

## Context

Large tool results, documentation lookup, and durable memory are useful but can
exhaust context, leak secrets, or introduce executable bridges.

## Decision

Virtualize oversized Tiber-controlled results into local content-addressed
artifacts with bounded previews and search/range access. Provide direct typed
HTTP adapters for Context7 and optional Hindsight, never MCP bridges. Separate
private and opt-in shared memory banks. Shared retention accepts reviewed
completion artifacts only; raw output, code, diffs, and detected secrets are
excluded by default.

## Consequences

Context selection is typed and provenance-bearing. Network access remains a
capability constrained by containment and settings. Service failure cannot
fabricate docs or memory and optional integrations do not weaken workflow.
