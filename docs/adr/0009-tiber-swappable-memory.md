# ADR-0009: Use swappable memory with Hindsight first

## Status

Accepted

## Date

2026-08-10

## Context

Long-running development benefits from recall, but memory is advisory,
fallible, and not authoritative workflow state.

## Decision

Define `MemoryBackend` and implement Hindsight HTTP API 0.8.3 first. Keep its
DTOs inside the adapter; support retain, recall, forget, status, and
cancellation. Scope banks and tags, use stable EventCore-derived IDs, attach
provenance and budgets, and never recall a turn into itself.

## Consequences

Memory is replaceable and failures are visible but normally nonfatal. EventCore
remains authoritative and Hindsight `reflect` is not primary v1 reasoning.

## Alternatives considered

Hard-coding Hindsight and treating recall as authority were rejected. No memory
was rejected because it loses useful continuity.

## Revisit when

A different backend provides a materially better measured contract.
