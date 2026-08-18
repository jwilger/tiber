# Tiber Product Requirements

## Product

Tiber is a standalone, local-first development harness for one repository
owner. Tiber owns the authoritative state and execution of development work.
Its `codex app-server` inference design passed the corrected Phase 1
effective-authority spike and is the accepted v1 transport. A source-level
interactive transcript/composer now runs task-bound durable sessions through
the checked workflow trampoline. Prompts, workflow effects, observations,
receipts, and transcript facts are published to signed authority before their
dependent action; interrupted uncertain effects stop at an explicit reconcile
next action rather than being replayed.

The v1 product and executable are named `tiber`. The existing task board is
named Tiber Tasks. Running `tiber` without arguments opens the current
interactive terminal UI. Native task operations live only under `tiber tasks
…`. The shipped task slice includes read-only `list`, `show`, `search`, and
`next` queries over EventCore history preserved on the signed Tiber authority
branch, plus command-specific native operations: `create <title>`, `start <ref>`, `acceptance check`,
`subtask check <ref> <one-based-occurrence>`, and `transition <ref> done`.
Each folds only the facts needed for that decision, publishes a closed modeled
fact sequence through the exact board/task consistency boundary, signs the
candidate, and uses an exact-base lease to publish to the fixed authority ref.
Creation emits one empty backlog task and the resulting strict order on the
board stream. Its stable `--id` retry form reconciles an ambiguous publication
as an idempotent no-op when that exact creation is already durable.
The occurrence check captures the addressed subtask's complete preimage, so a
legacy duplicate ID cannot select the wrong row. Transition accepts only the
terminal `done` status; it is not a generic lifecycle setter. There is no
general public EventCore append, legacy MCP task write, or generic task-mutation
surface. A retained done task with stale strict-board entries receives only an
order reconciliation, never a repeated transition. Publication reconciliation
and workflow scheduling remain subsequent native slices.

`start` is a bounded activation operation, not a general transition: it can
activate only the current eligible next task while no other task is active. An
exact retry for that sole active task succeeds without publishing another
authority revision. It does not implement scheduling or a workflow loop.

The native workflow is the authority for interactive inference.
`tiber-workflow-core` provides serializable semantic identities, a total
`step(state, observation)` trampoline, and one closed `Infer` effect with
bounded deadline, provenance, and idempotency data.
`tiber-workflow-service` provides command-specific EventCore decisions to
initialize a workflow, request the effect, record its observation, and advance
the trampoline. Recording an observation persists `EffectObserved` in its own
transaction; only a later advance decision may call `step` to request, complete,
or stop. The service exposes neither a generic workflow append nor an effect
executor. The CLI interprets only its closed `Infer` effect through app-server,
records the observation and terminal advance durably, and restores the TUI
projection on relaunch. App-server tools remain inert; broader scheduling and
operator-directed resolution of uncertain effects remain later native slices.

The native task surface also has one explicit legacy-data repair, not a general
write: `subtask repair-duplicate`. It requires a one-based occurrence and a new
identity, folds the exact current subtask as a precondition, and appends a typed
correction fact through the same board/task lease. This lets an owner repair a
malformed duplicate subtask ID without rewriting signed history or ambiguously
changing the first matching row.

### Current repository-mutation vertical slice

The interactive harness now connects model-proposed repository writes to the
existing assignment-bound authority and isolated Linux adapter. The app-server
request remains inert structured input until Tiber rereads the exact target,
constructs the diff, publishes the safe proposal identity to signed authority,
and receives an explicit owner decision. Denial or cancellation is signed,
performs no adapter dispatch, and returns the conversation to prompting. A
changed preimage produces a signed exact-digest reproposal and requires another
owner approval.

`tiber-repository-core` remains the pure authority boundary for narrow
assignment-bound repository file mutations. Its opaque authorization
permits a write with an absent-file or exact-digest precondition, or a delete
with an exact-digest precondition. It models typed mutation receipts and
failures plus read-only reconciliation without performing filesystem, Git,
process, or network I/O.

Authorization requires the complete workflow provenance, repository identity,
and component-aware assignment scope to match the trusted mutation policy and
an opaque `RepositoryMutationApproval` bound to that exact safe proposal
identity and policy/assignment context. A raw proposal cannot reach a repository
adapter.

