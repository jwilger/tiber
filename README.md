# Tiber

Tiber is a deterministic development workflow and shared task-tracking package
for [Pi](https://github.com/badlogic/pi-mono). It runs inside an unmodified Pi
Node.js process and is being rebuilt as the public npm package
`@jwilger/tiber`.

The current bootstrap release is deliberately read-only. It provides
`/tiber:doctor`, inherited global/project settings through `/tiber:settings`,
and blocks Pi's known mutation tools while governed task workflows are
implemented.

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

Then run `/tiber:doctor`, `/tiber:settings`, `/tiber:containment`,
`/tiber:tasks`, or `/tiber:task create <title>`.

Headless settings inspection and editing are also available:

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
Only after an exact delivery and its complete CI receipt may
`/tiber:done <task-id>` terminate that claim's processes, release the claim,
preserve dirty source privately, remove its owned worktree, and publish Done.

## Status and architecture

The accepted replacement plan is in
[`docs/plans/0001-stock-pi-typescript-replacement.md`](docs/plans/0001-stock-pi-typescript-replacement.md).
ADRs are authoritative decisions; [`ARCHITECTURE.md`](ARCHITECTURE.md) is their
cumulative normative architecture.

## License

Licensed under either the Apache License 2.0 or MIT License at your option.
