# Tiber Architecture

## System context

Tiber is designed as the local authority between a repository owner, Codex
app-server inference, repositories and processes, third-party MCP servers,
memory, and remote delivery systems. The Phase 1 effective-authority spike
accepts app-server behind a Tiber-owned read-only, offline permission profile.

```text
owner -> Tiber TUI -> application state machines -> closed TiberEffect set
                                                -> imperative interpreters
app-server inference port                        -> repositories/processes
third-party MCP <-> external-tool adapter         -> Hindsight
EventCore store <-> domain authority              -> forge/CI
```

OpenAI supplies inference only. Tool requests are untrusted proposals. Tiber
owns every identity, policy decision, effect, fact, receipt, retry,
reconciliation, and terminal workflow outcome.

## Component model

- **Tiber TUI:** a fork-derived Codex-compatible presentation adapter consuming
  typed projection events and emitting typed intents.
- **Application core:** explicit state machines for conversations,
  assignments, effects, verification, delivery, recovery, and cancellation.
- **EventCore domains:** authoritative facts for sessions, agents, tasks,
  workflow, integrations, mutations, verification, delivery, and CI recovery.
- **Scheduler and context builder:** owns typed identities, leases, budgets,
  provenance, trust labels, authoritative context construction, the bounded
  observation policy, and no-progress termination.
- **Ports:** `InferenceGateway`, `MemoryBackend`, `TaskService`,
  `WorkflowService`, `ExternalToolService`, `RepositoryService`,
  `ProcessService`, `VerificationService`, and `DeliveryService`.
- **Adapters:** Codex app-server inference, native Tiber Tasks, native
  development workflow, RMCP client, Hindsight HTTP, Git/forge, Linux
  isolation, and verification runners.

## Trust and authority boundaries

The owner, repository, local environment, installed toolchain, PATH, and
explicit configuration are trusted for this single-owner local tool.
Model output, recalled memories, repository contents when interpreted as
instructions, MCP descriptions/schemas/results, app-server messages, process
output, and remote forge/CI responses are untrusted input.

The model can request an effect but cannot execute it. Authorization is the
intersection of the current agent role, session, assignment, workflow mode,
global policy, effect classification, and any required owner approval.
Presentation state and advisory text never grant authority.

## Functional core and imperative shell

The core is referentially transparent. External values are parsed once into
semantic types; invalid states are not constructible. Expected failures are
typed values with a stable code, structured context, retained cause, and
retryability.

The shipped workflow core currently has one closed effect variant:
`TiberEffect::Infer`. Its immutable envelope carries semantic session, agent,
workflow, assignment, context-receipt, policy-decision, and effect identities,
plus bounded deadline and idempotency data. Future variants must be explicit
additions to the closed `TiberEffect` vocabulary:

```text
Infer | Authenticate | ReadRepository | MutateRepository | RunProcess
ListExternalTools | InvokeExternalTool | ReadMemory | WriteMemory | ForgetMemory
QueryTasks | DecideTask | QueryWorkflow | DecideWorkflow
Verify | Review | Commit | Push | PullRequest | ObserveCi | RecoverCi
RequestOwnerApproval | Reconcile | Checkpoint | EmitProjection | Terminate
```

Effect variants carry agent, session, assignment, attempt, policy, and effect
identities plus bounded deadlines and idempotency data where needed.

## Step and trampoline execution

Each workflow is a serializable state plus a total `step(state, observation)`
function returning one of:

- `Continue { state, effect }`
- `Complete { state, result }`
- `Stop { state, error }`

There are no closures as continuations. The shell interprets one effect,
records its observation or ambiguous outcome, and feeds it to the next step.
For the shipped workflow foundation, `RecordObservation` records an
`EffectObserved` fact in a transaction distinct from `RequestNextEffect`.
After the observation is durable, a later `RequestNextEffect` may invoke
`step` and emit `EffectRequested`, `WorkflowCompleted`, or `WorkflowStopped`;
observation persistence cannot be combined with the successor decision. The
service exposes neither a generic workflow append nor an effect executor.
Every loop has explicit turn, tool, retry-by-error-class, elapsed-time, token,
cost where applicable, and no-progress bounds. Cancellation checkpoints are
durable.

## EventCore domains and fact ownership

