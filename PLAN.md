# Pi-native Tiber Migration Plan

## Goal

Replace the Claude/Codex Development System marketplace with a Pi-native development system suitable for exclusive daily use, while preserving mature Rust/EventCore domain behavior, persisted history, and behavioral guarantees.

Pi is the destination harness, not a subordinate compatibility adapter. TypeScript remains a thin Pi runtime adapter. Rust remains authoritative for material workflow, task, review, routing, persistence, and delivery policy.

## Planning rules

- Deliver independently useful, reversible vertical increments.
- Reuse existing Rust modules, skills, tests, and history instead of rewriting or copying them wholesale.
- Keep one installable Rust crate and executable unless a documented technical blocker requires process isolation.
- Preserve distinct persistence authorities, stream schemas, locks, and security boundaries inside that deployment unit.
- Never add a TypeScript fallback for Rust policy.
- Do not claim parity or complete an increment without its listed evidence.
- Do not publish crates or packages, change global configuration, destructively restructure the repository, or archive legacy adapters without explicit approval.
- Use existing Tiber ticket tracking until Pi exposes safe native Tiber tools. Once available, dogfood the Pi tools for the remaining migration.

## Current state

The initial boundary proof exists under `./`:

- Pi package manifest and project-local loading;
- minimal TypeScript extension;
- `tiber` Rust crate/binary;
- bounded JSONL protocol v1 and executable compatibility negotiation;
- local package-owned `cargo install --locked --path` flow;
- Rust-backed semantic model-role routing proof;
- Rust-decided Pi `tool_call` interception proof;
- Pi-native doctor command;
- an initial Tiber skill; mature legacy skills have not yet been copied and adapted.

This proof is not yet a completed production-quality first increment. Contract, installer fault, fresh-context, lifecycle behavior, and Pi end-to-end tests remain incomplete.

## Increment 1: Complete the runtime boundary

### Outcome

Make the package/executable boundary reliable enough to support subsequent domain migration without bypasses.

### Work

1. Define shared protocol fixtures or generated contracts consumed by Rust and TypeScript tests.
2. Complete typed request/response envelopes and stable error classification for:
   - domain rejection;
   - configuration error;
   - runtime unavailability;
   - cancellation/timeout;
   - internal failure.
3. Enforce request and response byte limits while streaming, not only after buffering.
4. Verify correlation identifiers on every response.
5. Ensure process cancellation terminates the complete process tree and cannot settle a request twice.
6. Add adapter tests proving missing, incompatible, malformed, oversized, timed-out, and failed Rust runtimes fail closed.
7. Complete installer behavior:
   - fake Cargo tests;
   - concurrent installation serialization;
   - interrupted staging cleanup;
   - incompatible executable rejection;
   - atomic activation and reuse;
   - reversible removal;
   - controlled Cargo home/config behavior;
   - local-path and future registry command construction.
8. Add deliberate upgrade semantics that do not change the runtime of an active workflow.
9. Add package compatibility diagnostics covering Pi package, executable, crate, protocol, and known persisted schema versions.
10. Add a Pi RPC/JSON behavior test that loads the package and proves both the structured tool and lifecycle interception.

### Exit criteria

- Rust unit and protocol tests pass.
- Shared Rust/TypeScript contract tests pass.
- Installer fault/concurrency tests pass without using global Cargo state.
- Vanilla Pi local-load end-to-end test passes.
- No adapter test observes a TypeScript policy fallback.
- Bootstrap, doctor, repair, upgrade, removal, and failure diagnostics are documented.

## Increment 2: Expose safe Tiber ticket tracking through Pi

### Outcome

Use Pi for normal Tiber task operations while preserving the current Git/EventCore authority and history.

### Work

1. Inventory the active component Tiber CLI/core/git/MCP/server modules, commands, stream names, schemas, locks, and callers against the execution-time checkout.
2. Define native Rust application operations for:
   - initialize/open repository task state;
   - list and inspect tickets;
   - create tickets;
   - update title/description/status;
   - manage dependencies;
   - prioritize the complete backlog with no ties;
   - select the highest-priority unblocked ticket;
   - show blockers and history.
3. Move or reuse domain modules behind the single executable without shelling or MCP-looping back into `tiber`.
4. Preserve current persisted events, Git branch behavior, signing, synchronization, and conflict semantics.
5. Expose a narrow structured Pi tool surface backed exclusively by those Rust operations.
6. Add Pi commands for readable task status and diagnostics.
7. Port selected parity tests from the existing CLI/MCP surfaces.
8. Add malformed request, stale state, synchronization conflict, and restart tests.
9. Import this plan into Tiber tickets and begin using the Pi-native task tools for remaining work.

### Exit criteria

- Existing repositories and task history open without migration loss.
- Pi can safely perform all routine backlog operations required by `AGENTS.md`.
- Rust behavior tests cover task, dependency, status, priority, and conflict rules.
- Pi tools are adapters only and cannot directly mutate Tiber persistence.
- Existing CLI callers have a documented migration path.

## Increment 3: Setup, skills, and repository diagnostics

### Outcome

A vanilla Pi installation can configure and diagnose a repository without Claude/Codex bootstrap mechanisms.

