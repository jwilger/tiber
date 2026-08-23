# Tiber product requirements

## Summary

Tiber is a public installable package for unmodified stock Pi that provides
shared task tracking, deterministic development-workflow guardrails, bounded
autonomous execution, and controlled context and memory services. It replaces
the former Rust/Codex implementation without compatibility or data migration.

## Users and outcomes

A developer can install `@jwilger/tiber`, open an ordinary Git repository in Pi,
and:

- Configure local authority without trusting repository declarations.
- Share signed tasks, specifications, priorities, dependencies, and claims
  through Git.
- Start one task or a bounded campaign and continue steering from the visible
  conversation.
- Observe mechanical enforcement of specification review, BDD/TDD, review,
  delivery, CI, and cleanup.
- Collaborate concurrently without duplicate task ownership.
- Recover interrupted work without a background daemon.
- Use bounded documentation, large-output, headroom, and optional memory
  services without installing executable marketplace extensions.

## Functional requirements

### Installation and compatibility

- Ship as the public npm package `@jwilger/tiber` under `MIT OR Apache-2.0`.
- Load extensions, skills, prompts, workflows, and themes through Pi's package
  manifest.
- Run in Pi's Node.js process without a launcher, daemon, Pi fork, native Tiber
  binary, or MCP bridge.
- Test against explicitly supported stock Pi and Node versions.

### Configuration and trust

- Resolve built-in, user-global, and project-local values with visible
  inheritance.
- Let global settings constrain project overrides to more restrictive values.
- Keep project authority and secret references outside repository-controlled
  files.
- Bind project trust to generated identity, canonical Git common directory,
  and expected remotes.
- Enter visible read-only lockdown when required authority, extension inventory,
  or containment evidence is missing or invalid.

### Tasks and collaboration

- Store authoritative shared task events on a dedicated signed Git branch.
- Support Backlog, Ready, In Progress, Done, an orthogonal Blocked state, shared
  Ready order, dependencies, and evidence.
- Publish claims remotely before mutation and enforce one exclusive claim per
  task.
- Never transfer a stale claim automatically.
- Permit provenance-bearing untriaged discovery without autonomous promotion or
  priority changes.

### Specifications and workflow

- Store canonical structured Gherkin and typed acceptance criteria with the
  task.
- Require fresh readiness review before Ready and fresh baseline revalidation
  after claim.
- Compile versioned data-only workflow definitions into immutable canonical IR.
- Pin active runs to specification, baseline, workflow, policy, containment,
  model route, and budget.
- Enforce the Tiber policy floor regardless of project workflow.

### BDD, TDD, and review

- Work one independently valuable vertical scenario at a time.
- Require a semantically valid RED observation before production mutation.
- Allow a compile failure as RED only when it demonstrates a missing required
  public surface.
- Follow the actual diagnostic one minimal step at a time.
- Permit refactoring only while green.
- Require fresh lightweight review for every increment.
- Require scope-complete verification and three consecutive complete,
  finding-free final-review iterations.
- Reset dependent evidence after findings, malformed review output, or material
  source changes.

### Autonomous execution

- Keep the visible Pi session as coordinator and run worker roles in isolated
  in-process Pi sessions.
- Enforce hard task, initiative, time, token, cost, concurrency, and effect
  bounds.
- Keep worker prompts and tools stable within cache epochs.
- Handle ordinary denial privately; escalate only a necessary unresolved
  blocker.
- Continue eligible campaign work around pre- and post-mutation blockers using
  their distinct claim/worktree rules.
- Stop work and checkpoint when Pi exits.

### Effects and containment

- Let models request but never execute or authorize effects.
- Expose structured file operations and named executable/argv commands rather
  than arbitrary shell text.
- Persist intent and validate observations and receipts for consequential
  effects.
- Support host-trusted and externally attested isolation levels.
- Verify strong containment first on Linux and fail closed elsewhere when it is
  required.
- Allow only human exact, state-bound, single-use, audited exceptions.

### Worktrees and recovery

- Use dedicated task branches/worktrees by default.
- Reconcile owned worktrees, claims, processes, and heartbeats after restart.
- Never delete foreign or ambiguous paths.
- Preserve abandoned uncommitted source in private local recovery refs before
  cleanup.
- Require claim release and cleanup before Done.

### Delivery and CI

- Keep Git remote, CI providers, and review services as separate permissions and
  receipts.
- Support local-only, branch, direct, and review delivery modes.
- Use signed Conventional Commits with explanatory bodies.
- Require terminal success from every required CI authority for the exact
  delivered revision.
- Create a repository-wide hold after terminal CI failure until causally
  resolved.
- Support GitHub through a thin first-party adapter while retaining generic
  ports for other services.

### Context and integrations

- Virtualize oversized Tiber-controlled output into local content-addressed
  artifacts with bounded previews and search/range access.
- Reserve context headroom and use typed priorities without rewriting prior
  turns.
- Provide first-party bounded Context7 library resolution and documentation
  queries with provenance.
- Provide optional Hindsight HTTP integration with separate private and opt-in
  shared banks and strict retention filters.

### UI

- Provide status, doctor, settings, Kanban, task detail, work, campaign,
  attention, containment, and artifact surfaces.
- Keep lockdown, active workflow state, budgets, claim state, CI hold, and human
  attention visible without modal prompts for ordinary denial.

## Quality and delivery requirements

- Use strict TypeScript and a functional core/imperative shell.
- Parse untrusted data once and use stable typed failures.
- Test product behavior through public boundaries with deterministic fixtures.
- Do not test copied development-guidance wording.
- Keep local commit hooks fast and run full verification in ordinary Node/npm
  CI.
- Require PRs for `main`; authorized ordinary PRs may auto-merge after gates.
- Maintain versions through a release PR requiring explicit human merge.
- Publish automatically afterward through npm trusted OIDC with provenance.

## Non-goals

- Migrating legacy tasks, events, sessions, signatures, or schemas.
- Supporting the old Rust CLI or embedded Codex presentation.
- Provisioning containers, namespaces, VMs, or network policy.
- Running after Pi exits.
- Providing an arbitrary provider runner or live-provider CI.
- Trusting mutable repository scripts as CI authorities.
- Installing or executing marketplace packages as dependencies.
- Pretending Tiber can govern effects when it is disabled or a malicious peer
  extension acts outside observable Pi boundaries.

## Delivery plan

The accepted vertical slices and their black-box acceptance boundaries are in
`docs/plans/0001-stock-pi-typescript-replacement.md`.
