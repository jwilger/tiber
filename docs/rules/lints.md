# Lint policy

All shipping workspace crates inherit the workspace Rust and Clippy policy.
`unsafe_code` is forbidden. The required command is:

```shell
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

Fix warnings when practical. An intentional exception uses a narrowly scoped
`#[expect(clippy::lint_name, reason = "…")]`; it must have a concrete reason
and comply with [ADR 0012](../adr/0012-tiber-strict-clippy-policy.md).
Blanket Clippy allowances and unreasoned suppressions are prohibited. The
lint-policy script checks each first-party crate's inheritance and source
attributes. The only target-aware exception is the narrowly audited,
reasoned non-test `missing_docs_in_private_items` expectation directly on a
public module that contains EventCore generated private model internals; see
[ADR 0012](../adr/0012-tiber-strict-clippy-policy.md).
