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

## Status and architecture

The accepted replacement plan is in
[`docs/plans/0001-stock-pi-typescript-replacement.md`](docs/plans/0001-stock-pi-typescript-replacement.md).
ADRs are authoritative decisions; [`ARCHITECTURE.md`](ARCHITECTURE.md) is their
cumulative normative architecture.

## License

Licensed under either the Apache License 2.0 or MIT License at your option.
