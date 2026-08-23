# Stock Pi TypeScript replacement plan

Status: Accepted

## Objective

Replace the non-working Rust/Codex product with `@jwilger/tiber`, a public,
dual-licensed (`MIT OR Apache-2.0`) package for unmodified stock Pi. Tiber runs
entirely in Pi's Node.js process and provides shared task tracking,
deterministic workflow and effect guardrails, bounded autonomous development,
and first-party equivalents for context-mode, Headroom, Context7, and
Hindsight integration.

This is a hard cutover. There is no compatibility layer or data migration for
the existing Rust implementation, task data, schemas, binaries, or Codex
configuration.

## Product boundaries

Tiber ships one Pi package containing extensions, skills, prompts, workflows,
and themes. It does not ship or require a launcher, daemon, native Tiber binary,
Pi fork, MCP bridge, or third-party executable extension. Runtime functionality
uses Pi peer APIs and Node built-ins. Git and configured project tools remain
ordinary executables invoked through governed adapters.

Tiber governs requested effects but does not claim to sandbox authorized
project code. Strong process containment is externally provisioned and
attested. Linux is the first platform on which strong containment can be
verified.

## Repository reset

Remove the Cargo workspace, Rust crates, old reference implementation,
third-party Codex/Ratatui sources, Codex configuration and updater machinery,
obsolete spikes, generated evaluation output, legacy release machinery, and
obsolete Rust-, Codex-, EventCore-, and Clippy-specific documentation. Git
history remains the archive.

Rewrite `README.md`, `PRD.md`, `ARCHITECTURE.md`, `AGENTS.md`, `CLAUDE.md`, CI,
hooks, and local development configuration for the TypeScript product. Keep a
minimal local-only `flake.nix`; CI uses ordinary pinned Node/npm tooling and
never Nix. Remove obsolete Development System plugin configuration because the
plugin is not assumed to be loaded by Pi.

Replace the existing license presentation with `LICENSE-MIT`,
`LICENSE-APACHE`, a short root `LICENSE`, and npm metadata using the SPDX
expression `MIT OR Apache-2.0`. Remove the old `NOTICE` unless the replacement
actually incorporates material requiring notice.

## Documentation authority

ADRs are the authoritative decision history. `ARCHITECTURE.md` is the
cumulative normative architecture derived from active ADRs. It describes the
desired implementation of new and revised code and may intentionally lead the
current implementation. Existing code that is out of conformance is brought
into conformance when it is otherwise changed.

Initial ADRs cover:

1. Hard replacement with a stock-Pi strict-TypeScript package.
2. Functional core, closed effects, and deterministic workflow IR.
3. Layered local authority, project trust, ceiling locks, and human exceptions.
4. Signed shared task authority on a dedicated Git ref.
5. In-process coordinator/worker sessions and cache-preserving context.
6. External containment attestation and Linux-first verification.
7. Worktree, process, recovery, and cleanup ownership.
8. Separate Git, CI, and review-service ports.
9. Context virtualization, Context7, and Hindsight boundaries.
10. PR-required development and release-PR publication.

Adapt the necessary Development System guidance into repository-local rules for
preflight, functional-core design, semantic types and errors, BDD/TDD,
verification, review, delivery, CI recovery, worktree hygiene, and agentic
systems. `AGENTS.md` is a concise index pointing only to local files. Record the
source paths and pinned source revision for provenance; the local copies then
evolve independently.

Do not add snapshot, parity, wording, or synchronization tests for copied
guidance. Once guidance becomes mechanically enforced by the Tiber extension,
test the resulting public behavior rather than Markdown or prompt text.

## Package and toolchain

The package name is `@jwilger/tiber`. The initial compatibility baseline is
Node.js 22.23.1, npm 10.9.8, and
`@earendil-works/pi-coding-agent` 0.84.2. Use strict TypeScript with
`noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`,
`noImplicitOverride`, and `useUnknownInCatchVariables`. Shipping code has no
`any` and no blanket lint suppressions.

Commit `package-lock.json`. Publish compiled JavaScript plus workflows, skills,
prompts, themes, licenses, and required documentation. Do not commit build
output and do not require install-time lifecycle scripts or native add-ons.
List Pi-owned packages as peers according to Pi package guidance.

## Architectural model

### Functional core and effect shell

Domain commands are immutable semantic values. Each command folds only the
facts required for its decision and returns accepted events and closed typed
effects, a stable typed denial, or a blocker with compliant recovery
alternatives. The core performs no I/O, model invocation, clock access,
identifier generation, or process mutation.