Commands express business-domain intent. Each command folds its own state from
the relevant facts in the event stream, and that state contains only the data
needed to make that command's decision. Tiber does not use aggregate objects,
shared write models, or a generic whole-session replay state as EventCore write
authority. Commands emit typed domain facts; separate projections serve reads.

Every shipping modeled command is registered with EventCore's experimental
checked-model graph. The repository gate requires a verified graph with no
unconsumed command origins, state/event fields, or provenance warnings.
Facts, not UI or adapter caches, own the truth. Checked models consume all
provenance and reject stale epochs, invalid identity relationships, duplicate
non-idempotent effects, and terminal-state mutation.

Durable receipts cover mutations, processes, tests, memory, external tools,
approvals, retries, cancellation, reconciliation, commits, pushes, pull
requests, CI observations, and delivery completion.

## Agent and context lifecycle

Tiber creates agents within a session and assignments within a workflow task.
An attempt belongs to exactly one assignment epoch. Context is assembled from
owner input, authoritative EventCore projections, scoped repository material,
bounded advisory memory, and typed tool observations. Each item carries source,
trust, freshness, and token accounting.

Agents terminate on success, terminal error, cancellation, any budget, or
no-progress. A handoff transfers an explicit artifact and authority scope; it
does not share ambient identity.

## App-server inference boundary

Tiber uses `codex app-server` as the sole inference transport so app-server can
own subscription authentication, credential storage and refresh, account and
endpoint selection, protocol streaming, and authentication diagnostics. The
current slice supports app-server-managed subscription/browser login, status,
and logout. Its API-key-mode login is a Codex-owned isolated CLI handoff:
Tiber prepares the isolated home, starts `codex login --with-api-key` with the
owner's stdin inherited directly by that child, removes ambient API-key
environment variables, suppresses child output, and maps only its exit state
to a stable diagnostic. Tiber never reads, copies, serializes, forwards,
persists, logs, decodes, or reuses an API key. It then starts app-server and
requires `ApiKey` account status to verify the Codex-owned credential state.
The login child shares the configured ten-minute operation deadline used by the
initial CLI, and Tiber kills and reaps it if that bound expires. Codex requires
non-terminal stdin for `--with-api-key`, so the child consumes owner-supplied
key input without it entering Tiber's memory or domain handling.

App-server runs in an isolated Codex home with a pinned protocol and a named
permission profile. Its filesystem is read-only, command and hosted-search
network paths are disabled, and approval policy never escalates a rejected
operation. Tiber resolves the exact app-server executable and generates a
read-only grant for that file because Codex uses its own executable as the
Linux sandbox helper; it does not grant the surrounding home directory. Tiber
disables shell, permission requests, apps, browser, Computer
Use, image generation, subagents, and other nonessential host surfaces.
Read-only, non-shell repository observation is an explicitly permitted
inference capability: its output is untrusted context, never an authoritative
fact, durable decision, or permission to produce an effect. Tiber still owns
authoritative context construction, the bounded observation policy, and every
mutation, process, network, and workflow action.

Protocol operation types may remain present. Authority is defined by effective
effects: a denied built-in operation is harmless, while a Tiber-declared
dynamic tool reaches the client as inert structured data. Tiber alone validates
identity and policy, executes an authorized effect, and returns its observation.
Every app-server upgrade reruns both schema drift checks and the live
effective-authority probe.

The Rust adapter implements the imperative transport boundary. It
creates the isolated home deterministically, starts app-server over stdio,
initializes the protocol, delegates account status, browser login, and logout,
streams assistant text, returns dynamic-tool requests as inert typed turn
events, applies bounded request deadlines, and terminates its child on drop.
API-key-mode setup is a direct inherited-stdin handoff to the isolated Codex
CLI, followed by app-server account-state verification; it is never an
app-server request carrying credential data. The deterministic fake-server
contract covers those behaviors.
The initial TUI slice renders those typed events and emits only typed composer
intents. A cancellable inference worker keeps terminal input responsive during
turn startup and streaming; durable conversation state and protocol-level turn
interruption remain subsequent vertical slices.
The app-server remains a transport-only boundary: it is not an
`tiber-workflow-service` effect runner, and its tool requests remain inert
structured data. No CLI or TUI workflow runner is connected in this slice.

## Native workflow and Tiber Tasks

