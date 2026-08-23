# Contributing

Read `AGENTS.md`, the active ADRs, `ARCHITECTURE.md`, and the rule relevant to
your change before implementation.

All changes use pull requests. Use a Conventional Commit-compatible PR title.
Keep each commit signed and give it a concise Conventional Commit subject plus
a non-empty body explaining why the change exists. Ordinary PRs may auto-merge
after required review and CI gates when the author has permission. Release PRs
always require an explicit human merge.

Use focused tests while developing. Git's pre-commit hook runs the fast gate;
full verification runs in CI. Do not bypass signing, hooks, branch protection,
or Tiber policy.

Unless stated otherwise, contributions are intentionally submitted under both
MIT and Apache-2.0 terms.
