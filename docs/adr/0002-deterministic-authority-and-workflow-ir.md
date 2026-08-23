# ADR 0002: Deterministic authority and workflow IR

Status: Accepted

## Context

Prompt guidance alone cannot enforce development workflow or authorize host
effects safely. Executable workflow callbacks would make repository data code.

## Decision

Use a functional core returning closed typed effects and stable failures.
Compile versioned data-only JSON workflows into immutable canonical IR with a
digest. Enforce a non-configurable policy floor for task claims, reviewed
specifications, RED/GREEN, review, exact-revision delivery, CI, and cleanup.
Models provide semantic judgments but never authority.

## Consequences

Project workflows may rearrange or omit only optional stages and can only
narrow authority. Every active run pins its workflow and policy digests. New
effect or node kinds require product code and architectural review.
