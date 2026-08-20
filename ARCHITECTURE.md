# Tiber Architecture

## System context

Tiber is designed as the local authority between a repository owner, Codex
app-server inference, repositories and processes, third-party MCP servers,
memory, and remote delivery systems. The Phase 1 effective-authority spike
accepts app-server behind a Tiber-owned read-only, offline permission profile.

```text
owner -> reviewed Codex TUI -> private Tiber gateway -> Codex app-server
                                      |                (inference only)
                                      v
                         application state machines -> closed effects
                         EventCore domain authority -> interpreters
```

OpenAI supplies inference only. Tool requests are untrusted proposals. Tiber
owns every identity, policy decision, effect, fact, receipt, retry,
reconciliation, and terminal workflow outcome.

## Component model

- **Terminal presentation:** the reviewed, pinned Codex executable connects to
  Tiber over a private Unix-socket WebSocket endpoint. Tiber forwards
  presentation traffic without re-encoding it, so Codex owns the pixels,
  keyboard behavior, composer, and history rather than a look-alike fork.
- **Codex gateway:** terminates both protocol connections, rewrites effective
  inference policy, intercepts every effect-bearing server request, and returns
  only application-created bounded completions. The native Codex client and
  app-server never form a direct authority path. User turns are suspended until
  the prompt/workflow request is signed; terminal presentation is suspended
  until the exact correlated observation or interruption is signed and the
  workflow advances. Restart closes an admitted but unresolved turn without
  redispatching it. Native dynamic tools reuse the existing task, repository,
  and configured-process boundaries: a bounded non-shell read returns one
  exact UTF-8 regular-file preimage without minting authority, repository
  proposals remain inert until a later exact owner `approve` or `deny` turn,
  and configured commands resolve only semantic IDs from trusted repository
  configuration.
  Closing the native client cancels and reaps an active configured process
  before the gateway runtime is released.
- **Application core:** explicit state machines for conversations,
  assignments, effects, verification, delivery, recovery, and cancellation.
- **EventCore domains:** authoritative facts for sessions, agents, tasks,
  workflow, integrations, mutations, verification, delivery, and CI recovery.
- **Scheduler and context builder:** owns typed identities, leases, budgets,
  provenance, trust labels, authoritative context construction, the bounded
  observation policy, and no-progress termination.
- **Ports:** `InferenceGateway`, `MemoryBackend`, `TaskService`,
  `WorkflowService`, `ExternalToolService`, `RepositoryService`,
  `ProcessService`, and future `VerificationService` and `DeliveryService`
  ports.
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

Repository mutation is the first connected non-inference vertical slice. A
structured app-server request is parsed once, then Tiber rereads the selected
root-relative file and publishes only a content-free safe proposal identity.
Command-specific `tiber-repository-service` models own proposal/reproposal,
owner decision, preparation, terminal outcome, and reconciliation facts on the
signed authority branch. Verified `Proposed -> Approved -> Prepared` history is
required before the core can mint opaque adapter authority. The shell executes
that authority only through the fixed Bubblewrap repository worker. Stale
preimages require a new signed proposal and approval; signed `Prepared` without
a terminal fact permits one read-only reconciliation and never redispatch.

Configured process execution is the second connected non-inference vertical
slice. The trusted repository-owned `.tiber/commands.toml` is parsed once when
the TUI starts and maps semantic command IDs to fixed absolute executables,
literal argv, repository-relative working directories, cleared fixed
environments, deadlines, and output bounds; network is always denied. The
model requests only `run_configured_command` plus a command ID and never sees
or supplies that execution plan. Each app-server invocation identity derives a
distinct process stream under the active durable effect.

