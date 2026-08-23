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

Then run `/tiber:doctor` or `/tiber:settings`.

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
secret values.

## Status and architecture

The accepted replacement plan is in
[`docs/plans/0001-stock-pi-typescript-replacement.md`](docs/plans/0001-stock-pi-typescript-replacement.md).
ADRs are authoritative decisions; [`ARCHITECTURE.md`](ARCHITECTURE.md) is their
cumulative normative architecture.

## License

Licensed under either the Apache License 2.0 or MIT License at your option.