### Work

1. Migrate deterministic setup-policy decisions from Development Discipline into the unified Rust executable.
2. Discover repository boundaries, existing configuration, hooks, MCPs, worktree conventions, and conflicting development systems.
3. Provide structured setup preview/apply operations with explicit confirmation and no staging or commits.
4. Add Pi-native `setup`, `doctor`, `repair`, and resolved-configuration commands/tools.
5. Reuse public skills directly during incubation; remove harness-specific wording and mechanisms only where Pi provides a better native boundary.
6. Define deterministic packaged/user/project configuration locations and precedence.
7. Add worktree setup support using existing scripts/services rather than duplicating behavior.
8. Test untrusted projects, malformed configuration, missing Cargo, offline operation, and reversible removal.

### Exit criteria

- A vanilla Pi user can locally install, configure, diagnose, repair, and remove the system.
- Repository setup is deterministic, previewable, and Rust-authorized.
- Skill routing covers setup, engineering standards, tasks, development workflow, debugging/TDD, and delivery.
- No global Pi or Cargo setting is modified.

## Increment 4: Provider-neutral model-role routing and isolated assignments

### Outcome

Rust can authorize exact semantic work roles across Pi providers, and Pi can execute fresh-context assignments with verifiable attestation.

### Work

1. Refine the role vocabulary based on actual responsibility differences. Begin with:
   - bounded helper;
   - routine/substantive worker where materially distinct;
   - architecture/planning;
   - risk scout;
   - independent reviewer;
   - verifier;
   - debugging;
   - eval grading;
   - summarization/compaction where policy differs.
2. Implement Rust parsing and deterministic precedence for packaged, user, project, and permitted assignment overrides.
3. Support exact provider/model selection, ordered explicit fallback, disabled fallback, capability requirements, and independence constraints.
4. Represent portable preferences separately from namespaced provider-specific settings.
5. Translate Pi's provider/model catalog into credential-free canonical capability records.
6. Validate settings against Pi capabilities where possible and reject unsupported or silently discarded settings.
7. Spike and choose Pi SDK sessions versus child Pi RPC/JSON processes for isolated turns.
8. Launch exact Rust-authorized selections in fresh sessions/contexts.
9. Return and validate canonical attestations containing role, provider, model, material settings, session/context identity, fallback use, and relevant runtime capabilities.
10. Add fake catalogs shaped like OpenAI, Anthropic, Google, and local/custom providers.

### Exit criteria

- Configuration precedence and fallback tests pass.
- Exact model/provider and no-fallback requirements are enforced.
- Strong roles cannot silently downgrade.
- Independence policies are tested after canonical alias resolution.
- Credentials never enter Rust messages, logs, errors, events, or durable evidence.
- At least one isolated assignment succeeds in a Pi end-to-end test with Rust-accepted attestation.

## Increment 5: Native development workflow and restart recovery

### Outcome

Pi can coordinate one active development ticket durably across interruption and restart.

### Work

1. Inventory Development Discipline workflow domains, event streams, projections, recovery rules, and duplicated infrastructure.
2. Move/reuse pure workflow state machines and services in the unified Rust executable.
3. Preserve functional-core/imperative-shell boundaries and checked EventCore models.
4. Model assignment issuance, epochs/fencing, attempts, cancellation, timeout, malformed results, and no-progress termination.
5. Bind source snapshots and verification evidence immutably to workflow decisions.
6. Provide Pi tools/commands for starting, resuming, inspecting, and cancelling workflow work.
7. Use Pi lifecycle events to surface blockers and prevent operations that Rust rejects.
8. Add durable checkpoint and crash/restart recovery tests.
9. Integrate systematic debugging and test-driven development guidance with executable workflow state rather than prompt-only gates.

### Exit criteria

- One active ticket can proceed from start through verified source change after a process restart.
- Stale assignments/results are rejected by Rust.
- Material decisions and evidence are durable.
- Pi session state is never the sole workflow authority.

## Increment 6: Final review and verification enforcement

### Outcome

Pi enforces the existing configurable independent final-review contract, including consecutive clean reviews.

### Work

1. Consolidate/reuse `tiber-review` and Development Discipline review behavior without merging distinct persistence authorities prematurely.
2. Preserve risk assessment, selected lenses, verifier routing, assignment identities, source snapshots, and immutable evidence bindings.
3. Launch each reviewer/verifier in a fresh context with an exact Rust-authorized role.
4. Validate assignment fencing and runtime attestation before accepting results.
5. Require all selected lenses and verifiers in every iteration.
6. Reset the clean streak after findings, malformed results, stale evidence, or material source/evidence changes.
7. Enforce the configured minimum, including at least three consecutive clean iterations where configured.
8. Provide blocker/status rendering and restart recovery.
9. Add parity tests against shipped review behavior and behavior evals for attempts to bypass review.

### Exit criteria

- Rust tests prove clean-sequence, reset, provenance, completeness, and terminal-state rules.
- Fresh-context and independence tests pass across multiple fake providers/models.
- Completion remains blocked without current accepted evidence.
- Review state survives Pi and Rust process restart.

