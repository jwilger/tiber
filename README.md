# Tiber

Tiber is a standalone, local-first Rust development harness. It owns the
authoritative lifecycle for development work: sessions, agents, assignments,
context, workflow state, tools, memory, repository effects, verification, and
delivery. Inference is one bounded input to that authority, never the
authority itself.

Tiber v1 targets x86_64 Linux. The primary executable is `tiber`; running it
without an argument opens the terminal UI. Native task operations live only
under `tiber tasks …`. The shipped `list`, `show`, `search`, and `next` commands
are read-only queries over EventCore task history preserved on the signed
`tiber` authority branch. When an `origin` remote exists, they resolve its
currently advertised `tiber` commit and fetch that exact object without moving
any Git ref. They never append facts or update Git refs; task writes, publication,
and workflow scheduling remain later native slices.

`tiber tasks list` shows open work in board priority order with its status: one
item may be `in-progress` while the remaining queued work is `backlog`.

## Repository layout

- [`crates/`](crates/) — shipping Rust workspace crates.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) and [`PRD.md`](PRD.md) — product and
  system contracts.
- [`docs/adr/`](docs/adr/) — durable architectural decisions.
- [`docs/rules/`](docs/rules/) — engineering and delivery rules.
- [`old-code-for-reference/`](old-code-for-reference/) — frozen source used
  only while native Tiber services are ported; it is not built or shipped.

## Develop

Use the pinned Nix development shell:

```shell
nix develop
just ci
```

The local and CI gate runs deterministic checks only: actionlint, the Rust
lint-policy audit, formatting, strict Clippy, tests, and the app-server
authority fixture. It does not require credentials or start a live inference
session.

For the current executable slice:

```shell
cargo run --locked -p tiber -- app-server-probe path/to/protocol-schema.json
cargo run --locked -p tiber -- tasks list
cargo run --locked -p tiber -- tasks show <task-ref>
cargo run --locked -p tiber -- tasks search "outcome terms"
cargo run --locked -p tiber -- tasks next
```

Read [`AGENTS.md`](AGENTS.md) before making a change.