Command-specific checked EventCore models own atomic `Requested`/`Prepared` or
content-free `Refused` publication and the `Completed`, `SpawnFailed`,
`TimedOut`, `Cancelled`, `Unknown`, and `Reconciled` lifecycle. Verified
requested/prepared history and unchanged configuration are required before the
core can mint opaque process authority. The Linux adapter executes it through
a fixed Bubblewrap profile and a package-private direct-argv launcher. Raw
bounded stdout and stderr are returned only as an ephemeral, sanitized tool
result; durable signed facts and the private journal retain byte counts and
digests, not output content. Retained preparation is never redispatched. At TUI
startup the CLI automatically records `Unknown`, consumes the one-shot
read-only reconciliation capability through the Linux adapter, publishes
`Reconciled`, and projects `completed`, `not-completed`, or `still-unknown` to
the public session. Once every process stream for the active effect is closed
or reconciled, the CLI records a sanitized inference interruption, advances
the enclosing workflow, and exposes its successor so a new prompt can proceed
without replaying the interrupted inference. There is no explicit owner
recovery command. The pass scans verified stream identities, then fails closed
above 64 matching process streams for the active effect or four events in any
selected process stream; unrelated historical streams do not consume that
effect-scoped budget.

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
The TUI renders those typed events and emits only typed composer intents. A
cancellable inference worker keeps terminal input responsive during turn
startup and streaming. The CLI restores its projection from signed session
facts and interprets only a workflow-owned durable inference request.
The app-server remains a transport-only boundary: it is not an
independent authority, and its tool requests remain inert structured data. The
CLI runner stages the prompt plus workflow initialization/effect request in its
private disposable EventCore store, then publishes one signed Git candidate as
the atomic authority change before dispatch. Intermediate `append_events`
results are never repository authority. Observation and workflow receipt are
staged the same way and become authoritative through one signed candidate;
the deterministic terminal advance is a later exact-revision publication made
before presenting completion.

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
EventCore command authority. The write surface remains deliberately closed to
named commands: `tiber tasks create [--id <stable-prefix>] <title>`,
`tiber tasks start <ref>`, `tiber tasks acceptance check <ref>
<one-based-index>`, `tiber tasks subtask check <ref> <one-based-occurrence>`,
and `tiber tasks transition <ref> done`. Each is a command-specific pure fold
that consumes only an opaque modeled
publication token at the signed Git adapter; no adapter exposes a generic
append. Creation folds only current task identities and the latest strict board
order, emits one backlog task plus its appended order on the board stream, and
uses a stable caller-visible prefix to reconcile ambiguity without duplication.
`start` can activate only the current eligible next task when no other
task is active; an exact retry of that sole active task is a no-op. It is a
bounded activation operation rather than generic lifecycle mutation or a
scheduler. The occurrence check carries the exact current subtask at its
immutable position, so duplicate legacy IDs cannot redirect it. The transition
grammar accepts only `done`, therefore no arbitrary lifecycle transition enters
the native surface. When retained lifecycle state is already `Done` but strict
board order still names the task, the command publishes only the closed order
repair and never re-emits a transition. Every publication declares only the
board and addressed task stream as its consistency boundary. Publication
Broader workflow scheduling remains a subsequent vertical slice. Durable
interactive session binding and the closed app-server/CLI/TUI inference runner
are implemented; an uncertain dispatched effect is exposed as `reconcile` and
is never automatically replayed. Internal actions never call legacy MCP or shell
back into the `tiber` executable.

The same closed publication boundary admits one exceptional history-repair
fact: `tiber tasks subtask repair-duplicate <ref> <occurrence> <replacement-id>`.
It is not generic subtask mutation. Its pure decision captures the exact
one-based occurrence, complete current subtask preimage, replacement identity,
and board/task consistency boundary, then publishes only a named
`TaskSubtaskIdCorrected` fact. Replay verifies that preimage and changes only
the selected occurrence, preserving all historical bytes and leaving any
prerequisite references intact.

## Assignment-bound repository mutation

`tiber-repository-core` is a pure, unconnected authority boundary for narrow
repository file mutations made within one assignment. An opaque authorization
permits only a write with either an absent-file or exact-digest precondition, or
a delete with an exact-digest precondition. The core models typed mutation
receipts and failures plus a read-only reconciliation handle; it performs no
filesystem, Git, process, or network I/O.

Authorization requires complete workflow provenance, repository identity, and
component-aware assignment scope to agree with the trusted mutation policy and
an opaque `RepositoryMutationApproval` bound to that exact safe proposal
identity and policy/assignment context. A raw proposal cannot reach a repository
adapter.

An unknown mutation outcome must be reconciled by its stable mutation identity
before a later layer can decide what to do next. It is never auto-replayed.
The boundary is not a generic filesystem or shell runner, and it does not
generalize `tiber-store-git`: that adapter remains the narrowly scoped signed
publisher for the fixed `tiber` EventCore authority branch.

