# Tiber architecture

This is the cumulative normative architecture derived from the active ADRs.
It describes how new and revised code must be implemented and may intentionally
lead the current implementation. Existing divergence is corrected when that
code is otherwise changed.

## System boundary

Tiber is the npm package `@jwilger/tiber` loaded by an unmodified stock Pi. Its
TypeScript runs inside Pi's Node.js process and packages extensions, skills,
prompts, declarative workflows, and themes. Tiber has no launcher, daemon,
native binary, Pi fork, or MCP bridge.

The visible Pi conversation coordinates work. Isolated in-process Pi agent
sessions perform bounded planning, implementation, and review assignments.
Models are untrusted semantic collaborators: they may request effects and
classify evidence but never execute effects, grant authority, advance workflow
state, or approve exceptions.

Tiber governs requested effects. It does not claim that authorized project code
is sandboxed. Strong containment is externally provisioned and attested.

## Functional core and imperative shell

The core consists of pure command decisions over immutable semantic facts. A
command folds only the facts required for that decision and returns accepted
events and a closed list of effects, a stable typed denial, or a blocker with
compliant recovery alternatives.

The core cannot read time, generate identifiers, perform I/O, invoke models,
mutate processes, or inspect ambient state. Application services collect facts,
call decisions, persist intent, ask adapters to interpret effects, validate
observations, and record receipts.

The closed effect algebra includes exact Git reads and task publication,
bounded repository reads and writes, named executable/argv invocation, isolated
model assignments, content-addressed artifact access, configured HTTP queries,
and Pi UI updates. There is no generic shell, callback, import, or arbitrary
effect node.

Every consequential effect follows:

```text
durable intent -> attempted effect -> observation -> validated receipt
```

Startup and retry reconcile unresolved intents. They do not assume success or
blindly repeat non-idempotent work.

## Semantic boundaries and failures

All external representations remain `unknown` until parsed once into semantic
types. Important types include canonical repository paths, Git object IDs,
repository and task IDs, signer identities, workflow and specification digests,
containment levels, model routes, budgets, secret references, and exact
revisions.

Expected failure is typed and carries a stable code, safe context, causes,
retryability, required recovery evidence, and redaction classification.
Malformed configuration, model output, Git output, process output, HTTP data,
or persisted state never becomes partial authority.

## Ports and adapters

Authority domains remain separate behind these ports:

- `PiHost`
- `GitRepository`
- `TaskRemote`
- `Filesystem`
- `ProcessRunner`
- `ContainmentVerifier`
- `ModelSession`
- `CiAuthority`
- `ReviewService`
- `Context7Service`
- `HindsightService`
- `Clock`
- `IdentifierSource`
- `SecretResolver`

GitHub may implement GitHub-specific CI and review adapters, but its credentials,
permissions, failures, and receipts remain separate from Git transport.

## State and persistence

### Shared task authority

The remote branch `refs/heads/tiber/tasks/v1` contains signed append-only,
versioned task event batches. It is authoritative for tasks, specifications,
dependencies, Ready ordering, claims, blockers, amendments, review and
verification evidence, delivery and CI receipts, and completion.

Tiber verifies configured signer identities, schemas, event invariants, and
ancestry before projection. Publication creates a signed child commit of the
exact observed remote head and uses a normal fast-forward push. Concurrent
failure triggers fetch and command re-evaluation. Tiber never force-pushes the
task ref. Invalid signatures, malformed events, or rewritten history degrade
the board to read-only at the last verified head.

### Repository-shared declarations

Tracked files may declare versioned data-only workflows, named commands, test
mappings, and narrower project policy. They are untrusted input and cannot grant
more authority than user-local settings and the policy floor already permit.

### Local private state

Pi's agent directory stores global settings, repository trust profiles,
project-local settings, run journals, effect intents and receipts,
content-addressed artifacts, worktree and process registry, heartbeats, budget
usage, and diagnostics. A generated repository identity stored in the Git
common directory is bound to its canonical location and expected remotes.

Node's built-in SQLite may store local structured journals after its stock
runtime contract is verified. Artifact bodies are files addressed by digest and
protected by restrictive permissions and quotas. Session entries contain
bounded status and pointers, not primary authority.

## Configuration and trust

Effective settings resolve project-local explicit, user-global explicit, and
built-in default, then apply restrictive global ceiling locks and the immutable
Tiber floor. Repository declarations can only narrow authority.

