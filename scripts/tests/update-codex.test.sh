#!/usr/bin/env bash
set -euo pipefail

workspace_root=$(git rev-parse --show-toplevel)
updater="$workspace_root/scripts/update-codex.sh"
fixture_root=$(mktemp -d)
trap 'rm -rf "$fixture_root"' EXIT

git_env=(
  GIT_AUTHOR_NAME='Tiber Test'
  GIT_AUTHOR_EMAIL='tiber-test@example.invalid'
  GIT_COMMITTER_NAME='Tiber Test'
  GIT_COMMITTER_EMAIL='tiber-test@example.invalid'
)

commit_file() {
  local repository=$1 path=$2 content=$3 message=$4
  printf '%s\n' "$content" > "$repository/$path"
  git -C "$repository" add "$path"
  env "${git_env[@]}" git -C "$repository" -c commit.gpgsign=false commit -m "$message" >/dev/null
}

make_remote() {
  local name=$1
  local work="$fixture_root/$name-work"
  local bare="$fixture_root/$name.git"
  git init -q -b main "$work"
  commit_file "$work" README.md initial initial
  git clone -q --bare "$work" "$bare"
  printf '%s\n' "$work" "$bare"
}

mapfile -t upstream_paths < <(make_remote upstream)
upstream_work=${upstream_paths[0]}
upstream_bare=${upstream_paths[1]}
mkdir -p "$upstream_work/codex-rs"
commit_file "$upstream_work" codex-rs/README.md workspace workspace
git -C "$upstream_work" push -q "$upstream_bare" main
git -C "$upstream_work" tag --no-sign rust-v0.148.0
git -C "$upstream_work" tag --no-sign rust-v0.149.0-alpha.1
git -C "$upstream_work" push -q --tags "$upstream_bare"
stable_commit=$(git -C "$upstream_work" rev-parse rust-v0.148.0^{commit})

fork_work="$fixture_root/fork-work"
git clone -q "$upstream_bare" "$fork_work"
git -C "$fork_work" switch -q -c tiber-support
commit_file "$fork_work" SUPPORT.md enabled 'feat: support hook'
support_commit=$(git -C "$fork_work" rev-parse HEAD)
fork_bare="$fixture_root/fork.git"
git clone -q --bare "$fork_work" "$fork_bare"
git -C "$fork_work" push -q "$fork_bare" main tiber-support

make_tiber_fixture() {
  local target=$1
  mkdir -p "$target/scripts" "$target/crates/tiber-cli"
  cp "$updater" "$target/scripts/update-codex.sh"
  chmod +x "$target/scripts/update-codex.sh"
  cat > "$target/codex-source.toml" <<EOF
upstream_repository = "$upstream_bare"
fork_repository = "$fork_bare"
fork_push_repository = "$fork_bare"
cargo_repository = "https://github.com/jwilger/codex.git"
stable_tag = "rust-v0.148.0"
upstream_commit = "$stable_commit"
fork_branch = "tiber-support"
fork_commit = "$support_commit"
nix_output_hash_key = "codex-agent-extension-0.148.0"
nix_output_hash = "sha256-test="
EOF
  cat > "$target/crates/tiber-cli/Cargo.toml" <<EOF
[dependencies]
codex-tui = { git = "https://github.com/jwilger/codex.git", rev = "$support_commit" }
EOF
  cat > "$target/Cargo.lock" <<EOF
source = "git+https://github.com/jwilger/codex.git?rev=$support_commit#$support_commit"
EOF
  cat > "$target/flake.nix" <<'EOF'
outputHashes."codex-agent-extension-0.148.0" = "sha256-test=";
EOF
  git init -q -b main "$target"
  git -C "$target" add .
  env "${git_env[@]}" git -C "$target" -c commit.gpgsign=false commit -m fixture >/dev/null
}

current="$fixture_root/tiber-current"
make_tiber_fixture "$current"
current_output=$(cd "$current" && TIBER_CODEX_TEST_MODE=1 scripts/update-codex.sh)
grep -Fq 'Codex is already current at rust-v0.148.0' <<<"$current_output"
if grep -Fq 'rust-v0.149.0-alpha.1' <<<"$current_output"; then
  echo 'prerelease tag was treated as stable' >&2
  exit 1
fi

printf '%s\n' dirty > "$current/dirty.txt"
if dirty_output=$(cd "$current" && TIBER_CODEX_TEST_MODE=1 scripts/update-codex.sh 2>&1); then
  echo 'dirty checkout unexpectedly accepted' >&2
  exit 1
fi
grep -Fq 'refusing to update Codex from a dirty Tiber checkout' <<<"$dirty_output"

