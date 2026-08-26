# Tiber

Tiber is a deterministic development workflow and shared task-tracking package
for [Pi](https://github.com/badlogic/pi-mono). It runs inside an unmodified Pi
Node.js process and is being rebuilt as the public npm package
`@jwilger/tiber`.

Tiber provides signed shared tasks, exact claims, semantic RED/GREEN workflow,
review-bound delivery, independent CI/review authority, bounded campaigns,
human exceptions, context headroom, Context7, and optional Hindsight memory.
It preserves a read-only bootstrap mode until governed authority is available.

## Install, upgrade, and uninstall

Tiber 1.x supports Node.js 22.23.1 through the Node 22 line and unmodified Pi
0.84.2 or newer before Pi 1.0. Install the stable npm package into stock Pi:

```shell
pi install npm:@jwilger/tiber@1
```

Upgrade or reconcile the installed package, then restart Pi:

```shell
pi update npm:@jwilger/tiber
```

Remove the user-global installation and retained package checkout:

```shell
pi remove npm:@jwilger/tiber
```

A project-local installation uses `-l` on install/remove and is recorded in
`.pi/settings.json`. Removing Tiber does not delete signed task history,
worktrees, artifacts, or user-local authority records; inspect and remove those
separately only after their governing tasks and processes are closed. Never
delete `refs/heads/tiber/tasks/v1` as an uninstall shortcut.

## Guided setup

After Tiber is loaded, run one command in the repository:

```text
/tiber-setup
```

The setup assistant inspects the repository and current layered configuration,
then discusses one choice at a time. It explains and recommends every shipping
setting, authority floor, secret reference, project command and workflow
declaration, and optional integration. After a complete preview and explicit
approval, its typed setup host validates and persists the configuration, can
write or remove project declarations, and locally grants only the exact
`.tiber/commands.json` digest the user confirms. No manual JSON editing or
sequence of Tiber commands is required.

Secret values, strong-containment attestations, service administration, and
other externally provisioned authority never enter model context. Setup reports
those as explicit blockers or optional disabled capabilities rather than
fabricating them. Cancelling restores ordinary tools and containment policy.
Rerun `/tiber-setup` at any time to inspect or modify the setup.

## Development

Use the pinned local shell when Nix is available:

```shell
nix develop
npm install
npm run verify:fast
```

Nix is local convenience only. CI uses pinned Node and npm directly.

Build the Pi extension:

```shell
npm run build
```

Load it from this checkout:

```shell
pi -e ./dist/extension/index.js
```

Run `/tiber-setup` for the ordinary configuration path. `/tiber:doctor`,
`/tiber:settings`, `/tiber:containment`, `/tiber:tasks`, and `/tiber:task`
remain optional diagnostic and recovery surfaces.

Headless settings inspection and editing are also available for automation and
explicit recovery:

```text
/tiber:settings show
/tiber:settings set global assuranceLevel workspace-isolated
/tiber:settings set project worktreeMode current
/tiber:settings set project worktreeMode inherit
/tiber:settings lock assuranceLevel workspace-and-network-isolated
/tiber:settings unlock assuranceLevel unlock minimumAssuranceLevel=workspace-and-network-isolated
/tiber:settings secret context7 environment CONTEXT7_API_KEY
```

Global assurance locks prevent project settings from broadening authority.
Secret settings persist only external environment-variable references, never
secret values. Strong assurance levels require a signed external attestation
in `.tiber/containment-attestation.json`, a trusted verifier key in the private
Pi agent directory, and Linux namespace corroboration. Any missing, invalid,
mismatched, or expired evidence enters containment lockdown before provider
or tool dispatch while diagnostics remain available. Tiber replaces Pi's
active `read`, `bash`, `edit`, and `write` schemas with a fixed governed
surface: reads require canonical in-workspace targets, and mutation remains
denied until a remotely published exclusive task claim exists.

Shared Backlog tasks are append-only signed events on
`refs/heads/tiber/tasks/v1`. Publication uses ordinary fast-forward pushes and
retries from the newly observed head after a race; it never force-pushes.
Malformed events or any unsigned/invalid commit degrade the board read-only.
Git signing identity and SSH allowed-signers configuration are taken from the
repository's local Git configuration. A task can receive a canonical structured
specification with `/tiber:task specify <id> <base64url-json>`. Running
`/tiber:task ready <id>` creates a fresh, tool-free in-process reviewer session
with a 60-second and 4096-output-token budget; only an exact-schema, finding-free
review of the pinned specification digest can publish Ready.

`/tiber:work <ready-task-id>` compiles the built-in or narrower project
`.tiber/workflow.json`, durably records a claim intent, publishes one exclusive
claim, pins the exact source baseline and workflow digest, and revalidates both
before work begins. Baseline drift releases the claim and preserves Ready
ordering. Invalid workflow data, missing floor stages, competing claims, and
unresolved publication attempts fail closed. A successful claim receives a
quota-bounded dedicated branch and owned worktree in Tiber's private agent
directory. Ownership survives restart; shutdown terminates only registered
process groups. Cleanup refuses foreign, ambiguous, or active ownership and
first commits dirty tracked and untracked source to a private local
`refs/tiber/recovery/...` ref that is never pushed automatically.

Human takeover is available as `/tiber:work takeover <task-id>`. It requires an
interactive exact task-and-claim confirmation, publishes a state-bound takeover
event, and transfers durable worktree ownership; stale heartbeat alone never
transfers authority.

Projects may define closed, shell-free command data in
`.tiber/commands.json`: a name, absolute executable, fixed argv, exact literal
environment, worktree cwd, timeout, and inline-output bound. A human grants the
canonical catalog digest with `/tiber:commands grant`; repository edits revoke
the grant automatically. `tiber_command` runs only a granted name for an exact
active task claim and records its detached process group.

Oversized stdout/stderr is stored privately by SHA-256 instead of entering the
model context. Results include only bounded UTF-8 head/tail previews and an
artifact digest. `tiber_artifact_range` and `tiber_artifact_search` provide
bounded verified access; age, count, and byte quotas reap old artifacts.

`/tiber:red <task-id> <test-command> <test-mapping> <exact-scenario-name>`
projects the pinned scenario into deterministic Gherkin, runs only a locally
granted test-purpose
command in the owned worktree, stores the exact diagnostic by digest, and asks
a fresh tool-free classifier whether that failure is scenario-specific.
Unrelated, passing, stale, unbound, or malformed observations are rejected. A
compile failure counts only when it specifically demonstrates the scenario's
missing public surface. Before the resulting durable RED receipt, governed
`edit` and `write` permit only exact task test mappings; production paths remain
mechanically denied. `/tiber:green` takes the same arguments, requires the exact
RED receipt and a successful diagnostic observation, runs a fresh lightweight
review, and publishes one signed scenario increment. Repeating this pair covers
every scenario and mapped test without granting authority from model output.

After all scenarios and mappings are preserved,
`/tiber:final-review <task-id> <verification-command>` runs an exact granted
verification-purpose command and fresh risk-selected, tool-free review lenses.
Findings or source/verification deltas reset the signed clean streak; three
consecutive complete clean iterations are required.

`/tiber:deliver <task-id> <mode> <destination-ref-or-> <subject> -- <body>`
creates a signed Conventional Commit from the exact reviewed source snapshot.
The closed modes are `local-only`, `branch-push`, `direct`, and `review`;
non-local modes require an exact `refs/heads/...` destination and use only
fast-forward Git pushes. The signed task receipt records the exact commit, tree,
source snapshot, destination, and independently observed remote revision. Source
drift or a non-fast-forward remote head denies delivery and requires
revalidation; Tiber never force-pushes.

`/tiber:ci <task-id>` observes every required CI authority for that delivered
commit. Authorities are configured only in the user-local
`$PI_CODING_AGENT_DIR/tiber/ci-authorities.v1.json` as unique names, absolute
executable paths, SHA-256 executable pins, and fixed argv containing exactly one
`{revision}` argument. Tiber copies the verified executable bytes to a private
temporary file before shell-free execution and accepts only closed schema-v1
JSON observations naming the requested authority and full commit SHA. Every
authority must report terminal `success`; `pending` remains incomplete and any
terminal `failure` creates a repository-wide delivery hold shared by all
worktrees. After causal repair and a successful exact-revision rerun,
`/tiber:ci <task-id> --recover <causal-diagnosis>` records recovery evidence and
releases the hold. CI receipts remain separate from Git delivery receipts.
For review-mode delivery, `/tiber:review open <task-id> <owner/repository>
<base> <title> -- <body>` creates the exact pull request through the first-party
GitHub HTTP adapter. `/tiber:review observe <task-id>` independently observes
reviews, resolved conversations, exact-SHA check runs, author merge permission,
and merge state. The four operations use separate
`TIBER_GITHUB_PR_TOKEN`, `TIBER_GITHUB_REVIEW_TOKEN`,
`TIBER_GITHUB_CI_TOKEN`, and `TIBER_GITHUB_MERGE_TOKEN` capabilities. An
ordinary PR gets squash auto-merge only after approval, resolved conversations,
all checks, and author permission. Missing permission leaves it open. A
release-please branch or release title is always held for explicit human merge;
Tiber never enables its auto-merge. Signed task events retain the exact PR,
gates, disposition, and observed merge.

Current library documentation is available through first-party `resolve_library`
and `query_docs` tools. Network use is denied unless
`TIBER_CONTEXT7_NETWORK=enabled`; `TIBER_CONTEXT7_ENDPOINT` defaults to the exact
`https://context7.com/api/v2` endpoint, and `CONTEXT7_API_KEY` optionally supplies
the service credential. Direct bounded HTTP is used—never an MCP bridge.
Responses carry library/version, endpoint, digest, and cache provenance, while
oversized documentation is exposed through Tiber's artifact tools.

Optional Hindsight memory uses direct HTTP rather than an SDK or MCP bridge.
Set `TIBER_HINDSIGHT_ENDPOINT` to an HTTPS service (or exact loopback test
service), then independently enable `TIBER_HINDSIGHT_{GLOBAL,PRIVATE,SHARED}_{RECALL,RETAIN}`
with the value `enabled`. Shared access additionally requires
`TIBER_HINDSIGHT_SHARED_BANK`; `HINDSIGHT_API_KEY` is optional credential
material. Banks remain separate, initial recall happens at most once with a hard
budget, later recall is explicit, and only host-observed reviewed completions
can reach shared memory. Raw output, source, diffs, and detected secrets are
excluded from retention.

Tiber follows normal Pi conversation without requiring users to memorize
workflow commands. The active `tiber_workflow_request` tool lets Pi request a
typed task or campaign operation inferred from ordinary intent. Tiber injects
current signed task state as suffix context, validates every request against
deterministic authority, and automatically performs clean readiness-to-claim
progression when exact evidence permits it. A model request never grants
mutation authority. `/tiber:*` commands remain optional inspection and explicit
recovery surfaces; human input is reserved for genuine policy boundaries.

`/tiber:campaign start <bounds-base64url>` creates a repository-local
campaign checkpoint with task, per-initiative task, duration, cost, token, and
concurrency limits. `/tiber:campaign tick <input-base64url>` deterministically
ranks typed candidates, durably records consumption before returning start
requests, and stops at the applicable bound. Pre-mutation blockers release and
defer; post-mutation blockers retain their work. Both remain as non-modal
`/tiber:attention` items while independent work continues. `/tiber:campaign
goal <title>` publishes a provenance-bearing ad-hoc Backlog task. Session
shutdown records a restart-safe campaign checkpoint before process cleanup.

Tiber reserves context headroom through Pi's native `compaction.reserveTokens`
setting (default `16384`) and uses hard typed budgets for mandatory authority,
verification, goal, working, and optional context. Its automatic workflow
context has a byte-stable prompt/tool prefix; freshly folded signed state is an
append-only authority suffix. Lower-priority segments may be omitted at a hard
bound, but authority and verification overflow blocks instead of weakening
policy. Every Pi compaction starts an explicit cache epoch, privately preserves
the complete serialized source under its SHA-256 identity, sends only a bounded
input to an advisory summarizer, and appends normative provenance. Missing
model routes, malformed state, empty summaries, and artifact failures cancel
compaction rather than silently losing verification context.

When a consequential goal is genuinely blocked and no compliant route remains,
`tiber_exception_request` obtains an independent tool-free necessity review and
creates one deduplicated human attention item and prompts the human with the
complete frozen claim. Confirmation approves it for five minutes and one use;
`/tiber:exception` remains an optional inspection and recovery surface. Tiber
consumes the approval durably before directly executing the exact
shell-free executable, arguments, environment, directory, paths, preimages,
revision, and state binding. Capability material is never exposed to the model;
replay, near matches, drift, future use, expiry, and corrupt audit state fail
closed.

Only after an exact delivery and its complete CI receipt may
`/tiber:done <task-id>` terminate that claim's processes, release the claim,
preserve dirty source privately, remove its owned worktree, and publish Done.
Review-mode tasks additionally require the exact pull request to be observed as
merged.

## Status and architecture

The accepted replacement plan is in
[`docs/plans/0001-stock-pi-typescript-replacement.md`](docs/plans/0001-stock-pi-typescript-replacement.md).
ADRs are authoritative decisions; [`ARCHITECTURE.md`](ARCHITECTURE.md) is their
cumulative normative architecture.

## License

Licensed under either the Apache License 2.0 or MIT License at your option.
