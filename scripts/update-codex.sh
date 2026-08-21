#!/usr/bin/env bash
set -euo pipefail

die() {
  printf 'update-codex: %s\n' "$*" >&2
  exit 1
}

root=$(git rev-parse --show-toplevel 2>/dev/null) || die 'run this command inside the Tiber checkout'
cd "$root"
[[ -f codex-source.toml ]] || die 'codex-source.toml is missing'

if [[ -n $(git status --porcelain=v1 --untracked-files=normal) ]]; then
  die 'refusing to update Codex from a dirty Tiber checkout'
fi

read_value() {
  local key=$1
  sed -nE "s/^${key}[[:space:]]*=[[:space:]]*\"([^\"]*)\"[[:space:]]*$/\\1/p" codex-source.toml
}

upstream_repository=${TIBER_CODEX_UPSTREAM_URL:-$(read_value upstream_repository)}
fork_repository=${TIBER_CODEX_FORK_URL:-$(read_value fork_repository)}
fork_push_repository=${TIBER_CODEX_FORK_PUSH_URL:-$(read_value fork_push_repository)}
cargo_repository=$(read_value cargo_repository)
recorded_tag=$(read_value stable_tag)
recorded_upstream=$(read_value upstream_commit)
fork_branch=$(read_value fork_branch)
recorded_fork=$(read_value fork_commit)
recorded_hash_key=$(read_value nix_output_hash_key)
recorded_hash=$(read_value nix_output_hash)

[[ -n $upstream_repository && -n $fork_repository && -n $fork_push_repository ]] ||
  die 'repository provenance is incomplete'

stable_tag=$(
  git ls-remote --tags --refs "$upstream_repository" 'refs/tags/rust-v*' |
    sed -nE 's#^[0-9a-f]+[[:space:]]+refs/tags/(rust-v[0-9]+\.[0-9]+\.[0-9]+)$#\1#p' |
    sort -V |
    tail -n 1
)
[[ -n $stable_tag ]] || die 'upstream exposes no stable rust-vX.Y.Z tag'

temporary_parent=${TIBER_CODEX_TEMP_PARENT:-${TMPDIR:-/tmp}}
clone=$(mktemp -d "$temporary_parent/tiber-codex-update.XXXXXX")
completed=false
on_exit() {
  local status=$?
  if [[ $completed == true ]]; then
    rm -rf "$clone"
  else
    printf 'update-codex: preserved recovery clone at %s\n' "$clone" >&2
    printf 'update-codex: resolve any conflict there, run the reported checks, then rerun just update-codex from a clean Tiber checkout\n' >&2
  fi
  exit "$status"
}
trap on_exit EXIT

git clone --origin fork "$fork_repository" "$clone/codex"
codex_clone="$clone/codex"
git -C "$codex_clone" remote set-url --push fork "$fork_push_repository"
git -C "$codex_clone" remote add upstream "$upstream_repository"
git -C "$codex_clone" fetch --tags upstream "$stable_tag"
upstream_commit=$(git -C "$codex_clone" rev-parse "$stable_tag^{commit}")
remote_fork=$(git -C "$codex_clone" rev-parse "refs/remotes/fork/$fork_branch^{commit}")

verify_signature() {
  if [[ ${TIBER_CODEX_TEST_MODE:-0} == 1 ]]; then
    return
  fi
  git -C "$codex_clone" verify-commit "$1" >/dev/null ||
    die "fork commit $1 does not have a valid signature"
}

pins_are_current() {
  [[ $stable_tag == "$recorded_tag" ]] || return 1
  [[ $upstream_commit == "$recorded_upstream" ]] || return 1
  [[ $remote_fork == "$recorded_fork" ]] || return 1
  git -C "$codex_clone" merge-base --is-ancestor "$upstream_commit" "$remote_fork" || return 1
  verify_signature "$remote_fork"
  grep -Fq "rev = \"$remote_fork\"" crates/tiber-cli/Cargo.toml || return 1
  ! grep -En 'github.com/jwilger/codex\.git.*rev = ' crates/tiber-cli/Cargo.toml |
    grep -Fv "rev = \"$remote_fork\"" >/dev/null || return 1
  grep -Fq "?rev=$remote_fork#$remote_fork" Cargo.lock || return 1
  grep -Fq "outputHashes.\"$recorded_hash_key\" = \"$recorded_hash\"" flake.nix || return 1
}

if pins_are_current; then
  printf 'Codex is already current at %s (%s).\n' "$stable_tag" "$remote_fork"
  completed=true
  exit 0
fi

if [[ ${TIBER_CODEX_TEST_MODE:-0} == 1 && ${TIBER_CODEX_TEST_ALLOW_UPDATE:-0} != 1 ]]; then
  die 'fixture is not current; test mode never updates or pushes repositories'
fi

git -C "$codex_clone" switch main
git -C "$codex_clone" merge --ff-only "$upstream_commit" ||
  die 'fork main is not a clean upstream mirror; repair it in the preserved clone'