The imperative shell interprets closed effects such as reading an exact Git
fact, publishing a signed task event, accessing a bounded repository path,
running a named executable with an argv vector, requesting an isolated model
assignment, storing a content-addressed artifact, querying a configured HTTP
service, or updating Pi UI. Workflows cannot contain callbacks, imports, shell
text, arbitrary URLs, or generic effects.

Consequential effects follow durable intent, attempted effect, observation,
and validated receipt. Recovery reconciles unresolved intents rather than
assuming success or blindly replaying them.

External data is parsed once into semantic types. Failures have a stable code,
safe context, causes, retryability, recovery evidence, and redaction class.
Malformed model, configuration, adapter, or remote output never becomes partial
authority.

### Ports

Keep `PiHost`, `GitRepository`, `TaskRemote`, `Filesystem`, `ProcessRunner`,
`ContainmentVerifier`, `ModelSession`, `CiAuthority`, `ReviewService`,
`Context7Service`, `HindsightService`, `Clock`, `IdentifierSource`, and
`SecretResolver` separate. GitHub may implement multiple ports but never
collapses their credentials or permissions into one authority.

### Shared and local persistence

Use a dedicated remote branch, initially `refs/heads/tiber/tasks/v1`, for signed
append-only task event batches. It stores tasks, canonical specifications,
dependencies, Ready ordering, claims, blockers, review evidence, amendments,
delivery and CI receipts, and completion records.

Publication is a normal fast-forward push based on the exact observed remote
head. Never force-push the task ref. Verify every commit against configured
signer identities. Invalid signatures, rewritten ancestry, malformed events,
or unexpected history place the board into degraded read-only mode at the last
verified head.

Tracked project declarations may contain data-only workflows, named command
declarations, test mappings, and narrower project policy. Repository content
can narrow but never grant user-local authority.

Local private state under Pi's agent directory contains settings, trust
profiles, run journals, effect intents, artifacts, worktree registry,
heartbeats, budgets, and diagnostics. A generated repository identity is stored
in the Git common directory and bound to its canonical path and expected
remotes. Use Node's built-in SQLite only after a stock-runtime capability
contract passes; content artifacts are local content-addressed files with
restrictive permissions.

### Configuration

Resolve effective settings in this order:

1. Project-local explicit value.
2. User-global explicit value.
3. Built-in default.
4. User-global ceiling locks.
5. Immutable Tiber policy floor.

`/tiber:settings` shows Built-in, User global, and Project columns plus the
effective source. Empty project text values mean `inherit`. Global settings can
require project overrides to be no less restrictive. Unlocking requires an
explicit human confirmation and conflict preview.

Settings store references to externally provisioned secrets, never secret
values. Tightening applies immediately. Loosening applies only to a new run or
an explicit human rebind.

### Workflows

Workflow definitions are versioned JSON. Compilation performs schema parsing,
reference resolution, bounded-loop and reachability checks, policy-floor
validation, canonicalization, and SHA-256 digest calculation. Active runs pin
the workflow digest, task specification version, baseline revision, policy
digest, containment receipt, role/model routes, and budget.

The immutable policy floor requires:

- A remotely published exclusive claim before mutation.
- Clean specification readiness review.
- Fresh claimed-task baseline revalidation.
- Semantically valid RED before production mutation.
- Observed GREEN after production mutation.
- Refactoring only while green.
- Fresh lightweight review for every increment.
- Scope-complete verification.
- Three consecutive complete finding-free final-review iterations.
- Invalidation of dependent receipts on material source changes.
- Exact-revision delivery and CI evidence.
- Success from every required CI authority.
- Resolved claims and owned worktrees before Done.
- Human-only exact single-use exceptions.

The default workflow is intake, specification, readiness review, Ready, remote
claim, baseline revalidation, task worktree, vertical scenario RED/GREEN loops,
lightweight review and refactoring, increment preservation, full acceptance
verification, risk-selected final review, delivery, exact-revision CI, claim
release, cleanup, and Done.

### Coordinator and workers

The visible Pi session coordinates and receives user steering. Planning,
specification review, implementation, RED classification, and code review run
in isolated in-process Pi agent sessions, not `pi` subprocesses. Workers receive
a byte-stable role prompt, one bounded initial context pack, fixed tool schemas,
typed assignment input, typed completion output, and hard token, time, cost,
concurrency, and effect budgets. A worker may request an effect but cannot
execute or authorize one.

