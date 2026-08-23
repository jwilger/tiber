# ADR 0004: Signed shared task ref

Status: Accepted

## Context

Tasks, specifications, claims, dependencies, and completion evidence must be
shared across collaborators without requiring a forge-specific service.

## Decision

Store append-only versioned task event batches on
`refs/heads/tiber/tasks/v1`. Publish signed commits by normal fast-forward push
from the exact observed remote head. Verify configured signers and ancestry
before projecting state. Use exclusive remotely published claims; heartbeat
staleness never transfers ownership automatically.

## Consequences

Git is the collaboration protocol and concurrent publication uses
compare-and-swap reconciliation. Invalid signatures, malformed events, or
rewritten history degrade the board to read-only. Local transcripts, trust,
artifacts, and recovery state never enter the task ref.