An unknown mutation outcome is reconciled by stable mutation identity before any
later decision; it is never auto-replayed. This is neither a generic filesystem
nor shell-runner API, and it does not extend `tiber-store-git` beyond its fixed
signed `tiber` authority-branch publication role.

`tiber-repository-service` owns command-specific EventCore 2.0.1 models for
proposal, reproposal, approval, denial, cancellation, preparation, terminal
outcomes, and reconciliation. Every command is registered with the experimental
checked-model graph and must verify without provenance warnings. Preparation is
two-phase: signed `Prepared` is confirmed before verified history may mint the
opaque adapter authority. The CLI then dispatches only through
`tiber-repository-linux`, the x86_64 Linux-only
`RepositoryService` adapter. It runs only opaque bounded authorizations and
reconciliation through a fixed, private `tiber-repository-worker` under
Bubblewrap. The model and caller cannot provide shell text, arbitrary argv,
cwd, environment, mount, or network configuration. The adapter owns bounded
operational timeouts, cancellation, child cleanup, and typed non-durable
outcomes. It adds no shell, generic runner, or arbitrary filesystem surface.

Signed EventCore history is the business authority for `Proposed`, owner
decision, `Prepared`, terminal `Applied`/`Failed`/`Unknown`, and `Reconciled`
facts. A restart with signed `Prepared` and no terminal fact derives only a
read-only reconciliation handle, never mutation authority or replay. The Linux
adapter's private full-fsync journal remains operational evidence inside that
query; it cannot initiate recovery independently of signed history. Tiber signs
one reconciliation outcome, and later restarts neither query again nor append a
duplicate result. These receipts make no broader claim about working-tree
filesystem durability.

The clean x86_64 Linux package exposes public `tiber` and keeps the worker plus
Bubblewrap helper private under `libexec`. CI's package smoke verifies package
layout and entry behavior only; real adapter tests remain outside that smoke.

### Current external-tools S1 boundary

The external-tools boundary is now implemented but is not connected to a
workflow. `tiber-external-tools-core` is pure: the global, workflow-mode,
agent-role, session, assignment, and effect-policy grants all intersect and
bind to one `IntegrationId` before they mint opaque authorizations for tool
list/call, Tiber-owned root declaration, optional resource list/read, or
optional prompt list/get. A root URI can leave the core only through the
dedicated root authorization; server metadata and resource/prompt outputs stay
bounded and untrusted.

`tiber-rmcp-client` pins RMCP 3.1.2 and interprets only bounded absolute
direct-argv stdio and loopback Streamable HTTP. It rejects proxies, redirects,
automatic replay or reinitialization, SSE resumption, resource templates,
subscriptions, cache directives, and interactive continuations. Mutating tool
calls carry an idempotency identity and ambiguous outcomes enter
reconciliation. Sampling, elicitation, and MCP tasks are explicit refusals.
This slice adds no `TiberEffect`, EventCore, CLI, TUI, app-server, scheduler,
or runner integration and makes no live external-service validation claim.
The completed S3 audit boundary remains pure and unconnected.

### Current memory S2 boundary

`tiber-memory-core` provides a swappable `MemoryBackend` port;
`tiber-hindsight-http` is its first adapter and keeps private DTOs for the
schema-verified Hindsight HTTP API 0.8.3 and 0.8.4 contracts at that boundary.
The adapter supports only asynchronous retain,
operation status, cancellation, forget, recall, and named read-only
reconciliation. It uses an explicit
endpoint only: Tiber does not install or globally configure Hindsight, retry
requests, manage Hindsight authentication, or claim generic or deployment
service validation.

Every memory operation is scoped by strict owner and repository provenance,
with typed repository, agent, session, task, and memory-kind tags. Backend
document and operation handles are stable and scope-bound. An ambiguous
mutation carries a read-only reconciliation handle rather than granting a
replay. Recall is bounded, advisory, untrusted,
provenance-carrying context—not authority for a workflow, decision, or effect.
Retain requests name their source turn, and recall requests never admit that
same turn. Memory
failures are visible and normally nonfatal. This S2 boundary has no EventCore,
workflow, CLI, TUI, app-server, or scheduler integration.

### Current audit and integration S3 boundary