Within a cache epoch, prompts, initial context, tool schemas, and ordering are
byte-stable. Dynamic state is appended only as suffix messages or tool results.
Compaction deliberately creates a new epoch. Typed priorities and a configurable
reserve protect completion capacity. Summaries remain advisory and retain links
to original artifacts.

### Process and containment

Named commands consist of executable, argv, canonical cwd, scrubbed
environment, timeout, and output limits. Do not accept model-authored shell
strings, pipes, redirects, substitutions, or executable paths.

Containment levels are `host-trusted`, `workspace-isolated`,
`workspace-and-network-isolated`, and `hermetic`. Strong levels require an
external attestation plus local Linux corroboration. Tiber verifies but does
not provision isolation. Failure enters persistent configuration-only lockdown
by default; an optional stricter setting requests graceful Pi shutdown.

A stock-Pi contract test must prove that startup abort prevents provider
dispatch. Failure is a release blocker requiring upstream support, not a
wrapper or weakened guarantee. Strong governed mode also requires complete
executable-extension/tool inventory; incomplete inventory fails closed.

### Human exceptions

Ordinary denials return typed private recovery feedback and compliant
alternatives. If the goal is genuinely blocked and no compliant route remains,
Tiber records a blocker claim, obtains an independent necessity review, and
creates one deduplicated attention item. Human approval freezes the exact
operation and binds it to task, run, revision, paths, preimages, arguments, and
expiry. Tiber executes it once. Replay, near matches, drift, or expiry are
denied and every outcome is audited. The model never receives or mints a
reusable capability.

## Shared task model

Task lifecycle is `Backlog -> Ready -> In Progress -> Done`; Blocked is an
orthogonal badge and filter. Done requires satisfied canonical acceptance,
configured delivery, exact-revision required CI, human criteria, released
claim, and verified cleanup.

Canonical structured Gherkin and typed acceptance criteria live in task events.
Tiber renders deterministic repository `.feature` projections and verifies
semantic equivalence. Readiness requires a clean fresh-context review of the
outcome, scenarios, edge cases, exclusions, dependencies, test mappings, and
architecture implications. Material amendments create a new approved version.

A task has at most one remotely published claim. Stale heartbeats are advisory;
claims are never automatically stolen. Ownership changes through release,
completion, or explicit audited human takeover. Offline work may continue only
after a claim was published, and delivery requires remote revalidation.

Ready order is shared and deterministic. Selection removes tasks with
unsatisfied dependencies or active claims before choosing the highest-ranked
eligible task. Agent-discovered work may create provenance-bearing untriaged
Backlog tasks but cannot promote, prioritize, or claim them without user action
or an explicit deterministic policy.

## User surfaces

Provide `/tiber:status`, `/tiber:doctor`, `/tiber:settings`, `/tiber:tasks`,
`/tiber:task`, `/tiber:work`, `/tiber:campaign`, `/tiber:attention`,
`/tiber:containment`, and `/tiber:artifacts`. Persistent UI shows containment,
active task and stage, budgets, claim state, delivery/CI holds, and pending
human attention. Keep the model-facing tool list small and stable; the host,
not a model-callable transition tool, advances workflow state.

## Vertical delivery slices

Every slice is independently installable, user-visible, and black-box testable.
Core, adapters, UI, and documentation for a behavior ship together. Start each
feature scenario with a failing public-boundary test unless the work is only
documentation or external repository configuration.

### 1. Clean package, licensing, and doctor

Perform the repository reset; establish the TypeScript package, dual licensing,
local architecture and rules, `/tiber:doctor`, safe read-only startup, fast
hooks, full Node CI, PR-required delivery, and release-PR automation. Prove by
packing and installing into an isolated stock-Pi home and running the doctor
without a Rust binary, native add-on, install hook, or external plugin.

### 2. Layered settings UI

Deliver built-in, global, and project settings, generated repository identity,
inheritance, atomic persistence, validation, and `/tiber:settings`. Prove
inheritance, project overrides, effective-source display, and restart
persistence across two repositories.

### 3. Ceiling locks and secret references

Deliver restrictive global locks, conflict preview, explicit unlock, secret
references, and fail-closed malformed configuration. Prove that a project
cannot broaden a locked network setting while a narrower value remains valid.

### 4. Verified containment lockdown

