# Tiber's deterministic local quality gate. Run inside \`nix develop\`.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default: ci

# Mirrors the required CI checks. No networked model or provider runner belongs here.
ci: code-quality package

# Runs the source and behavior gates without constructing the Nix package.
code-quality: actionlint lint-policy format clippy test update-codex-test

actionlint:
    actionlint

lint-policy:
    bash scripts/check-lint-policy.sh

format:
    cargo fmt --all --check

clippy:
    CARGO_TARGET_DIR=target/tiber cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

test:
    CARGO_TARGET_DIR=target/tiber cargo test --locked --workspace --all-features

package:
    # The all-feature Cargo gate and Nix package compile the same large graph.
    # Release Cargo artifacts before Nix starts so both copies do not exhaust CI disk.
    cargo clean --target-dir target/tiber
    nix build --no-link .#tiber
    nix build --no-link .#checks.x86_64-linux.package-smoke

# Update the signed embedded Codex fork and Tiber's reproducible source pins.
update-codex:
    scripts/update-codex.sh

# Exercise updater decisions without network access or remote mutation.
update-codex-test:
    scripts/tests/update-codex.test.sh
