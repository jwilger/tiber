# Tiber

Tiber is a deterministic development workflow and shared task-tracking package
for [Pi](https://github.com/badlogic/pi-mono). It runs inside an unmodified Pi
Node.js process and is being rebuilt as the public npm package
`@jwilger/tiber`.

The current bootstrap release is deliberately read-only. It provides
`/tiber:doctor` and blocks Pi's known mutation tools while the governed task and
workflow slices are implemented.

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

Then run `/tiber:doctor`.

## Status and architecture

The accepted replacement plan is in
[`docs/plans/0001-stock-pi-typescript-replacement.md`](docs/plans/0001-stock-pi-typescript-replacement.md).
ADRs are authoritative decisions; [`ARCHITECTURE.md`](ARCHITECTURE.md) is their
cumulative normative architecture.

## License

Licensed under either the Apache License 2.0 or MIT License at your option.
