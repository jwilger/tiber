# Tiber's deterministic local quality gate. Run inside \`nix develop\`.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default: ci

# Mirrors the required CI checks. No networked model or provider runner belongs here.
ci: actionlint lint-policy format clippy test authority-fixture package

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

authority-fixture:
    node scripts/tests/probe-app-server-effective-authority.test.mjs

package:
    # The all-feature Cargo gate and Nix package compile the same large graph.
    # Release Cargo artifacts before Nix starts so both copies do not exhaust CI disk.
    cargo clean --target-dir target/tiber
    nix build --no-link .#tiber
    nix build --no-link .#checks.x86_64-linux.package-smoke

# Manually compare a locally generated protocol schema to the reviewed fixture.
app-server-authority-fixture schema:
    bash scripts/check-app-server-authority-fixture.sh "{{schema}}"
