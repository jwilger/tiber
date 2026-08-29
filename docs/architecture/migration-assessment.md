# Pi-native Tiber: architecture and migration assessment

## Decision summary

Use this repository as the canonical Pi-native Tiber product boundary and keep Rust authoritative. Pi is the product harness, not a subordinate adapter. Copy and adapt selected source from the legacy `ai-plugins` behavioral reference, establish a versioned JSONL process boundary, and consolidate authority incrementally into one installable `tiber` crate and binary.

## Existing public capability inventory

| Capability                                                        | Current implementation                                                   | Migration classification                                                                   |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| Skill routing and engineering standards                           | `plugins/development-system/skills/*`                                    | Directly reusable through Pi's Agent Skills support                                        |
| Setup and compatibility inspection                                | setup skill, `development-discipline-mcp`, session-start hooks           | Thin Pi adapter plus reusable Rust; hooks become Pi lifecycle events                       |
| Tiber tasks, dependencies, priority, Git history                  | component `tiber-core`, `tiber-git`, CLI/MCP/server                      | Directly reusable Rust, later moved behind native modules                                  |
| Development workflow and debugging/TDD                            | skills plus Development Discipline services                              | Skills reusable; material state/policy requires Rust service migration                     |
| Final review and clean sequence                                   | Development Discipline EventCore authority and standalone `tiber-review` | Directly reusable domain behavior; persistence adapters require consolidation              |
| Fresh reviewers/verifiers                                         | current multi-agent orchestration                                        | Pi-native launch adapter; Rust assignment/fencing remains authoritative                    |
| Worktrees                                                         | setup/worktree skills and scripts                                        | Reusable through thin Pi tools; defer broad migration                                      |
| Commit, push, CI recovery, delivery                               | skills, Tiber CI authority, Git adapters                                 | Rust authority reusable; Pi lifecycle/tool gates required                                  |
| Agentic/eval engineering and eval-case reporting                  | public skills, Promptfoo component                                       | Skills directly reusable; posting remains explicit-approval; broader eval tooling deferred |
| Claude/Codex manifests, hook schemas, MCP bootstrap compatibility | harness-specific manifests/hooks                                         | Obsolete under Pi once equivalent native paths ship                                        |
| Standalone Codex app-server inference and forked Codex TUI        | `tiber/`                                                                 | Deferred/likely obsolete as transport/presentation under Pi; preserve domain engines       |

## Rust inventory and consolidation

Current deployment surfaces are:

- `development-discipline-mcp` package/binary: setup, workflow/final-review EventCore authority.
- component Tiber workspace: `tiber` CLI binary plus `tiber-core`, `tiber-git`, `tiber-mcp`, and `tiber-server`.
- standalone `tiber/` workspace: `tiber` binary, `tiber-review`, `tiber-app-server`, and `tiber-tui`.

Persistence authorities must remain logically distinct while sharing a process: task Git/EventCore history, local-only review history, standalone workflow history, and CI incident fencing cannot be merged into one write model or lock. One async runtime and executable are feasible if modules retain separate repositories, stream identities, locks, and typed ports. No discovered capability requires process isolation except untrusted inference/tool subprocesses, which can remain children behind the one installed entry point.

Target command families: `service stdio`, `doctor`, `setup`, `tasks`, `review`, `workflow`, and `delivery`. Move pure modules first; adapt existing callers; preserve event/schema versions and Git branches. Do not shell or MCP-loop back internally.

## Pi 0.84.4 capability inventory

Official installed documentation/source confirms:

- npm, Git, local-path, and project-local packages with manifest resource globs;
- TypeScript extensions loaded by Jiti, custom tools/commands, dynamic tools, and full lifecycle events;
- fail-safe blocking `tool_call`, session switch/fork interception, model-selection events, and provider request observation;
- session entries and append-only JSONL session trees for adapter-local evidence (not domain authority);
- provider/model catalog access, auth-aware availability, exact `setModel`, thinking-level controls, and custom providers;
- SDK sessions and runtime session replacement; RPC and JSON modes for tests/integration;
- official subprocess subagent example with isolated child Pi processes and cancellation.