`/tiber:settings` exposes Built-in, User global, and Project columns, the
effective value, and its source. Empty project text values mean inheritance.
Global settings can forbid broader project overrides; unlocking requires an
explicit human confirmation and conflict preview.

`/tiber-setup` is the ordinary setup and reconfiguration entry point. Its
conversational agent inspects a host-generated closed catalog, explains and
recommends each supported choice, gathers explicit user intent, and proposes a
typed setup plan. A deterministic `tiber_setup` host validates the complete
plan and requires interactive confirmation before settings, authority, or
repository declarations change. Model output never grants command authority,
loosens a ceiling, selects secret material, or writes arbitrary paths. Setup
reports externally provisioned containment, signing, CI, review, context, and
memory prerequisites as blockers or optional capabilities rather than
fabricating their evidence.

Settings contain references to externally provisioned secrets. Child process
environments are scrubbed by default. Tightening applies immediately. Loosening
applies to a new run or an explicitly rebound existing run.

Tiber is the only executable extension trusted by default in governed mode. An
observed unallowlisted executable extension or incomplete inventory causes
read-only lockdown. If Tiber is disabled or a malicious peer can act outside
observable Pi boundaries, Tiber cannot claim governance.

## Task model

Tasks move through `Backlog -> Ready -> In Progress -> Done`. Blocked is an
orthogonal badge and filter. Done requires canonical acceptance, configured
delivery, exact-revision required CI, human criteria, claim release, and
verified cleanup.

Canonical structured Gherkin, rendered canonical text, typed acceptance
criteria, exclusions, dependencies, and verification mappings live in task
events. Repository feature files are deterministic projections and must remain
semantically equivalent.

Readiness requires a clean fresh-context review of outcome, scenarios, edge
cases, exclusions, dependencies, mappings, and architecture implications.
Material amendments create a new user-approved version. After claim,
independent revalidation freezes the specification against the current
baseline and active architecture. Material baseline changes invalidate that
receipt.

A task has one exclusive remotely published claim. Heartbeats may establish
staleness but never transfer ownership. Release, completion, or an explicit
audited human takeover changes ownership. Offline continuation is permitted
only after remote claim publication; delivery requires remote revalidation.

Ready ordering is shared and deterministic. Scheduling removes tasks with
unsatisfied dependencies or active claims before selecting the highest-ranked
eligible task. Agent discoveries may create provenance-bearing untriaged
Backlog tasks but cannot promote, rank, or claim them without user action or an
explicit deterministic policy.

## Workflow definitions and execution

Workflow definitions are versioned JSON compiled into immutable canonical IR.
Compilation parses schemas and references, validates bounds and reachability,
checks the policy floor, canonicalizes data, and calculates a SHA-256 digest.
Definitions cannot contain executable code, imports, callbacks, shell text,
arbitrary tools, or arbitrary network endpoints.

An active run pins workflow digest, task specification version, baseline
revision, effective policy digest, containment receipt, model routes, and
budgets. Material changes invalidate affected receipts instead of mutating the
run silently.

The immutable floor requires a remote claim before mutation, clean readiness
and start reviews, semantically valid RED before production mutation, observed
GREEN, green-only refactoring, lightweight review per increment, scope-complete
verification, three consecutive complete clean final reviews, exact-revision
delivery and CI, all required CI authorities, resolved claims and worktrees,
and human-only exact exceptions.

The default flow is intake, specification, readiness review, claim,
revalidation, worktree, vertical RED/GREEN increments, lightweight review and
refactoring, increment preservation, full verification, risk-selected final
review, delivery, exact-revision CI, claim release, cleanup, and Done.

Pre-mutation blockers release the claim, return the task to the same Ready rank,
and permit independent campaign work. Post-mutation blockers retain claim and
worktree while other eligible work may proceed. A repository-wide CI hold
blocks further delivery until causally resolved.

Campaign authority is bounded simultaneously by total tasks, tasks per
initiative, elapsed time, cost, tokens, and concurrency. The shell replaces
proposed ranks and blocker phases with signed-board order, claim state, and run
journal evidence; initiative and estimate bindings are immutable after first
checkpoint. Candidate ordering is deterministic by shared rank and task
identity. Scheduling returns closed
start, release-and-defer, or retain-work requests; it does not itself grant
workflow effects. Every decision and accumulated consumption is atomically
checkpointed in repository-scoped private state before requests are exposed.
Blockers create durable non-modal attention rather than stopping unrelated
eligible work. Ad-hoc goals create provenance-bearing Backlog tasks, never
implicitly Ready tasks. Session shutdown records an explicit checkpoint before
owned-process termination so restart resumes from observed consumption rather
than replaying work blindly.