Deliver containment levels, attestation protocol, Linux corroboration,
configuration-only lockdown, optional shutdown, and provider-dispatch veto.
Prove that missing, invalid, mismatched, or expired attestation prevents both
provider dispatch and effects while diagnostics remain available.

### 5. Governed stable tool surface

Deliver Tiber-owned stock-tool overrides, canonical path/symlink checks,
read-only inspection, extension inventory, task-bound mutation denial, and
fixed schemas. Prove out-of-root writes, symlink escapes, unapproved commands,
and unallowlisted executable tools fail before effects.

### 6. Signed shared Kanban

Deliver task-ref initialization/synchronization, signed event publication,
Backlog editing, task details, Kanban UI, and concurrent compare-and-swap. Prove
that two clones publish concurrent tasks without loss or force and an invalid
signer degrades the board to read-only. Begin dogfooding the new board after
this slice; migrate no legacy data.

### 7. Reviewed Ready specifications

Deliver structured Gherkin, acceptance criteria, exclusions, dependencies,
test mappings, shared Ready ordering, first isolated reviewer role, model
routing, basic budgets, and fresh readiness review. Prove incomplete or
adversely reviewed tasks cannot enter Ready and a complete cleanly reviewed task
can.

### 8. Workflow compilation, claim, and revalidation

Deliver data-only workflow compilation/digest, built-in workflow, restrictive
project workflows, exclusive remote claims, baseline revalidation, durable run
journal, resume, and pre-mutation blocker handling. Prove invalid workflow
rejection, claim-before-mutation, and Ready-rank preservation after a
pre-mutation blocker.

### 9. Owned worktrees and recovery

Deliver task branches/worktrees, process-group registry, restart
reconciliation, safe cleanup, private local recovery refs, quotas, and human
claim takeover. Prove interrupted source work survives restart and abandonment
creates a recovery ref without deleting foreign or ambiguous paths.

### 10. Structured commands and output virtualization

Deliver named executable/argv commands, local grants and digests, environment,
time, cwd, and output bounds, content-addressed result storage, bounded
previews, search/range tools, and artifact reaping. Prove oversized output is
virtualized rather than injected. This is the first-party context-mode
equivalent.

### 11. Semantically valid RED

Deliver deterministic feature projection, test-only authority, exact diagnostic
observation, independent RED classification, valid missing-public-surface
compile failure support, and denial of production mutation before accepted RED.
Prove unrelated failures are rejected and a scenario-specific missing API can
establish RED.

### 12. GREEN, review, and increment preservation

Deliver diagnostic-driven production micro-steps, GREEN observation, green-only
refactoring, fresh lightweight review, rework, and signed increment
preservation. Prove a scenario reaches GREEN minimally, overimplementation is
returned for rework, and a new failure revokes refactor authority.

### 13. Multi-scenario completion and final review

Deliver repeated vertical increments, full acceptance verification,
risk-selected review lenses, three consecutive clean complete iterations,
review reset on findings or deltas, local-only Done, and cleanup. Prove partial
scenario completion cannot finish and findings reset the streak. Begin
self-hosting local workflow after this slice.

### 14. Generic Git delivery

Deliver local-only, branch-push, direct, and review modes, signed Conventional
Commits with bodies, exact snapshot identity, fast-forward-only pushes, and
Git-only delivery receipts. Prove pushed receipts name the exact commit and
source drift or non-fast-forward delivery requires revalidation.

### 15. CI authorities and recovery hold

Deliver multiple required CI authorities, user-local digest-pinned executable
adapters, schema-validated observations, exact SHA matching, repository-wide
failure hold, causal diagnosis, and recovery. Prove wrong-revision success is
rejected and all required providers must succeed.

### 16. Review services and GitHub adapter

Deliver a generic review-service port and thin first-party GitHub HTTP adapters
for PR, review, CI, and merge, with separate permissions. Ordinary PRs may
auto-merge only when the author has permission and all gates pass. Tiber never
auto-merges a release PR. Prove missing permission leaves a PR open and a
release PR always waits for explicit human merge. Begin self-hosting PR delivery
after this slice.

### 17. Bounded autonomous campaigns

Deliver task, initiative, time, cost, token, and concurrency campaign bounds,
deterministic scheduling, ad-hoc goal task creation, blocker deferral,
non-modal attention, and shutdown checkpoints. Prove a task-count bound is
honored, pre-mutation blockers release and defer, and post-mutation blockers
retain their work while independent work continues.