Important gaps: Pi's generic model shape does not fully normalize all provider-specific settings; the official subagent example does not provide strong assignment attestation; tool hooks may run for parallel siblings. Rust must therefore parse settings, authorize assignments, and validate exact runtime attestations.

## Ecosystem evaluation

Search identified `pi-subagents`, `pi-background-tasks`, `pi-fabric`, and TUI/scaffolding utilities. None is adopted or vendored in this slice. Their broad runtime/security surfaces exceed the small official mechanisms needed, and no third-party runtime dependency is acceptable without a complete source/license/maintenance/API audit. The official MIT-licensed subagent example is preferred prior art; only its small child-process/JSON-stream pattern may be independently implemented and tested. This is a **reject for runtime adoption**, not a claim that those packages are defective.

## Package and protocol boundary

Layout:

```text
Cargo.toml
package.json
src/
extensions/
skills/
scripts/
tests/
docs/
vendor/
```

Protocol v1 is LF-delimited JSON with a 256 KiB record bound, explicit negotiation, correlation IDs, typed operations, typed outcomes, and stable error `code`/`class`/`retryable` fields. Error classes distinguish domain rejection, configuration, runtime unavailability (adapter), and internal failure. Adapter requests have cancellation and a 10-second deadline. Credentials are excluded by contract; catalog messages contain identities/capabilities only.

Compatibility is explicit: Pi/npm package `1.2.2` currently requires crate `0.1.0` and protocol `1`. Persisted EventCore/schema versions remain domain-specific and will be reported separately rather than inferred from package versions.

## Provider-neutral routing

Initial semantic roles are `bounded-helper`, `substantive-worker`, `independent-reviewer`, and `verifier`. Add architecture, debugging, and grading roles only when they alter responsibility or policy. Rust owns precedence (assignment, project, user, packaged), exact requirements, capability predicates, ordered explicit fallback, independence, and attestation acceptance. Pi reports canonical provider/model identities and capabilities, applies the exact selection, and returns session/model/settings attestation.

Portable preferences are capability/latency/cost classes. Provider settings remain namespaced and must be checked against Pi metadata or rejected visibly. Never silently discard settings or downgrade a strong role.

## Staged migration

1. **Boundary slice:** package loading, host-local Cargo install, protocol negotiation, Rust-backed tool, Rust lifecycle gate, catalog-to-Rust routing, isolated-turn spike, and contract tests.
2. **Safe Tiber daily operations:** migrate task service and persistence unchanged; setup/doctor/removal; dogfood task work.
3. **Workflow/review:** consolidate final-review domains, durable assignments, clean streak, fresh-context reviewer/verifier attestation, restart recovery.
4. **Delivery:** verification binding, commit/push/CI recovery, publication approval, and lifecycle enforcement.
5. **Full daily-use parity:** worktrees, agentic/eval workflows, behavior evals, deliberate upgrades, and release readiness.
6. **Cutover/archive:** archive legacy harness adapters only after evidence and explicit approval.

## Daily-use switch criteria

Switch only when clean-machine/project-local tests prove setup and skill routing; safe Tiber operations; lifecycle coordination; configured clean-review enforcement; fresh independent review/verification; multi-provider routing without silent downgrade; commit/push/CI delivery gates; restart recovery; reversible install/doctor/remove; and representative behavior/effectiveness evals. Legacy-only compatibility does not block cutover.

## Risks and spikes

- Spike Pi child SDK versus subprocess RPC for fresh sessions and canonical attestation.
- Map Pi model/provider metadata to namespaced runtime settings without exposing auth.
- Inventory EventCore stream/schema and file-lock compatibility before moving modules.
- Record provenance and license compatibility for every skill or Rust module copied from the legacy source.
- Verify Cargo's project-config isolation across supported Cargo versions and platforms.
- Define active-workflow version pinning before upgrades.

## Legacy archival strategy

This repository is already the destination, so no later extraction is needed. Copy selected legacy modules with provenance and license notices, preserve persisted-data compatibility, and use focused parity tests to document retained behavior. Archive or delete legacy marketplace adapters only after Pi daily-use criteria are met and explicit approval is given.