## Model sessions and context

The visible session coordinates and remains steerable. Ordinary user intent is
translated by Pi into a typed `tiber_workflow_request`; users do not need to
issue transition commands. The request is untrusted semantic input. The host
injects current signed workflow state as dynamic suffix context, parses each
request once, and advances only transitions already authorized by deterministic
state and evidence. Slash commands remain optional diagnostics and recovery
surfaces. Bootstrap always leaves this effect-request path reachable, and human
interaction is reserved for explicit trust, takeover, exception, authority
loosening, and release-publication boundaries.

Each worker is an
isolated in-process Pi agent session with typed assignment input and completion
output, one bounded initial context pack, a role-specific immutable prompt,
fixed tool schemas, and hard token, cost, time, concurrency, and effect budgets.
Missing model routes block instead of silently substituting.

Within a cache epoch, prompt, initial context, tool schemas, and ordering are
byte-stable. Dynamic state is append-only suffix content. Context segments have
closed authority, verification, goal, working, or optional priority. Hard token
and byte planning may omit only non-mandatory lower priorities; authority or
verification overflow blocks. Pi's configured native compaction reserve keeps
capacity for completion and tool results.

Compaction starts a new explicit digest-identified epoch. Tiber privately
preserves the complete serialized source as a content-addressed artifact,
bounds the summarizer input and output, disables cache retention for the
one-off request, and binds the new epoch to the prior epoch, source digest,
advisory-summary digest, and first retained entry. Summaries are advisory,
grant no authority or verification, and retain provenance to original
artifacts. Missing routes, empty output, cancellation, or persistence failure
fails closed rather than falling back to unproven context.

Oversized Tiber-controlled results are stored as content-addressed local
artifacts. Models receive bounded previews and searchable/range-readable
handles. This replaces arbitrary context-mode execution with closed effects.

## Processes and containment

A named command has an executable, argv vector, canonical cwd, scrubbed
environment, timeout, output limits, and local grant. Tiber does not accept
model-authored shell strings, interpolation, pipes, redirects, substitutions,
or executable paths. It owns and terminates process groups it starts.

Containment assurance levels are `host-trusted`, `workspace-isolated`,
`workspace-and-network-isolated`, and `hermetic`. Strong levels require an
external attestation and local Linux corroboration. Tiber defines and verifies
the protocol but does not provision isolation. Unsupported platforms fail
closed when strong assurance is required.

Failure enters persistent configuration-only lockdown by default. An optional
policy requests graceful Pi shutdown. Stock Pi must prove that startup abort
prevents provider dispatch; otherwise pre-inference refusal is unsupported and
release is blocked. The explicit human-invoked guided-setup conversation is the
only bounded exception: while it is active, Pi exposes only governed repository
reads and the typed, independently confirmed `tiber_setup` host. Every other
tool remains denied, setup proposals grant no authority, and the ordinary
inventory is restored and containment re-evaluated immediately after apply.

## Worktrees and recovery

Mutating tasks use dedicated owned branches and worktrees by default. A durable
registry reconciles Git metadata, canonical paths, claims, heartbeats, process
groups, and quotas. Foreign, ambiguous, or actively claimed paths are never
deleted.

Before abandoned uncommitted source is removed, Tiber stores it under a bounded
private local recovery ref. Generated and ignored content may be discarded.
Recovery refs are not pushed automatically. Shutdown terminates owned process
groups, checkpoints state, and retains active claims/worktrees for resume; no
daemon continues work.

## Human exceptions

Ordinary denials provide typed private recovery feedback and compliant
alternatives. Escalation occurs only when the stated goal is blocked and no
compliant route remains. An independent review establishes necessity before one
deduplicated attention item reaches the user.

