# Workflow and commits

Tiber delivers directly to `main` unless the owner explicitly selects a
different mode. Work in one focused vertical increment, run the relevant local
gate, perform final review, and keep CI evidence bound to the exact pushed
revision.

## Final-review delivery boundary

Final review applies to the final source-content snapshot. Once that review is
clean, create the signed commit, run the required post-commit gate against the
exact commit, then push and confirm CI.

Changing only the Git staging partition, `HEAD`, commit metadata, or signature
does not change the reviewed source content and must not start an administrative
re-review loop. Preserve stage-aware hashing while a review is active, so real
staged, unstaged, or untracked source changes remain detectable.

Restart final review only when reviewed paths, contents, modes, untracked
content, pinned baseline, or requested scope changes. Commit-message and
signature checks are delivery verification; they do not invalidate a completed
source review. The post-commit gate remains mandatory because it binds the
repository checks to the signed commit that will be delivered.

## Commit and push rules

- Never disable commit signing.
- Every authored commit has a concise Conventional Commit subject and a
  non-empty body explaining motivation, decision context, tradeoff, or the
  failure prevented.
- Do not add `Co-Authored-By` or other AI-attribution trailers.
- Never force-push without explicit case-by-case owner authorization.
- If pushed CI fails, diagnose that exact revision and repair or rerun it before
  starting unrelated work.
