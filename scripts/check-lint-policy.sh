#!/usr/bin/env bash

set -euo pipefail

readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly repository_root="$(cd -- "$script_dir/.." && pwd)"

node --test "$script_dir/check-lint-policy.test.mjs"
node "$script_dir/check-lint-policy.mjs" "$repository_root"
