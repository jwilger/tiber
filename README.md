# Tiber

Tiber is a standalone, local-first Rust development harness. It owns the
authoritative lifecycle for development work: sessions, agents, assignments,
context, workflow state, tools, memory, repository effects, verification, and
delivery. Inference is one bounded input to that authority, never the
authority itself.

Tiber v1 targets x86_64 Linux. The primary executable is `tiber`; running it
without an argument opens the terminal UI. Native task operations live only
under `tiber tasks …`. The shipped `list`, `show`, `search`, and `next` commands
are read-only queries over `EventCore` task history preserved on the signed
`tiber` authority branch. When an `origin` remote exists, they resolve its
currently advertised `tiber` commit and fetch that exact object without moving
any caller Git ref.

Tiber exposes only bounded native task activation and completion mutations:
`start <task-ref>`, `acceptance check`, `subtask check`, and
`transition <task-ref> done`. Each starts with canonical history, makes one
command-specific pure decision, and
publishes only its opaque modeled fact sequence with the board and task-stream
versions as its consistency boundary. Tiber creates a signed candidate and uses
an exact-base `--force-with-lease` update of the fixed authority branch (or a
local ref CAS when no `origin` exists), so it cannot overwrite a changed remote
head. A post-push ambiguity is reported rather than retried automatically.
`start` activates only the current eligible next task while no other task is
active; an exact retry for that sole active task is a no-op. It is a bounded
activation operation, not a generic lifecycle setter.
`subtask check` addresses a stable one-based occurrence and carries that row's
exact preimage, so retained duplicate IDs cannot redirect a check. `transition`
accepts only `done`; it is a terminal completion operation, not an arbitrary
status setter. If history already says `done` but strict board order retains
that task, the same bounded command publishes only the order reconciliation; it
does not re-emit a lifecycle transition. There is no general public EventCore
append, legacy MCP task write, or generic task-mutation surface. Publication
reconciliation and workflow scheduling remain later native slices.

The first native workflow slice is deliberately internal.
`tiber-workflow-core` defines serializable semantic identities, a total
`step(state, observation)` trampoline, and one closed `Infer` effect with
bounded deadline, provenance, and idempotency data.
`tiber-workflow-service` provides only command-specific EventCore decisions to
initialize a workflow, request that effect, record its observation, and advance
the trampoline. Recording an observation persists `EffectObserved` by itself;
only a later advance decision may call `step` to request, complete, or stop.
There is no generic workflow append or effect executor. The app-server, TUI,
and CLI do not yet run this workflow—app-server tools remain inert—and
scheduler, effect reconciliation, durable interactive sessions, and UI
integration remain later slices.

## Repository-mutation boundary (S2)

`tiber-repository-core` is a pure, unconnected authority contract for narrow
assignment-bound repository file mutations. An opaque authorization permits a
write with an absent-file or exact-digest precondition, or a delete with an
exact-digest precondition. Typed mutation receipts and failures plus read-only
reconciliation carry the bounded operation; no filesystem, Git, process, or
network I/O occurs in S1.

Authorization requires matching workflow provenance, repository identity,
component-aware assignment scope, trusted policy, and an opaque
`RepositoryMutationApproval` bound to that exact safe proposal identity and
policy/assignment context.

An unknown mutation outcome is reconciled by stable mutation identity rather than
automatically replayed. This does not create a generic filesystem or shell
runner, and it does not generalize `tiber-store-git`, which remains the fixed
signed `tiber` authority-branch publisher.

`tiber-repository-linux` is the x86_64 Linux-only `RepositoryService` adapter.
It interprets only the opaque bounded operations through a fixed, private
`tiber-repository-worker` under Bubblewrap: no model or caller can provide shell
text, arbitrary argv, cwd, environment, mount, or network configuration. S2
owns bounded timeout, cancellation, child cleanup, and typed non-durable
outcomes, but adds no workflow, EventCore, CLI, TUI, app-server, scheduler,
runner, or generic `ProcessService` integration. S3 adds durable recovery,
crash/restart and stale/corrupt-state reconciliation, concurrency evidence, and
clean-machine packaging.

## External-tools boundary (S1)

