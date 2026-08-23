# Tiber contributor guide

Tiber is the stock-Pi TypeScript package defined by `PRD.md`, the active ADRs,
and `ARCHITECTURE.md`. The former Rust/Codex product is obsolete and must not be
restored, invoked, included in CI, or exposed through a compatibility path.

## Authority and documentation

ADRs record accepted decisions. `ARCHITECTURE.md` is the cumulative normative
architecture derived from active ADRs and may lead implementation. New and
revised code must conform; existing divergence is corrected when that code is
otherwise changed.

Read the relevant local rule before changing code:

- `docs/rules/change-preflight.md`
- `docs/rules/functional-core.md`
- `docs/rules/semantic-types-and-errors.md`
- `docs/rules/bdd-and-tdd.md`
- `docs/rules/verification.md`
- `docs/rules/review.md`
- `docs/rules/workflow-and-commits.md`
- `docs/rules/ci-and-delivery.md`
- `docs/rules/worktree-hygiene.md`
- `docs/rules/agentic-systems.md`

The accepted implementation sequence is
`docs/plans/0001-stock-pi-typescript-replacement.md`. Until the new shared task
board is delivered, that approved plan and its vertical-slice order are the
bootstrap task authority. Do not attempt to use the deleted Rust task system.

## Architecture

Keep a functional core and imperative shell. Parse external representations
once into semantic types. Domain decisions return closed typed effects or
stable typed failures. Models may request effects but never execute or
authorize them. Every consequential effect uses durable intent, observation,
and validated receipt.

Workflow and policy authorization are deterministic. Inference may assess
semantics but cannot grant capability. Human exceptions are exact, single-use,
state-bound, short-lived, and audited.

Use strict TypeScript. Shipping code has no `any`, unsafe casts across trust
boundaries, blanket lint suppressions, hidden subprocess shells, or install-time
native dependencies.

## Development and tests

Use the pinned Node/npm versions. Nix is local convenience only and must not be
required by CI.

Write behavior-focused tests through stable public boundaries. Use a real
failing observation before implementation when behavior changes. Do not add
tests that merely assert copied guidance, prompt wording, types, constants, or
source layout.

Run focused tests while developing. Do not run the complete CI suite locally as
a delivery ritual. Git's pre-commit hook runs formatting, strict lint,
incremental type checking, and fast unit tests. Full acceptance, integration,
recovery, package, and mutation verification belongs to CI. There is no heavy
pre-push hook.

## Delivery

All source changes use pull requests. Direct pushes to `main` and force pushes
are prohibited. Every authored commit is signed and has a concise Conventional
Commit subject plus a non-empty explanatory body. Never disable signing or add
AI-attribution trailers.

Ordinary PRs may auto-merge after every required gate when the author has
permission. Generated release PRs require explicit human merge; tagging,
GitHub Release creation, and OIDC/provenance npm publication are automatic only
after that merge.

Complete final source review before creating the delivery commit. A
content-identical commit metadata or signature change does not invalidate that
review. If a hook or CI gate fails, fix the cause and rerun the narrowest
relevant check before retrying delivery.
