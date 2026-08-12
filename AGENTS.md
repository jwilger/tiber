# Tiber contributor guide

Tiber is a standalone Rust application, not an integration bundle. The root
workspace is the product. `old-code-for-reference/` is frozen migration input:
do not add it to workspace membership, CI, packaging, or a public command path.

## Toolchain and checks

Use the pinned Nix shell; do not install toolchains globally.

```shell
nix develop
just ci
```

`just ci` is the required deterministic gate. It runs actionlint, the
workspace lint-policy audit, formatting, strict Clippy, tests, and the
app-server authority fixture. Keep it credential-free and reproducible.

## Architecture

- Keep a functional core and an imperative shell. Domain decisions are pure;
  adapters interpret closed, typed effects.
- Parse external representations once at the boundary into semantic types.
  Expected failures use typed errors with stable codes, context, causes, and
  retryability.
- Use no unsafe code. Every shipping crate inherits workspace lints. Do not
  add blanket Clippy allowances; a narrow `#[expect]` needs a reason and must
  comply with [ADR 0012](docs/adr/0012-tiber-strict-clippy-policy.md).
- EventCore commands express business-domain intent. Each command folds only
  the facts needed for that decision; do not introduce aggregates, generic
  mutable write models, or whole-session replay as command authority. Register
  shipping models with EventCore's checked-model facilities and consume their
  provenance completely.
- The model may request a tool but never execute one. Keep all mutation,
  process, network, memory, verification, and delivery effects behind Tiber
  policy and durable receipts.

Read [`ARCHITECTURE.md`](ARCHITECTURE.md), [`PRD.md`](PRD.md), the relevant
ADR, and [`docs/rules/`](docs/rules/) before changing an architectural boundary.

## Tests and documentation

Write behavior-focused tests through public boundaries. Keep deterministic
fixtures local and scrub secrets from all inputs, output, and failures. Record
hard-to-reverse decisions in `docs/adr/`; update the relevant rule when a
working agreement changes.

There is no provider-runner or marketplace-validation surface in this
repository. Do not add one or wire one into CI without an explicit product
decision.

## Delivery

Tiber uses direct-to-trunk delivery unless the owner explicitly chooses another
mode. Every authored commit must be signed, use a concise Conventional Commit
subject, and include a non-empty body explaining why the change exists. Never
disable signing and never add AI-attribution trailers.

The delivery boundary is one-way:

1. Complete final review against the final source-content snapshot.
2. Create the signed commit.
3. Run the required gate again against that exact commit.
4. Push and confirm CI for the pushed revision.

A content-identical commit does not invalidate completed source review merely
because staging partition, `HEAD`, commit metadata, or its signature changed.
Restart source review only when reviewed paths, contents, modes, untracked
content, pinned baseline, or requested scope changes. Commit-message and
signature checks are delivery verification, not a reason to repeat source
review. See [`docs/rules/workflow-and-commits.md`](docs/rules/workflow-and-commits.md).