The workspace now contains a real but unconnected third-party MCP boundary.
`tiber-external-tools-core` is a pure authority contract: all six policy layers
and the configured `IntegrationId` must permit an opaque authorization for a
tool list or call, Tiber-owned root declaration, optional resource list/read,
or optional prompt list/get. Roots are disclosed only by the dedicated root
authorization. Server metadata plus resource and prompt outputs stay bounded,
untrusted data.

`tiber-rmcp-client` pins RMCP 3.1.2 and interprets only bounded absolute
direct-argv stdio or loopback Streamable HTTP sessions. It uses no proxy,
redirect, automatic replay or reinitialization, or SSE resumption. Mutations
carry an idempotency identity and an unknown result enters reconciliation;
sampling, elicitation, MCP tasks, resource templates, subscriptions, cache
directives, and interactive continuations are refused. This is not yet a
workflow `TiberEffect`, EventCore, CLI, TUI, app-server, scheduler, or runner
integration, and it does not claim live external-service validation. Hindsight
and audit/integration S3 is now a pure, unconnected audit-fact boundary.

## Memory boundary (S2)

`tiber-memory-core` defines a swappable `MemoryBackend` port. Its first
adapter, `tiber-hindsight-http`, owns private DTOs for the schema-verified
Hindsight HTTP API 0.8.3 and 0.8.4 contracts and
only supports asynchronous retain, operation status, cancellation, forget,
recall, and named read-only reconciliation. It connects only to an explicitly
configured endpoint: it neither
installs nor globally configures Hindsight, retries requests, manages
authentication, or claims generic or deployment-service validation.

Memory writes and recalls carry strict owner/repository provenance and typed
tags for repository, agent, session, task, and memory kind. Retained document
and operation handles are stable and scope-bound. An ambiguous mutation carries
a read-only reconciliation handle instead of authorizing a replay. Recall
results are bounded, advisory, untrusted context
with provenance; they cannot authorize a decision or effect. Retain requests
name their source turn, and recall requests never admit that same turn. Visible
memory failures remain nonfatal unless a
future workflow expressly requires memory. This boundary is not yet connected
to EventCore, workflow execution, the CLI, TUI, app-server, or scheduler.

## Audit facts and integration evidence (S3)

`tiber-integration-audit` supplies provider-neutral, serializable audit facts
for the memory and external-tool boundaries. They retain trusted provenance,
stable policy and operation outcomes, reconciliation identities, and bounded
evidence. Memory facts exclude raw retained text, recall queries, and recalled
content. External-tool facts exclude arguments, transport/configuration, and
server payloads; an observed payload is represented only by its byte count and
a domain-separated digest. These immutable DTOs do not publish EventCore facts
or add workflow, scheduler, CLI, TUI, app-server, or runner integration.

Deterministic local fake-server coverage exercises policy denial without server
I/O, sanitized observed/ambiguous/reconciled tool outcomes, and scoped memory
lifecycle and hostile-input behavior. The Hindsight adapter also provides an
ignored live test that runs only when both
`TIBER_RUN_LIVE_HINDSIGHT=1` and a nonempty `TIBER_HINDSIGHT_ENDPOINT` are
supplied. It uses a nonce-isolated synthetic lifecycle and exact-document
cleanup. Default CI remains network-free; this explicit operator check passed
against a local loopback Hindsight 0.8.4 service on 2026-08-14. That evidence
does not claim a deployed service or compatibility beyond the verified API
versions.

`tiber tasks show <task-ref>` renders subtasks by stable one-based occurrence,
identity, status, title, and prerequisites. The narrowly scoped
`tiber tasks subtask repair-duplicate <task-ref> <occurrence> <replacement-id>`
corrects one malformed legacy duplicate identity only: it records the exact
current subtask as a precondition, changes only that occurrence, and appends a
new named fact rather than rewriting preserved history. It is not a general
subtask-edit surface.

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
cargo run --locked -p tiber -- tasks start <task-ref>
cargo run --locked -p tiber -- tasks acceptance check <task-ref> <one-based-index>
cargo run --locked -p tiber -- tasks subtask check <task-ref> <one-based-occurrence>
cargo run --locked -p tiber -- tasks subtask repair-duplicate <task-ref> <one-based-occurrence> <replacement-id>
cargo run --locked -p tiber -- tasks transition <task-ref> done
```

Read [`AGENTS.md`](AGENTS.md) before making a change.