### 18. Exact human exception capabilities

Deliver blocker claims, independent necessity review, deduplicated escalation,
exact one-use short-lived human approvals, state-bound execution, and audit.
Prove only the frozen operation executes once and all replay, near-match, drift,
and expiry cases fail.

### 19. Headroom, cache epochs, and compaction

Deliver token reserve, typed context priorities, byte-stable prefixes, hard
budgets, deliberate cache epochs, and Tiber-aware Pi compaction. Prove repeated
requests preserve cacheable prefixes, dynamic state is suffix-only, and budget
exhaustion blocks without weakening verification. This is the first-party
Headroom equivalent.

### 20. Context7 capability

Deliver first-party `resolve_library` and `query_docs` tools, bounded direct
HTTP, endpoint restrictions, provenance, cache, and virtualized results. Prove
unauthorized endpoints, malformed payloads, unavailable network authority, and
oversized responses fail safely.

### 21. Hindsight memory

Deliver an optional first-party Hindsight HTTP adapter; separate global,
private-repository, and opt-in shared-project banks; independent recall/retain
permissions; bounded initial recall; explicit later recall; private checkpoint
retention; reviewed completion-only shared retention; and secret/raw-output
exclusion. Prove bank separation and retention filters against a fake server.

### 22. Stable release and marketplace submission

Harden package contents, supported Pi/Node compatibility, upgrade/uninstall,
security and contribution documentation, and `1.0.0` readiness. Install the
exact release candidate in a clean stock-Pi environment and complete a
representative workflow without repository-local Tiber code or external
extensions. Submit to the Pi marketplace after the stable npm release.

## Testing strategy

Public-boundary acceptance tests exercise the installed package in stock Pi,
commands and TUI interactions, scripted model responses, filesystem/process
effects, bare Git remotes and multiple clones, fake CI/GitHub/Context7/Hindsight
services, and restart, crash, timeout, malformed response, and concurrency
behavior. They do not import reducers to assert internal structure.

Use deterministic unit, property, and mutation tests for policy composition,
workflow compilation, task decisions, claims, capabilities, context selection,
budgets, receipts, and recovery. Adapter contracts cover parse-once behavior,
stable failures, cancellation, bounds, redaction, exact identities, and
idempotent uncertain-outcome recovery.

Do not add tests for copied documentation, implementation layout, mirrored
constants/types, live paid providers, marketplace validation, or Nix CI.

## Local and CI verification

Lefthook's fast local commit gate runs changed-file formatting and strict lint,
incremental type checking, fast deterministic unit tests, and commit-message
validation. It rejects rather than rewrites staged content. There is no heavy
pre-push hook.

Every PR runs clean install, formatting, lint, clean type check, unit/property
tests, Gherkin acceptance tests, adapter contracts, crash/reconciliation tests,
TUI tests, stock-Pi package installation, package/dependency checks, mutation
testing, and one aggregate required status. Superseded runs may be cancelled.

## PR and release policy

Direct pushes to `main` are prohibited. Every ordinary change uses a PR,
Conventional Commit-compatible PR title, required aggregate CI, resolved
conversations, linear squash history, and no protected-branch force push or
deletion. An authorized ordinary PR author may enable auto-merge after all
gates pass.

Use release-please pinned to an immutable action revision. Conventional commits
on `main` maintain one release PR containing version, changelog, and metadata
changes. The release PR runs full CI and requires an explicit human merge;
Tiber never enables auto-merge for it. After merge, tagging and GitHub Release
creation are automatic. A separate least-privilege workflow verifies the tagged
revision and tarball and publishes `@jwilger/tiber` publicly through npm trusted
publishing with OIDC and provenance. Do not use a long-lived npm token.

Install this process in the first slice so every slice can receive a
human-approved `0.x` release. The final slice is the `1.0.0` and marketplace
milestone.

## Early release blockers

1. Pi startup abort must prevent provider dispatch.
2. Strong governed mode requires complete executable-extension/tool inventory.
3. In-process isolated sessions must support cancellation and fixed schemas.
4. Stock Node persistence must work without native add-ons.
5. Git signature verification must be deterministic for supported signing.
6. The npm tarball must run without install lifecycle scripts.
7. Strong containment must never be inferred from local heuristics alone.

Resolve a failed contract through an upstream capability, explicit ADR, or
narrower documented guarantee, never a hidden wrapper or permissive fallback.