S2 adds `tiber-repository-linux`, the Linux-only imperative
`RepositoryService` adapter. It interprets only opaque bounded authorizations
and reconciliation values through a fixed, private
`tiber-repository-worker` under Bubblewrap. Neither the model nor a caller can
supply shell text, arbitrary argv, cwd, environment, mount, or network
configuration. The adapter owns bounded operational timeout, cancellation,
child cleanup, and typed non-durable outcomes; it adds no workflow
`TiberEffect`, EventCore fact, CLI, TUI, scheduler, runner, or generic
`ProcessService` integration. S3 adds the private recovery and package boundary
described below without changing that integration scope.

## Third-party MCP

`tiber-external-tools-core` is the pure authority boundary for configured
third-party MCP integrations. A capability must pass the global, workflow-mode,
agent-role, session, assignment, and effect-policy grants, all bound to the
configured `IntegrationId`, before it mints an opaque authorization. The named
operations are configured tool list/call, Tiber-owned root declaration,
optional resource list/read, and optional prompt list/get. Roots remain hidden
from ordinary tokens and can be disclosed only by the dedicated root
authorization. Descriptions, schemas, server notifications, and resource or
prompt outputs are bounded untrusted payloads; they never grant authority.

`tiber-rmcp-client` is the imperative adapter pinned to RMCP 3.1.2. It admits
only bounded absolute direct-argv stdio and loopback Streamable HTTP sessions,
with capability negotiation, cancellation, tool/resource/prompt operations,
roots, and bounded tool/resource/prompt/progress/log/change observations. Its
HTTP client uses no proxy, redirects, automatic retry, automatic
reinitialization, or SSE resume. Resource templates, subscriptions, cache
directives, and input-required continuations are refused. Sampling,
elicitation, and MCP tasks are explicit refusals.

For bounded safety, the adapter rejects a negotiated protocol version at or
above `ProtocolVersion::STANDARD_HEADERS` (`2026-07-28`) before an operational
request. RMCP 3.1.2 standard-header mode retains tool schemas without a Tiber
bound while constructing later request headers. This is a deliberate
compatibility ceiling pending an upstream-safe bounded path, not a retry or a
generic protocol change.

Mutating calls require stable idempotency; an unknown result enters the
configured read-only reconciliation operation rather than an automatic replay.
This S1 boundary is not connected to workflow `TiberEffect`, EventCore, CLI,
TUI, app-server, scheduler, or runner code, and no live external-service
validation is claimed. The S3 audit-fact boundary is pure and does not change
that execution boundary.

## Memory

`tiber-memory-core` defines a swappable `MemoryBackend` port. The first
adapter, `tiber-hindsight-http`, contains private DTOs for the schema-verified
Hindsight HTTP API 0.8.3 and 0.8.4 contracts and
supports only asynchronous retain, operation status, cancellation, forget,
recall, and named read-only reconciliation. Tiber connects only to an explicit
endpoint; it never installs or
globally configures Hindsight, retries a request, manages Hindsight
authentication, or claims generic or deployment-service validation.

Memory operations carry strict owner and repository provenance. Banks are
owner-global or repository-scoped; typed tags include repository, agent,
session, task, and memory kind. Backend document and operation handles are
stable and scope-bound. An ambiguous mutation supplies a read-only
reconciliation handle rather than a replay. Retain requests name their source
turn, and recall requests never admit that same turn. Recall is
advisory,
untrusted, provenance-carrying, and bounded by item and token budgets. It
cannot grant authority. Failure is visible and nonfatal unless a future
workflow explicitly requires memory. This boundary is not connected to
EventCore, workflow execution, CLI, TUI, app-server, or scheduler.

## Audit facts and integration evidence

`tiber-integration-audit` is a functional-core boundary that constructs
provider-neutral, serializable audit DTOs. It records trusted provenance,
stable policy and operation outcomes, reconciliation identities, and bounded
evidence. It never retains raw memory text, recall queries, recalled content,
tool arguments, integration/transport configuration, or server payloads. An
observed external payload becomes only a byte count and domain-separated digest.
The facts are not EventCore publications or durable receipts yet and grant no
workflow, scheduler, CLI, TUI, app-server, or runner authority.