stale="$fixture_root/tiber-stale"
make_tiber_fixture "$stale"
sed -i 's/stable_tag = "rust-v0.148.0"/stable_tag = "rust-v0.147.0"/' "$stale/codex-source.toml"
git -C "$stale" add codex-source.toml
env "${git_env[@]}" git -C "$stale" -c commit.gpgsign=false commit -m stale >/dev/null
if stale_output=$(cd "$stale" && TIBER_CODEX_TEST_MODE=1 scripts/update-codex.sh 2>&1); then
  echo 'test mode unexpectedly mutated a stale fixture' >&2
  exit 1
fi
grep -Fq 'test mode never updates or pushes repositories' <<<"$stale_output"
grep -Fq 'preserved recovery clone at' <<<"$stale_output"
[[ $(git -C "$fork_bare" rev-parse refs/heads/tiber-support) == "$support_commit" ]]

commit_file "$upstream_work" NEW_STABLE.md stable 'release: stable'
git -C "$upstream_work" tag --no-sign rust-v0.149.0
git -C "$upstream_work" push -q "$upstream_bare" main --tags
new_upstream=$(git -C "$upstream_work" rev-parse rust-v0.149.0^{commit})

update_fixture="$fixture_root/tiber-update"
make_tiber_fixture "$update_fixture"
stub_bin="$fixture_root/stub-bin"
mkdir -p "$stub_bin"
cat > "$stub_bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'cargo %s\n' "$*" >> "$TIBER_CODEX_COMMAND_LOG"
if [[ $PWD == */codex-rs ]]; then
  exit 0
fi
if [[ ${1:-} != update ]]; then
  exit 0
fi
revision=$(sed -nE 's/.*rev = "([0-9a-f]{40})".*/\1/p' crates/tiber-cli/Cargo.toml | head -n 1)
sed -i -E "s/[0-9a-f]{40}/$revision/g" Cargo.lock
EOF
cat > "$stub_bin/nix" <<'EOF'
#!/usr/bin/env bash
printf 'nix %s\n' "$*" >> "$TIBER_CODEX_COMMAND_LOG"
if [[ $* == *'.#checks.x86_64-linux.package-smoke'* ]]; then
  exit 0
fi
printf '%s\n' '       got:    sha256-updated=' >&2
exit 1
EOF
chmod +x "$stub_bin/cargo" "$stub_bin/nix"
command_log="$fixture_root/update-commands.log"
: > "$command_log"

(cd "$update_fixture" && \
  PATH="$stub_bin:$PATH" \
  TIBER_CODEX_COMMAND_LOG="$command_log" \
  TIBER_CODEX_TEST_MODE=1 \
  TIBER_CODEX_TEST_ALLOW_UPDATE=1 \
  scripts/update-codex.sh >/dev/null)
updated_support=$(git -C "$fork_bare" rev-parse refs/heads/tiber-support)
git -C "$fork_bare" merge-base --is-ancestor "$new_upstream" "$updated_support"
grep -Fq 'stable_tag = "rust-v0.149.0"' "$update_fixture/codex-source.toml"
grep -Fq "fork_commit = \"$updated_support\"" "$update_fixture/codex-source.toml"
grep -Fq "rev = \"$updated_support\"" "$update_fixture/crates/tiber-cli/Cargo.toml"
grep -Fq "?rev=$updated_support#$updated_support" "$update_fixture/Cargo.lock"
grep -Fq 'outputHashes."codex-agent-extension-0.149.0" = "sha256-updated="' "$update_fixture/flake.nix"
grep -Fq 'host_policy_rejects_requests_before_worker_dispatch' "$command_log"
grep -Fq 'default_policy_transparently_admits_distinct_builtin_slash_commands' "$command_log"
grep -Fq 'host_policy_can_reject_one_exact_builtin_plan_decision' "$command_log"
grep -Fq 'plan_slash_command_with_args_submits_prompt_in_plan_mode' "$command_log"
grep -Fq 'slash_side_requests_forked_side_question_while_task_running' "$command_log"
grep -Fq 'slash_btw_requests_forked_side_question_while_task_running' "$command_log"
grep -Fq 'plan_implementation_popup_yes_emits_submit_message_event' "$command_log"
grep -Fq 'plan_implementation_popup_stay_emits_typed_cancel' "$command_log"
grep -Fq -- '--test embedded_runtime bare_tiber_launches_embedded_codex_without_invoking_path_codex' "$command_log"
git -C "$update_fixture" add codex-source.toml Cargo.lock crates/tiber-cli/Cargo.toml flake.nix
env "${git_env[@]}" git -C "$update_fixture" -c commit.gpgsign=false commit -m updated >/dev/null
second_output=$(cd "$update_fixture" && TIBER_CODEX_TEST_MODE=1 scripts/update-codex.sh)
grep -Fq 'Codex is already current at rust-v0.149.0' <<<"$second_output"

echo 'update-codex behavior tests passed'
