# Workflow and commits

Tiber delivers directly to `main` unless the owner explicitly selects a
different mode. Work in one focused vertical increment, let Git run the tracked
Lefthook pre-commit gate, perform final review, and keep CI evidence bound to
the exact pushed revision. `just ci` is remote-CI-only and must not be used as a
local pre-commit, post-commit, or delivery gate. Agents must not manually run
Lefthook, the installed pre-commit entrypoint, or its component commands as
delivery verification. Proceed to `git commit`; Git owns hook invocation. A
hook failure rejects the commit and must be repaired before retrying.

## Final-review delivery boundary

Final review applies to the final source-content snapshot. Once that review is
clean, create the signed commit, allowing Git to run Lefthook, then push and
confirm the full remote CI gate against the exact revision.

Changing only the Git staging partition, `HEAD`, commit metadata, or signature
does not change the reviewed source content and must not start an administrative
re-review loop. Preserve stage-aware hashing while a review is active, so real
staged, unstaged, or untracked source changes remain detectable.

Restart final review only when reviewed paths, contents, modes, untracked
content, pinned baseline, or requested scope changes. Commit-message and
signature checks are delivery verification; they do not invalidate a completed
source review. Do not rerun `just ci` after committing. Remote CI is the
authoritative full-gate evidence bound to the pushed commit.

## Commit and push rules

- Never disable commit signing.
- Every authored commit has a concise Conventional Commit subject and a
  non-empty body explaining motivation, decision context, tradeoff, or the
  failure prevented.
- Do not add `Co-Authored-By` or other AI-attribution trailers.
- Never force-push without explicit case-by-case owner authorization.
- If pushed CI fails, diagnose that exact revision and repair or rerun it before
  starting unrelated work.
