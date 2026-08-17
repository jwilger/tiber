# ADR-0012: Enforce a workspace-wide strict Clippy allowlist

## Status

Accepted

## Date

2026-08-10

## Context

Copied UI code and new harness crates must share one reviewable quality floor;
grandfathered warnings hide defects and make future toolchain changes noisy.

## Decision

Every shipping crate inherits workspace lints. Enable `pedantic` and
`restriction` at warning priority -1, allow
`blanket_clippy_restriction_lints`, `clippy::expect_used`,
`clippy::implicit_return`, and `clippy::question_mark_used`; then fail
`cargo clippy --workspace --all-targets --all-features -- -D warnings`.
Forbid unsafe code. Library code contains no `unwrap`, `expect`, `panic`,
`todo`, or `unimplemented`.

The three global Clippy allowances remove stylistic noise from typed
fallible-control-flow and test fixture code; they do not authorize fallible
shortcuts in shipping behavior. The functional-core and black-box TDD rules
remain the controlling constraints for those decisions.

Fix warnings where practical. Permit only narrowly scoped
`#[expect(clippy::lint_name, reason = "…")]`. Blanket
`#[allow(clippy::…)]`, unreasoned suppression, and grandfathered fork source
are prohibited. Workspace exceptions require an amendment to this ADR.
`ModelEvent` is the sole crate-level Clippy exception: its EventCore derive
generates public checked-model helpers at the invoking crate scope, and the
macro exposes no item-local lint hook for those generated helpers. A crate that
directly invokes that derive may use one reasoned crate-inner expectation for
only `clippy::exhaustive_structs` and `clippy::impl_trait_in_params`; it must
name this macro limitation. It does not apply to handwritten items or any other
lint.
The pinned RMCP 3.1.2 `ClientHandler` API leaves three required callbacks
deprecated without a supported replacement: `create_message` for explicit
sampling refusal, `list_roots` for Tiber-owned roots, and
`on_logging_message` for bounded untrusted logging. Only those exact callback
implementations in `tiber-rmcp-client` may carry item-level
`#[allow(deprecated)]`, each with a nearby explanation naming this upstream
constraint and the ADR-0008 behavior it preserves. No other Rust lint allow is
authorized by this exception.
`clippy.toml` may set deterministic thresholds but cannot disable categories;
nursery lints are selected individually.

Rust does not emit `clippy::missing_docs_in_private_items` for test targets,
which makes a crate-level expectation unfulfilled under the required
all-targets command. The lint audit therefore permits exactly one
target-aware form: a reasoned `#[cfg_attr(not(test),
expect(clippy::missing_docs_in_private_items, ...))]` immediately on a public
module declaration that contains EventCore generated private model internals.
It is not permitted on a crate, any other item, or for any other lint.

Add a repository check proving first-party crate inheritance and auditing
unapproved `allow` or `expect` attributes.

## Consequences

New warnings intentionally break the build until reviewed. Adapting upstream UI
may require substantial fixes, but the policy remains uniform and explicit.

## Alternatives considered

Default Clippy, blanket category suppression, and grandfathering copied code
were rejected because they create an unreviewed exception surface.

## Revisit when

The pinned Rust toolchain changes lint semantics; review each exception rather
than weakening categories wholesale.