[[ $(git -C "$codex_clone" rev-parse HEAD) == "$upstream_commit" ]] ||
  die 'fork main contains commits outside the selected stable upstream tag'

git -C "$codex_clone" switch "$fork_branch"
if ! git -C "$codex_clone" merge --no-ff --no-commit "$upstream_commit"; then
  die "merge conflict while bringing $fork_branch to $stable_tag"
fi

fork_check=${TIBER_CODEX_FORK_CHECK:-'cargo test -p codex-app-server-client --lib host_policy_rejects_requests_before_worker_dispatch &&
cargo test -p codex-app-server-client --lib host_policy_observes_cancellation_before_admission &&
cargo test -p codex-app-server-client --lib default_policy_transparently_admits_distinct_builtin_slash_commands &&
cargo test -p codex-app-server-client --lib host_policy_can_reject_one_exact_builtin_slash_identity &&
cargo test -p codex-app-server-client --lib default_policy_transparently_admits_distinct_builtin_plan_decisions &&
cargo test -p codex-app-server-client --lib host_policy_can_reject_one_exact_builtin_plan_decision &&
cargo test -p codex-app-server-client --lib notification_waits_for_host_admission_before_forwarding &&
cargo test -p codex-tui --lib plan_slash_command_with_args_submits_prompt_in_plan_mode &&
cargo test -p codex-tui --lib slash_side_requests_forked_side_question_while_task_running &&
cargo test -p codex-tui --lib slash_btw_requests_forked_side_question_while_task_running &&
cargo test -p codex-tui --lib plan_implementation_popup_yes_emits_submit_message_event &&
cargo test -p codex-tui --lib plan_implementation_popup_stay_emits_typed_cancel'}
(cd "$codex_clone/codex-rs" && bash -c "$fork_check") || die 'focused Codex fork checks failed'
if ! git -C "$codex_clone" diff --cached --quiet; then
  commit_signing=(--gpg-sign)
  if [[ ${TIBER_CODEX_TEST_MODE:-0} == 1 ]]; then
    commit_signing=(--no-gpg-sign)
  fi
  git -C "$codex_clone" commit --no-signoff "${commit_signing[@]}" \
      -m "chore(codex): merge $stable_tag" \
      -m "Bring the Tiber support branch onto the newest tested stable upstream source."
fi
new_fork=$(git -C "$codex_clone" rev-parse HEAD)
verify_signature "$new_fork"
git -C "$codex_clone" push fork main
git -C "$codex_clone" push fork "$fork_branch"
[[ $(git ls-remote "$fork_push_repository" "refs/heads/$fork_branch" | cut -f1) == "$new_fork" ]] ||
  die 'pushed support branch does not resolve to the tested signed commit'

sed -i -E \
  "/git = \"https:\/\/github.com\/jwilger\/codex.git\"/ s/rev = \"[0-9a-f]{40}\"/rev = \"$new_fork\"/" \
  crates/tiber-cli/Cargo.toml
cargo update -p codex-tui --precise "$new_fork"

new_version=${stable_tag#rust-v}
new_hash_key="codex-agent-extension-$new_version"
placeholder='sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA='
sed -i -E "s#outputHashes\.\"codex-agent-extension-[^\"]+\" = \"[^\"]+\"#outputHashes.\"$new_hash_key\" = \"$placeholder\"#" flake.nix
hash_log="$clone/nix-hash.log"
if nix build --no-link .#tiber >"$hash_log" 2>&1; then
  die 'Nix unexpectedly accepted the placeholder Codex output hash'
fi
new_hash=$(sed -nE 's/^[[:space:]]*got:[[:space:]]+(sha256-[A-Za-z0-9+\/=]+)[[:space:]]*$/\1/p' "$hash_log" | tail -n 1)
[[ -n $new_hash ]] || die "could not extract the Codex output hash from $hash_log"
sed -i "s#$placeholder#$new_hash#" flake.nix

sed -i -E \
  -e "s#^stable_tag = .*#stable_tag = \"$stable_tag\"#" \
  -e "s#^upstream_commit = .*#upstream_commit = \"$upstream_commit\"#" \
  -e "s#^fork_commit = .*#fork_commit = \"$new_fork\"#" \
  -e "s#^nix_output_hash_key = .*#nix_output_hash_key = \"$new_hash_key\"#" \
  -e "s#^nix_output_hash = .*#nix_output_hash = \"$new_hash\"#" \
  codex-source.toml

tiber_check=${TIBER_CODEX_TIBER_CHECK:-'cargo test -p tiber --test embedded_runtime bare_tiber_launches_embedded_codex_without_invoking_path_codex && nix build --no-link .#checks.x86_64-linux.package-smoke'}
bash -c "$tiber_check" || die 'focused embedded-interface or package checks failed'

printf 'Updated Tiber from %s to %s at signed fork commit %s.\n' "$recorded_tag" "$stable_tag" "$new_fork"
completed=true