Approval freezes an operation and binds it to task, run, exact revision, paths,
preimages, executable, arguments, environment, working directory, output and
time bounds, state digest, expiry, and one use. A tool-free independent reviewer
must first establish that the stated goal is blocked and no compliant route
remains; escalation is deduplicated by the complete claim digest. Human approval
is interactive, lasts at most fifteen minutes, and never exposes capability
material to model context. Consumption is atomically audited before Tiber
executes the frozen shell-free operation directly, followed by a digest-bearing
observation receipt. Replay, near matches, drift, future use, expiry, corrupt
audit state, or concurrent double consumption fail. The model cannot create,
approve, see, or reuse a capability.

## External context services

Context7 is a first-party direct typed HTTP adapter exposing `resolve_library`
and `query_docs` with library, version, source-digest, and cache provenance. It
requires explicit `TIBER_CONTEXT7_NETWORK=enabled` authority, accepts only the
exact HTTPS Context7 v2 endpoint (or an exact loopback endpoint for local
service testing), refuses redirects, and applies hard request, timeout, response,
and result-count bounds. Responses are parsed once and malformed or oversized
payloads fail closed. Oversized documentation is content-addressed and returned
through the ordinary bounded artifact preview/range/search surface. The optional
API key is used only as an HTTP credential and never returned to model context.

Hindsight is an optional first-party direct HTTP adapter using separate hashed
user-global, hashed private-repository, and explicitly named opt-in
shared-project banks. Every bank has independent recall and retain permissions;
no configured endpoint means no memory effect. A session receives at most one
bounded low-budget initial recall, while later recall requires the explicit
`tiber_memory_recall` tool. Recalled facts are bounded, treated as untrusted
context, and never grant workflow authority. Selected private/global checkpoints
may be retained through `tiber_memory_checkpoint`; shared retention is not model
requestable and occurs only after the host has published reviewed completion.
Raw output, source, diffs, and detected credentials are denied before direct,
time-bounded HTTP. Endpoint restrictions and bounded responses fail closed.

## Delivery, CI, and release

Git delivery, CI observation, and review service are independent. Git delivery
supports local-only, branch-push, direct, and review modes. It commits only the
exact reviewed source snapshot with a signed Conventional Commit and non-empty
body, never force-pushes, and records the exact commit, tree, destination, and
observed remote revision in its receipt. Every required CI authority must report
terminal success for the exact delivered revision.
Generic CI commands are user-local, digest-pinned executable/argv templates
returning validated JSON; mutable repository scripts cannot assert remote CI
success. Tiber executes a private copy of the exact bytes whose SHA-256 digest
was granted, without a shell, and requires closed observations naming both the
configured authority and requested full commit revision. Pending observations
remain incomplete. Terminal failure creates a Git-common-directory hold visible
to every worktree; only a recorded causal diagnosis followed by terminal success
for the exact failed revision releases that hold. CI success is also published
as a signed task event and never inferred from a Git delivery receipt.

The generic review-service port separates pull-request creation, review
observation, CI observation, and merge authority. GitHub implements each as a
thin direct HTTP adapter with a distinct credential capability. Review receipts
bind the PR, delivered head revision, current approvals, complete resolved
conversation set, exact-SHA checks, author permission, auto-merge disposition,
and observed merge state.

The Tiber repository requires PRs, linear squash history, Conventional
Commit-compatible titles, resolved conversations, and an aggregate full-CI
check. An authorized ordinary PR author may enable squash auto-merge only after
all exact gates; missing permission leaves the PR open. Release-please branch or
title identity deterministically classifies a release PR, which Tiber never
auto-merges. Human merge is the publication boundary. The release-please job
creates the tag and GitHub Release, then invokes a separate least-privilege
reusable publication workflow from its validated `release_created` and
`tag_name` outputs. Publication does not depend on a release event emitted by
the workflow token, because GitHub suppresses recursive workflow triggers for
those events. An explicit exact-tag dispatch is retained only for audited
recovery. Stable package contents
are compiled JavaScript plus licenses, user documentation, prompts, skills, and
default workflow—never TypeScript source, tests, source maps, declarations,
legacy native artifacts, or install-time native builds. Tiber 1.x supports
Node 22.23.1 through Node 22 and stock Pi 0.84.2 through the pre-1.0 line.
Marketplace submission names the exact immutable npm release and occurs only
after clean stock-Pi installation verifies that publication.

Local hooks remain fast: formatting, strict lint, incremental type checking,
fast unit tests, and commit-message validation. There is no heavy pre-push
hook. Full acceptance, integration, recovery, package, and mutation
verification runs in ordinary Node/npm CI without Nix.
