# ADR-0009: Use swappable memory with Hindsight first

## Status

Accepted

## Date

2026-08-10

## Context

Long-running development benefits from recall, but memory is advisory,
fallible, and not authoritative workflow state.

## Decision

Define a swappable `MemoryBackend` port in `tiber-memory-core` and make
`tiber-hindsight-http` its first adapter. Keep Hindsight HTTP API 0.8.3 DTOs
inside that adapter. Its bounded operation vocabulary is asynchronous retain,
operation status, cancellation, forget, recall, and named read-only
reconciliation.

Every operation carries strict owner and repository provenance. Banks are
owner-global or repository-scoped, and typed tags include repository, agent,
session, task, and memory kind. Backend document and operation handles are
stable and scope-bound. An ambiguous mutation carries a read-only
reconciliation handle rather than authorizing a replay. Retain requests name
their source turn; attach provenance and item/token budgets to recall, and
never admit that same turn. Recall is
advisory, untrusted context, never authority for a workflow or effect.

Tiber connects only to an explicit Hindsight endpoint. It does not install or
globally configure Hindsight, retry requests, manage Hindsight authentication,
or claim live-service validation. The S3 audit DTOs retain trusted provenance,
stable bounded outcomes, reconciliation identity, and retain evidence while
excluding raw retained text, recall queries, and recalled content. They are not
EventCore publications, durable receipts, or workflow, CLI, TUI, app-server,
or scheduler integration.

## Consequences

Memory is replaceable and failures are visible but normally nonfatal. EventCore
remains authoritative and Hindsight `reflect` is not primary v1 reasoning.
Deterministic local fake-server coverage checks scoped lifecycle and
hostile-input behavior without network access. An ignored live test is an
explicit operator check only: it requires exact `TIBER_RUN_LIVE_HINDSIGHT=1`
and a nonempty `TIBER_HINDSIGHT_ENDPOINT`, uses nonce-isolated synthetic data,
and attempts exact-document cleanup. Default CI remains network-free, and this
ADR does not claim that a live Hindsight service has been executed.

## Alternatives considered

Hard-coding Hindsight and treating recall as authority were rejected. No memory
was rejected because it loses useful continuity.

## Revisit when

A different backend provides a materially better measured contract.
