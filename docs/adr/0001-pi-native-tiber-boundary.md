# ADR-0001: Build Tiber as a Pi-native package with one Rust executable

## Status

Accepted

## Date

2026-08-29

## Context

The multi-harness marketplace contains mature Rust task, review, workflow, and persistence behavior, but Claude/Codex manifests, hooks, MCP bootstrap, inference transport, and presentation are not appropriate permanent foundations for exclusive Pi use. A rewrite would discard tested domain behavior, while adding Pi as a subordinate compatibility harness would preserve unnecessary boundaries.

## Decision

This repository is the Pi-native Tiber product boundary. Pi owns provider authentication, model/catalog discovery, turns, sessions, tools, lifecycle events, and presentation. A thin TypeScript extension translates those surfaces to one versioned, bounded JSONL stdio protocol. The installable `tiber` Rust crate and binary own all material policy and durable domain decisions and fail closed when unavailable or incompatible.

Treat the legacy `ai-plugins` checkout as a behavioral specification and source donor only. Copy and adapt the necessary skills, tests, and Rust modules into this repository with provenance and applicable license notices; do not create runtime or development-time cross-repository dependencies. Consolidate reusable Rust behavior into this one Cargo package and executable while preserving distinct EventCore authorities, stream schemas, locks, and internal security boundaries. Install exact locked crate versions into package-owned host-local roots with staging, verification, serialization, and atomic activation. Do not publish without separate approval.

This decision supersedes ADR-0004 only for the future Pi product direction: Pi supplies native session/model/tool boundaries that the older Codex plugin did not. It does not weaken the requirement that advisory prompt text cannot authorize work.

## Consequences

### Positive

- Establishes Pi as the destination harness while retaining mature Rust behavior and history.
- Keeps TypeScript an adapter with no policy fallback.
- Keeps the npm/Pi package and single installed Rust entry point in the canonical Tiber repository.
- Makes protocol, executable, and model-role compatibility independently testable.

### Negative

- Legacy and Pi-native product surfaces coexist in separate repositories during migration.
- Copied source requires provenance, license review, adaptation, and compatibility tests.
- Consolidation requires careful compatibility work across distinct persistence authorities.

## Alternatives Considered

### Add Pi metadata to the legacy marketplace plugin

Rejected because it would make Pi a permanent third adapter and blur the future product boundary.

### Rewrite the system as a TypeScript Pi extension

Rejected because it would duplicate or weaken mature Rust domain policy.

### Incubate inside the legacy marketplace repository

Rejected because that checkout is a behavior/source reference and may have concurrent work. Tiber already has a canonical reset repository and package identity.

## Revisit when

Revisit if a concrete capability requires a separately isolated process, while preserving one installed `tiber` entry point where possible.

## Related

- Legacy `ai-plugins` ADR-0004
- Legacy `ai-plugins` ADR-0007
- Legacy `ai-plugins` ADR-0013