`tiber-integration-audit` defines provider-neutral, serializable facts for
memory and external-tool interactions. Its facts retain trusted provenance,
stable policy/operation outcomes, reconciliation identities, and bounded
evidence—not raw memory text, recall queries or recalled content, tool
arguments, integration configuration, transport detail, or server payloads.
Observed external payloads contribute only a byte count and domain-separated
digest. The DTOs neither publish EventCore facts nor create workflow,
scheduler, CLI, TUI, app-server, or runner authority.

Local deterministic fake-server tests cover no-I/O policy denial, sanitized
tool observation/ambiguity/reconciliation, and scoped memory lifecycle plus
hostile-input handling. An ignored Hindsight live test is available only with
both `TIBER_RUN_LIVE_HINDSIGHT=1` and a nonempty
`TIBER_HINDSIGHT_ENDPOINT`; it uses nonce-isolated synthetic data and
exact-document cleanup. It is not part of default CI, which remains
network-free. The explicit check passed against a local loopback Hindsight
0.8.4 service on 2026-08-14; this is not a deployment claim or support beyond
the schema-verified API versions.

## Problem

An inference client cannot safely serve as the authority for durable task,
workflow, tool, memory, repository, verification, or delivery decisions.
The former marketplace's advisory instructions were useful bootstrap context,
but they could not prove agent identity, isolate a process, reconcile an
ambiguous write, or resume a partially completed delivery after a crash.

Tiber gives an individual developer one inspectable authority for those
concerns. The intended product retains Codex's subscription-backed inference
and familiar terminal interaction behind the accepted app-server authority
boundary.

## Target user

The v1 user is a single repository owner developing on x86_64 Linux. Tiber
trusts that owner, the repository, local environment, configured toolchain,
PATH, and explicitly configured integrations. It protects against ordinary
mistakes, malformed or hostile external data, model errors, interruption,
crashes, stale or corrupt state, partial I/O, ambiguous remote results, and
remote data loss. Malicious local root, intentional owner bypass, and a
compromised trusted toolchain are outside the v1 threat model.

## Target primary workflows

1. Start `tiber`, resume or create a session, select a Tiber Task, and converse
   with streaming inference.
2. Inspect the model's proposed tools, apply Tiber policy and owner approvals,
   execute authorized effects, and retain durable receipts.
3. Delegate bounded assignments to typed agents while Tiber owns identity,
   context, budgets, cancellation, and no-progress termination.
4. Inspect repository state, edit in an assignment boundary, run isolated
   processes, verify behavior, review changes, and reconcile failures.
5. Commit, push, open or update a pull request, recover CI, and resume delivery
   after restart from EventCore facts.
6. Use native Tiber Tasks and development-workflow services, third-party MCP
   integrations, and optional Hindsight memory without internal MCP loopback.

## Terminal experience

The target TUI preserves the useful Codex transcript, streaming, composer,
Plan mode, `/side`, `/btw`, resume, diff display, status bar, and status card
interactions. It will add workflow phase, active task, assignment, agent, gate,
memory, and integration health plus `/tasks`, `/memory`, and
`/integrations`. The current extracted slice provides transcript, streaming,
composer, typed failures, and inert-tool proposals. UI state is a projection;
it never grants authority.

## Functional requirements

- Create invariant-carrying agent, session, assignment, attempt, effect, task,
  and workflow identities.
- Construct bounded context with provenance and explicit trust labels.
- Use `codex app-server` as the sole inference transport and delegate
  subscription/browser login, credential storage, refresh, account selection,
  endpoint selection, headers, streaming, and authentication errors to it;
  use a direct isolated `codex login --with-api-key` stdin handoff for
  API-key-mode setup and verify it through app-server account status.
- Run app-server with a Tiber-owned isolated Codex home that cannot load the
  user's Codex plugins, hooks, agents, MCP servers, tools, or global settings.
- Parse streamed text and structured tool requests; never let the model execute
  a tool.
- Own Tiber Tasks and development-workflow operations through native services.
- Execute configured third-party MCP servers through a harness-owned client.
- Provide a swappable memory port with the schema-verified Hindsight HTTP API
  0.8.3 and 0.8.4 contracts as the first adapter, bounded asynchronous
  retain/status/cancel/forget/recall/reconciliation operations, strict
  owner/repository provenance and tags, and advisory untrusted recall.
- Isolate bounded repository mutations behind the x86_64 Linux
  `tiber-repository-linux` platform adapter; generic process effects remain
  future product scope.
