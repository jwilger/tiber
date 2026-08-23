# ADR 0008: Separate Git, CI, and review ports

Status: Accepted

## Context

Git transport, CI observation, and forge review have different credentials,
permissions, failure modes, and evidence. Treating a forge as one authority
would couple Tiber to GitHub and blur receipts.

## Decision

Model Git remote, each CI authority, and optional review service as separate
ports. Require every configured CI authority to report terminal success for the
exact delivered revision. Generic CI adapters are user-local digest-pinned
executable/argv commands returning validated JSON. Add GitHub only as a thin
first-party HTTP adapter.

## Consequences

A push is not CI success and a PR permission is not Git authority. Additional
forges can implement ports independently. CI failure creates a repository-wide
delivery hold until causally resolved.
