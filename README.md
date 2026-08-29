# Tiber

Tiber is an incubating Pi-native development system. Pi owns provider authentication, model discovery, sessions, turns, tools, lifecycle events, and presentation. A thin TypeScript extension delegates material decisions to one Rust package and installed `tiber` executable.

The repository was reset before this redesign. Mature task, review, workflow, persistence, and delivery behavior will be selectively copied and consolidated from the legacy `ai-plugins` repository; that repository is a behavioral and historical source, not Tiber's runtime or development location.

## Current status

The current implementation is a boundary proof, not operational parity. It provides:

- a Pi package and TypeScript extension;
- a versioned, bounded JSONL Rust process protocol;
- a package-owned local Cargo installation flow;
- a Rust-backed semantic model-routing proof;
- a Rust-decided Pi lifecycle interception proof;
- a Pi-native doctor command and initial skill.

Tiber ticket tracking, durable workflow/review recovery, fresh-context attestation, and delivery are not yet available through the Pi package. See [`PLAN.md`](PLAN.md) for the ordered migration plan.

## Local bootstrap

Requirements: a compatible Pi installation and Cargo/Rust 1.96.

```sh
npm run runtime:install
pi -e .
```

The installer uses `cargo install --locked --path . --root <staging>`, verifies executable/version/protocol compatibility, and atomically activates `.runtime/current`. It never installs into the global Cargo bin directory.

For a non-interactive package-load check:

```sh
pi -e . --list-models
```

## Pi operations

- `/tiber-doctor` checks the package-owned Rust executable and protocol.
- `tiber_route` asks Rust to authorize and apply a semantic model role.
- Direct `git commit` and `git push` tool calls are temporarily blocked by a Rust lifecycle decision until the delivery workflow is migrated.

The adapter has no TypeScript policy fallback. Missing, incompatible, timed-out, or malformed Rust behavior fails closed.

## Removal

```sh
npm run runtime:remove
```

This removes only package-owned compiled/runtime state. `pi -e .` does not persist Pi settings.

## Development

```sh
npm ci
npm run verify
```

No crate or npm publication should occur without explicit approval.