- Record durable facts and receipts for decisions, mutations, tests, memory,
  retries, cancellation, reconciliation, verification, and delivery.
- Resume safely after cancellation, interruption, crash, stale state, corrupt
  state, concurrency, or an ambiguous remote result.

## Non-functional requirements

- Functional core and imperative shell with explicit serializable trampoline
  steps; no closure continuations.
- Semantic types at domain boundaries and typed expected errors with stable
  codes, structured context, causal chains, and retryability.
- Explicit bounds for attempts, elapsed time, tokens, tool calls, cost where
  applicable, and no-progress detection.
- EventCore commands for durable decisions, command-specific folds, and no
  unconsumed provenance in checked models.
- Exact RED–GREEN implementation authority: every new or changed first-party
  product behavior records a public-boundary failing scenario before
  implementation, and the authorized production delta may address only that
  exact failure. A predicted compiler diagnostic may serve as RED when a
  missing type/API/trait/case is the intended boundary; incidental compilation
  breakage may not. When an outer BDD failure has multiple plausible causes,
  Tiber requires drill-down RED evidence at progressively narrower behavioral
  boundaries until one cause is explicit, and authorizes only that leaf fix
  before rerunning outward. After generation, an independent fresh-context
  exact-failure-conformance review compares the complete production delta with
  the durable RED evidence and blocks every later phase when the delta exceeds
  that authority. Explicit exemptions cover simple development scripts, CI
  workflows, covered refactors, and removals; tests never assert committed text
  merely to manufacture evidence.
- Rust Edition 2024, forbidden unsafe code, strict workspace Clippy inheritance,
  and warnings denied in CI.
- Credentials are never read, copied, logged, decoded, retained, serialized,
  forwarded, or traced by Tiber. API-key-mode login hands inherited owner stdin
  directly to the isolated `codex login --with-api-key` child with ambient
  API-key environment variables removed and child output suppressed; Tiber
  records only a stable exit diagnostic, kills and reaps the child if its
  configured ten-minute deadline expires, and requires app-server to confirm the
  resulting `ApiKey` account state. Codex's non-terminal-stdin requirement keeps
  owner-supplied key input out of Tiber's memory and domain handling.
- Observable model, context, policy, tool, memory, and delivery decisions with
  sensitive data redacted.
- Reproducible x86_64 Linux packaging and clean-machine installation.

## Acceptance criteria

- A deterministic fixture pins the app-server protocol control surface before
  conversation construction, and an opt-in live probe verifies effective
  authority for each supported Codex version.
- No operation outside Tiber policy can produce an effect. Denied built-ins may
  remain in the protocol; explicitly permitted read-only, non-shell repository
  observation remains untrusted inference context; and Tiber-declared dynamic
  tools remain inert until the harness authorizes and executes them.
- Native tasks, workflow, repository, process, verification, and delivery
  services operate without MCP or shell loopback into Tiber.
- MCP denial, cancellation, ambiguous-write, hostile-input, and capability
  negotiation cases pass.
- Hindsight fake-server tests prove scoped memory, provenance budgets,
  cancellation, and nonfatal failure behavior without a live-service claim.
- Crash/restart, stale/corrupt state, concurrency, and clean-machine packaging
  cases pass.
- Formatting, strict Clippy, deterministic behavior, EventCore,
  semantic-type property, trampoline, mutation, TUI snapshot, and secret-leak
  gates pass. Qualitative evaluation design is deferred until the complete
  harness workflow is available; provider or stochastic evaluations are not
  current product or CI gates.
- Every roadmap increment is reviewed, committed with a rationale-bearing
  Conventional Commit, pushed, confirmed green in CI, and closed in Tiber.

## Non-goals

- Platforms other than x86_64 Linux in v1.
- Direct OpenAI Responses API or Anthropic/Claude inference providers.
- `codex --remote` or Codex runtime authority.
- Installing or globally configuring Codex, Claude, MCP, Hindsight, shells, or
  SSH.
- MCP sampling, elicitation, or MCP tasks in the initial integration.
- Hindsight `reflect` as the primary agent reasoning mechanism.
- Compatibility aliases, deprecated task commands, transition crates, or a
  command/package migration window.
- Defending against malicious local root, intentional owner self-bypass, or a
  compromised trusted local toolchain.