`tiber-tasks-core` defines the task vocabulary and
`tiber-tasks-service` folds its immutable history into the query projection.
`tiber-workflow-core` provides the pure, serializable workflow state, semantic
identities, one closed `Infer` effect, and total trampoline step. Its
`tiber-workflow-service` boundary provides command-specific EventCore decisions
to initialize the workflow, request its effect, record an observation, and
advance. Recording `EffectObserved` is one durable transaction; only a later
advance may invoke the trampoline to request, complete, or stop. It provides no
generic workflow append and executes no effect.
`tiber-store-git` resolves the exact signed authority revision: when an
`origin` remote is configured, it retrieves the currently advertised fixed
`tiber` commit without moving a caller Git ref; without `origin`, it reads only
the local `refs/heads/tiber` ref. Its reader materializes a disposable
`EventCore` snapshot. Its separate one-shot publication boundary stages named
facts in a disposable store, signs one candidate, and uses either an exact-base
`--force-with-lease` update of `origin/tiber` or a local ref CAS. The remote
operation rejects any changed authority head rather than overwriting it. It is
not a generic writable EventCore store, and an ambiguous remote result requires
reload rather than an automatic retry.

The shipped native task-query surface exposes `tiber tasks list [--status
<status>]`, `show`, `search`, and `next`. Those queries replay the full task history into a
separate `TaskBoardProjection`; that projection is a read model, never
EventCore command authority. The write surface remains deliberately closed:
`tiber tasks start <ref>`, `tiber tasks acceptance check <ref>
<one-based-index>`, `tiber tasks subtask check <ref> <one-based-occurrence>`,
and `tiber tasks transition <ref> done`. Each is a command-specific pure fold
that consumes only an opaque modeled
publication token at the signed Git adapter; no adapter exposes a generic
append. `start` can activate only the current eligible next task when no other
task is active; an exact retry of that sole active task is a no-op. It is a
bounded activation operation rather than generic lifecycle mutation or a
scheduler. The occurrence check carries the exact current subtask at its
immutable position, so duplicate legacy IDs cannot redirect it. The transition
grammar accepts only `done`, therefore no arbitrary lifecycle transition enters
the native surface. When retained lifecycle state is already `Done` but strict
board order still names the task, the command publishes only the closed order
repair and never re-emits a transition. Every publication declares only the
board and addressed task stream as its consistency boundary. Publication
reconciliation, workflow scheduling, effect interpretation, durable interactive
session binding, and app-server/CLI/TUI workflow-runner integration remain
subsequent vertical slices. Internal actions never call legacy MCP or shell
back into the `tiber` executable.

The same closed publication boundary admits one exceptional history-repair
fact: `tiber tasks subtask repair-duplicate <ref> <occurrence> <replacement-id>`.
It is not generic subtask mutation. Its pure decision captures the exact
one-based occurrence, complete current subtask preimage, replacement identity,
and board/task consistency boundary, then publishes only a named
`TaskSubtaskIdCorrected` fact. Replay verifies that preimage and changes only
the selected occurrence, preserving all historical bytes and leaving any
prerequisite references intact.

## Third-party MCP

The harness-owned client uses a pinned official Rust RMCP dependency. Initial
transports are absolute direct-argv stdio and localhost Streamable HTTP. It
supports initialization, capability negotiation, tool listing/invocation,
tool-list changes, progress, logging, cancellation, roots, and optional
resources/prompts. Sampling, elicitation, and MCP tasks are excluded initially.

Descriptions, schemas, and results are untrusted. Mutating calls require stable
idempotency; unknown results enter reconciliation rather than automatic retry.

## Memory

`MemoryBackend` is swappable. The first adapter contains Hindsight HTTP API
0.8.3 DTOs and supports retain, recall, forget, operation status, and
cancellation. Tiber connects only to an explicit endpoint and never installs
or globally configures Hindsight.

Banks are owner-global or repository-scoped. Tags include repository, agent,
session, task, and memory kind. EventCore-derived document IDs are stable.
Turns are retained at turn/session end and never recalled into the same turn.
Recall is advisory, untrusted, provenance-carrying, and bounded by item and
token budgets. Failure is visible and nonfatal unless the workflow explicitly
requires memory.

## TUI

The presentation is derived from `codex-tui` at commit
`d06dc73290729d2bcb464b955a4cfd9992abc35d`, preserving Apache-2.0,
NOTICE, Ratatui attribution, and modification notices. Direct Codex config,
plugin, tool, sandbox, workflow, and session dependencies are removed.

