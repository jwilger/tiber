# ADR 0007: Owned worktrees, processes, and recovery

Status: Accepted

## Context

Concurrent or interrupted autonomous tasks must not corrupt the coordinator
checkout, leak processes, or destroy ambiguous user work.

## Decision

Use dedicated owned branches and worktrees by default. Track canonical paths,
claims, heartbeats, and process groups in a durable local registry. Reconcile on
startup and shutdown. Preserve abandoned uncommitted source in bounded private
local Git recovery refs before cleanup. Never delete foreign or ambiguous
paths; never push recovery refs automatically.

## Consequences

Post-mutation blockers retain their claim and workspace. Done requires verified
claim release and cleanup. Tiber work exists only while Pi is alive; shutdown
checkpoints rather than spawning a daemon.