## Increment 7: Commit, push, CI recovery, and delivery

### Outcome

Pi can safely deliver verified work without bypassing Rust authority.

### Work

1. Replace the temporary blanket direct commit/push block with workflow-aware Rust authorization.
2. Reuse Git, verification, signing, push, forge, and CI incident authorities.
3. Bind commits, pushes, publications, and delivery completion to exact source and verification/review evidence.
4. Preserve the single fenced pushed-CI recovery incident and remote-data-loss protections.
5. Model idempotency and reconciliation for ambiguous remote outcomes.
6. Distinguish authentication, quota, rate limit, provider outage, model incompatibility, transport, executable, and domain-policy failures.
7. Add owner approval boundaries for publication and external posting.
8. Add restart and recovery tests through commit, push, CI failure, remediation, and final delivery.

### Exit criteria

- Pi can commit, push, observe CI, recover failures, and complete delivery for a real repository.
- Stale or mismatched evidence cannot authorize delivery.
- Interrupted/ambiguous remote operations reconcile safely.
- No force-push or publication occurs without the applicable explicit policy/approval.

## Increment 8: Agentic systems, evals, and effectiveness evidence

### Outcome

Preserve the useful agentic/eval engineering workflows and establish evidence for daily-use trust.

### Work

1. Reuse and adapt agentic-systems and eval-case-reporting skills.
2. Preserve privacy scrubbing, sanitized previews, and explicit posting approval.
3. Add Pi behavior/effectiveness evals for skill routing, task operations, lifecycle coordination, review, routing, recovery, and delivery.
4. Test provider failure and stochastic behavior with named metrics, distinct cases, and intentional repeat counts.
5. Keep live provider tests credential-gated and supplemental to deterministic fake-provider evidence.
6. Compare selected outcomes against legacy Claude/Codex behavior without preserving obsolete mechanisms.
7. Make model cost-impacting settings and fallbacks visible before multi-model work.

### Exit criteria

- Required deterministic suites pass.
- Representative Pi behavior/effectiveness evals meet documented thresholds.
- Eval-case reporting cannot post without sanitized preview and explicit approval.
- Remaining parity gaps are documented as material, obsolete, or deliberately deferred.

## Increment 9: Daily-use cutover

### Outcome

Use Pi exclusively for normal development-system work.

### Required evidence

- Reliable repository setup and skill routing from vanilla Pi.
- Safe Tiber task operations and history compatibility.
- Durable development lifecycle coordination and restart recovery.
- Required final-review enforcement and configured clean sequence.
- Fresh-context independent review and verification.
- Configurable multi-provider role routing without silent downgrade.
- Commit, push, CI recovery, and delivery gates.
- Reversible package/runtime installation, diagnostics, repair, upgrade, and removal.
- Sufficient behavior/effectiveness results for real daily use.

### Cutover actions

1. Run a representative real-work dogfooding period.
2. Record failures, recovery outcomes, and remaining gaps.
3. Decide whether each legacy-only feature is obsolete or still material.
4. Mark Pi as the supported daily-use product only when all required evidence is current.
5. Do not archive or delete legacy adapters yet.

## Increment 10: Publication and legacy archival

### Outcome

Release the standalone product from this canonical repository and retire obsolete marketplace surfaces without losing required provenance or data compatibility.

### Work

1. Verify every copied resource has recorded provenance, applicable license notices, focused tests, and no cross-repository runtime dependency.
2. Complete crate packaging metadata, lockfile inclusion, package-content verification, name availability, ownership, and release procedure.
3. Request explicit approval before the first crates.io publication.
4. Smoke-test exact crates.io installation after approved publication.
5. Prepare local/Git Pi package installation and upgrades before any npm/gallery publication.
6. Request explicit approval before publishing the Pi package.
7. Document the legacy source commits used for copied domain behavior and verify persisted-data compatibility.
8. Tag and verify the first standalone Pi-native release and its recovery path.
9. Request explicit approval before destructive restructuring, deletion, or archival.
10. Archive Claude/Codex adapters only after the standalone Pi product and recovery path are verified.

### Exit criteria

- Standalone repository/package records relevant source provenance and preserves required license notices.
- Exact crate and protocol compatibility is enforced in released installs.
- Published installation and deliberate upgrade smoke tests pass.
- Legacy archival is approved, reversible through retained history, and does not strand persisted task/workflow data.

## Cross-cutting verification checklist

Every affected increment must add proportional evidence for:

- Rust unit and behavior tests for material rules;
- persisted-event/schema compatibility and migrations;
- malformed, oversized, cancelled, and timed-out protocol traffic;
- adapter delegation and fail-closed behavior;
- installer concurrency/interruption/incompatibility;
- Pi lifecycle enforcement;
- restart recovery;
- provider routing, settings, fallback, canonical identity, and independence;
- credential exclusion from durable evidence;
- behavior/effectiveness evals;
- selected legacy parity where useful.

## Plan maintenance

Update this file whenever an increment is completed, materially re-scoped, split, reordered, or rejected. Record the reason rather than silently changing direction. Tiber tickets should reference the relevant increment and acceptance criteria; this file remains the durable overall migration map.