Deterministic fake-server coverage crosses both adapters: a policy denial
performs no server I/O, and tool observation, ambiguity, reconciliation,
scoped memory lifecycle, and hostile inputs yield only sanitized facts. The
Hindsight adapter also carries an ignored, explicit-only live check. It runs
only with `TIBER_RUN_LIVE_HINDSIGHT=1` plus a nonempty
`TIBER_HINDSIGHT_ENDPOINT`, uses a nonce-isolated synthetic lifecycle, and
forgets its exact document during cleanup. Default CI is network-free; the
existence of this check is not evidence of a deployed service run.

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

Linux-specific filesystem, process, and network controls sit behind the
`tiber-repository-linux` platform adapter. S2 interprets only bounded
`tiber-repository-core` authorizations through its fixed, private
`tiber-repository-worker` under Bubblewrap, rather than exposing a generic
filesystem, shell, or process executor. The adapter constructs the worker argv
and isolation configuration from parsed trusted configuration and opaque
authorization; callers supply neither command nor execution configuration.
It enforces resource bounds, timeout, cancellation, and child cleanup, and
returns typed non-durable outcomes. The v1 implementation targets only x86_64
Linux. S3 adds durable receipt facts and recovery evidence, but does not claim
durable working-tree filesystem state beyond those journal facts.

Configured commands use a separate `tiber-process-linux` adapter. It accepts
only opaque authority derived from signed process history and the startup
catalog, clears the environment, and constructs a fixed network-denied
Bubblewrap invocation around a private direct-argv launcher. The launcher
handshake distinguishes a definitive pre-launch failure from an outcome that
became uncertain after launch. Its private full-fsync journal is operational
evidence, not business authority, and contains no raw stdout or stderr. The
package installs the launcher and pinned Bubblewrap helper under `libexec`; it
does not expose a second public command.

## Recovery, verification, and delivery

The native development workflow treats one observed public-boundary RED as the
only implementation authority for a product-behavior increment. Durable facts
bind scenario identity, declared behavioral scope, command/evidence, and the
exact expected failure before repository mutation or process execution can be
authorized. RED evidence is either the predicted public runtime assertion or
the predicted compiler diagnostic for an intentionally missing
type/API/trait/case; incidental compilation failures grant no authority. GREEN
references that same scenario and proves the specific
failure resolved; it does not authorize unrelated production behavior. A
source delta outside the active scenario's behavioral scope fails closed before
the workflow advances. When an outer BDD failure has multiple plausible causes,
the workflow records a drill-down chain of progressively narrower behavioral
RED evidence and withholds mutation authority until one leaf failure has a
single predicted cause. It authorizes only that leaf repair, then requires
evidence to replay outward through the chain. Every generated production delta
then receives a mandatory independent fresh-context exact-failure-conformance
review against the durable RED evidence, drill-down chain, declared scope, and
complete source delta. A non-clean result blocks GREEN, the next RED,
verification, final review, and delivery. Each green increment requires a fresh-context
lightweight review before another RED may begin. Explicit typed exemptions
cover simple development-environment scripts, CI workflows, covered refactors,
and removals without inventing a tautological committed-text test.

Partial or unknown mutation results are reconciled by identity before retry.
Checkpoints make crash and restart resumption explicit. Verification and review
gates consume exact-revision evidence. Delivery state machines own commit,
push, pull-request, CI observation, and the single fenced CI-recovery incident.
Remote writes are idempotent where possible and otherwise enter typed
reconciliation.

For repository mutation S3, `tiber-repository-linux` keeps a private, pinned
`eventcore-fs` receipt journal outside the repository in an owner-only state
root, with full file and directory fsync. It records `Prepared` before the
private worker receives mutation bytes, then durable terminal `Applied`,
`Failed`, or `Unknown` facts. Its restart scan projects only read-only
ambiguity-derived reconciliation handles; it never recreates mutation authority
or auto-replays a worker request.

The journal validates corrupt, dangling, forked, and stale state fail-closed.
The adapter takes its cooperative state-root lease before the worker can take
the repository-root lock, preserving a single lock order for concurrent owners.
The x86_64 Linux package exposes public `tiber` and keeps the worker plus
Bubblewrap helper private under `libexec`. CI's package smoke covers that
artifact layout and entry behavior only; real adapter behavior remains covered
by separate integration tests.

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