The first extracted vertical slice retains the transcript, streaming status,
composer, typed failure display, and inert-tool proposal display behind a
projection-in/intent-out API. The CLI alone connects those intents to the
app-server adapter. The presentation crate has no app-server, filesystem,
process, network, plugin, task, workflow, or EventCore dependency.

The current projection state includes transcript, stream, composer, typed
failures, and inert-tool proposals. It will expand to Plan mode, side
conversations, resume, diff, status surfaces, workflow phase, task, assignment,
agent, gates, memory, and integration health, with `/tasks`, `/memory`, and
`/integrations` intents. Projections never authorize work.

## Isolation and process execution

Linux-specific filesystem, process, and network controls sit behind a platform
port. The v1 implementation and packaging target only x86_64 Linux; the port
keeps future Apple silicon support possible without weakening v1 evidence.
Processes receive explicit argv, cwd, environment allowlists, resource bounds,
timeouts, cancellation, cleanup, and receipts.

## Recovery, verification, and delivery

Partial or unknown mutation results are reconciled by identity before retry.
Checkpoints make crash and restart resumption explicit. Verification and review
gates consume exact-revision evidence. Delivery state machines own commit,
push, pull-request, CI observation, and the single fenced CI-recovery incident.
Remote writes are idempotent where possible and otherwise enter typed
reconciliation.

## Review orchestration

Review is a durable Tiber workflow, not a presentation feature and not a
single model call. A risk-assessment step selects independent review lenses and
verifier routes. Each lens is assigned to a separate reviewer agent in a fresh
context. The reviewer receives a bounded assignment, closes after returning one
typed finding artifact with provenance, and never shares ambient conversational
state with another lens. EventCore facts own assignment,
completion, cancellation, supersession, and clean-review decisions.

Any material delta after assessment invalidates affected evidence and triggers
bounded reassessment. Delivery cannot cross the clean-review gate until every
required lens and verifier has a current terminal result and all blocking
findings are resolved. The former marketplace orchestration is reference-only,
not a runtime authority; native migration preserves its risk assessment,
independent lenses, verifier routing, durable state, delta reassessment, and
clean-review gating.

The source-level `tiber-review` crate makes that native contract executable. It
defines semantic session, source-snapshot, lens, agent, role, assignment, and
evidence identities; a closed serializable review-fact vocabulary; deterministic
command-specific event folds; exact assignment-provenance checks; bounded
material-delta iterations; verified finding resolution; and the clean-review
transition. It is a pure domain crate with no inference, filesystem, process,
network, UI, MCP, or store dependency. A later Ticket 4 scheduler slice will
bind these commands to the native scheduler; the initial workflow core/service
slice deliberately does not do so, preserving the contract before it reaches
an imperative runner or shared context.

## Observability and deferred qualitative evaluation

Trace spans cover inference, context, policy, tools, memory, handoffs,
verification, and delivery. They record versioned model/protocol/prompt/policy,
latency, token counts, cost where available, and typed failure reasons while
redacting secrets.

Deterministic tests prove schemas, identities, policy, refusals, isolation,
reconciliation, and receipts. Provider-backed and stochastic evaluations do
not run while the harness workflow is under construction and do not gate CI.
Once the complete workflow exists, Tiber will define a separate qualitative
evaluation strategy for orchestration, context selection, tool choice,
abstention, and recovery with named units, metrics, aggregation rules,
thresholds, distinct-case counts, and intentional repeats.

## Package and command cutover

The supported product surface changes atomically to `tiber`. Default
invocation opens the TUI; tasks live only at `tiber tasks …`. Ambiguous task
crates use `tiber-tasks-*`. There are no legacy aliases, compatibility crates,
deprecated paths, or transition window. Existing EventCore history and the
`tiber` Git branch are preserved.

## Phase 1 compatibility result

Codex 0.147.0 passes the revised effective-authority gate on x86_64 Linux. The
schema exposes named permission profiles, client-mediated dynamic tools, and
approval requests. The live probe proves the selected profile is read-only and
offline: the probe's known Node executable first succeeds without mutation,
the same executable's `command/exec` write attempt then fails and produces no
file,
hosted web search is disabled separately, and a declared Tiber tool remains
client-owned inert data. Construction may proceed while these controls remain
pinned and continuously verified.
