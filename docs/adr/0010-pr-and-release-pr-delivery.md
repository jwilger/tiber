# ADR 0010: PR and release-PR delivery

Status: Accepted

## Context

The public repository needs protected collaboration, fast local commits, full
remote verification, semantic versions, and tokenless npm publication.

## Decision

Require pull requests for `main`, linear squash history, Conventional
Commit-compatible titles, resolved conversations, and one aggregate full-CI
status. Permit authorized ordinary PR authors to enable auto-merge after all
gates. Use a pinned release-please workflow to maintain a release PR. Never have
Tiber auto-merge that PR; a human explicitly merges it. Then create the tag and
GitHub Release and publish through npm trusted OIDC with provenance.

Local hooks run only formatting, strict lint, incremental type checking, fast
unit tests, and message validation. There is no heavy pre-push hook; complete
verification runs in CI.

## Consequences

Every release has a human publication boundary without a long-lived npm token.
Ordinary work remains autonomous after deterministic gates. Nix is not part of
CI.
