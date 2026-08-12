#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <codex_app_server_protocol.v2.schemas.json>" >&2
  exit 2
fi

readonly schema_path="$1"
readonly expected_version="codex-cli 0.147.0"
readonly expected_sha256="ff10829cd75b67297019b39ab508ac699198574663579aa18336b7dc55ea178f"
readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly repository_root="$(cd -- "$script_dir/.." && pwd)"
readonly fixture_path="$repository_root/crates/tiber-app-server/tests/fixtures/codex-0.147.0-authority-surface.json"
readonly extractor_path="$script_dir/extract-app-server-authority-surface.jq"

actual_version="$(codex --version)"
if [[ "$actual_version" != "$expected_version" ]]; then
  echo "codex_version_mismatch: expected $expected_version, got $actual_version" >&2
  exit 1
fi

actual_sha256="$(sha256sum "$schema_path" | cut -d' ' -f1)"
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "app_server_schema_hash_mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi

generated_fixture="$(mktemp "${TMPDIR:-/tmp}/tiber-authority-surface.XXXXXX.json")"
trap 'rm -f "$generated_fixture"' EXIT

jq \
  --arg codex_version "0.147.0" \
  --arg schema_sha256 "$actual_sha256" \
  -f "$extractor_path" \
  "$schema_path" \
  | prettier --parser json >"$generated_fixture"

if ! cmp -s "$fixture_path" "$generated_fixture"; then
  diff -u "$fixture_path" "$generated_fixture" >&2 || true
  echo "app_server_authority_fixture_mismatch: regenerate the committed projection" >&2
  exit 1
fi

echo "app-server authority fixture matches Codex 0.147.0 generated schema"
