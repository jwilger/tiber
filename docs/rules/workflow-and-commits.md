# Workflow and commits

Tiber delivers directly to `main` unless the owner explicitly selects a
different mode. Work in one focused vertical increment, let Git run the tracked
Lefthook pre-commit gate, perform final review, and keep CI evidence bound to
the exact pushed revision. `just ci` is remote-CI-only and must not be used as a
local pre-commit, post-commit, or delivery gate. Agents must not manually run
Lefthook, the installed pre-commit entrypoint, or its component commands as
delivery verification. Proceed to `git commit`; Git owns hook invocation. A
hook failure rejects the commit and must be repaired before retrying.

## Exact RED–GREEN authority

Every new or changed first-party product behavior begins with one executable
test through a supported public boundary. The test must first fail because the
specified behavior is missing. That exact observed failure is the complete
implementation authority for the increment: production code may address only
that failure and must not add adjacent, anticipatory, or merely convenient
behavior. After the focused scenario becomes green, perform a fresh-context
lightweight review before defining the next failing scenario.

An expected compiler error is valid RED evidence when the scenario
deliberately requires a missing type, API, trait implementation, or exhaustive
case and the observed diagnostic is the one predicted before compilation.
Incidental syntax, fixture, dependency, or setup failures are not RED evidence.
When compilation is the RED boundary, the production delta may resolve only
that expected diagnostic and the scenario must then execute far enough to
expose its next intended behavioral observation.

An outer BDD scenario commonly fails across more than one plausible cause. If
its observation does not identify one obvious missing behavior, it grants no
production-change authority yet. Drill down through progressively narrower
behavioral boundaries, without editing production code, until one focused RED
has a single predicted cause. Fix only that leaf failure, then rerun outward
through every drill-down scenario to the original BDD outcome. A new failure at
any level starts another drill-down cycle; it does not widen the prior fix.

Simple development-environment scripts and CI workflow changes may be exempt
when executing the script or remote workflow is the meaningful evidence.
Refactors with adequate existing behavioral coverage and pure removal follow
their documented applicability rules. Never create a test that reads committed
source, documentation, policy, manifests, or workflow text merely to assert
that expected text exists.

Tiber's durable workflow must enforce the same boundary: implementation
authority requires recorded RED evidence identifying the public scenario and
its exact runtime assertion or expected compiler diagnostic; GREEN evidence
must identify the same scenario and prove
that failure is resolved. A production delta outside that scenario's declared
behavioral scope cannot advance to the next increment, verification, review,
or delivery. Exempt work records the applicable exemption instead of fabricated
RED evidence.

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
