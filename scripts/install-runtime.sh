#!/usr/bin/env bash
set -euo pipefail

PACKAGE_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
RUNTIME_ROOT=${TIBER_RUNTIME_ROOT:-"$PACKAGE_ROOT/.runtime"}
SOURCE=${TIBER_RUST_SOURCE:-"$PACKAGE_ROOT"}
VERSION=0.1.0
RELEASE="$RUNTIME_ROOT/releases/$VERSION"
LOCK="$RUNTIME_ROOT/install.lock"
mkdir -p "$RUNTIME_ROOT/releases"

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'Cargo is required. Install a compatible Rust toolchain, then rerun npm run runtime:install.' >&2
  exit 2
fi

until mkdir "$LOCK" 2>/dev/null; do sleep 0.1; done
cleanup() { rm -rf "${STAGING:-}" "$LOCK"; }
trap cleanup EXIT INT TERM

if "$RELEASE/bin/tiber" doctor 2>/dev/null | grep -Fq '0.1.0 protocol 1'; then
  ln -sfn "releases/$VERSION" "$RUNTIME_ROOT/current.next"
  mv -Tf "$RUNTIME_ROOT/current.next" "$RUNTIME_ROOT/current"
  exit 0
fi

STAGING=$(mktemp -d "$RUNTIME_ROOT/.install.XXXXXX")
CARGO_HOME="$RUNTIME_ROOT/cargo-home" cargo install --locked --path "$SOURCE" --root "$STAGING"
"$STAGING/bin/tiber" doctor | grep -Fq '0.1.0 protocol 1'
rm -rf "$RELEASE.next"
mv "$STAGING" "$RELEASE.next"
STAGING=
mv -T "$RELEASE.next" "$RELEASE"
ln -sfn "releases/$VERSION" "$RUNTIME_ROOT/current.next"
mv -Tf "$RUNTIME_ROOT/current.next" "$RUNTIME_ROOT/current"
