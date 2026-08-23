# Workflow and commits

Work in independently valuable vertical increments. Preserve each accepted
green increment according to the configured delivery mode. Keep unrelated
changes separate and never hide repository state from review.

Every authored commit is signed. Use a concise Conventional Commit subject and
a non-empty body explaining why the change exists. Do not disable hooks or
signing and do not add AI-attribution trailers.

All changes reach `main` through pull requests. Use squash merge with a
Conventional Commit-compatible PR title. Direct and force pushes to protected
branches are prohibited. Ordinary PRs may auto-merge after all gates when the
author is permitted; release PRs require explicit human merge.
