# ADR-0004: Make Tiber the standalone authority

## Status

Accepted

## Date

2026-08-10

## Context

The former marketplace's plugin instructions and hooks could advise a host
agent but could not reliably own identity, isolation, durable workflow state,
recovery, or delivery.

## Decision

Tiber is a standalone harness and the sole authority for its sessions, agents,
tasks, workflow, tools, memory, repository effects, verification, delivery, and
audit history. The marketplace bootstrap was retired by the Tiber-only
repository cutover; it is historical reference material, never a Tiber runtime
dependency, and cannot change global user settings.

## Consequences

Authority and recovery are explicit and testable. Tiber must implement services
previously approximated by host policy, and ordinary Codex remains free to edit,
verify, commit, and push while bootstrapping Tiber.

## Alternatives considered

Codex boundary enforcement was rejected because the required host guarantees
are unsupported. Plugin-only orchestration was rejected because advisory text
is not durable authority.

## Revisit when

A host exposes stable, attestable identity, policy, isolation, and durable
workflow primitives equivalent to Tiber's contracts.
