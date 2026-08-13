# ADR 0013: Tiber-only repository cutover

- Status: Accepted
- Date: 2026-08-12

## Context

Tiber began in the `ai-plugins` marketplace beside Claude Code and Codex plugin
surfaces. That layout made the product's authority difficult to understand: a
standalone harness, marketplace bootstrap code, provider-oriented evaluation
assets, and a former task-board implementation shared one repository.

Tiber is now the product. It must own workflow state, task coordination,
inference boundaries, review orchestration, tool execution, isolation, and
delivery without requiring a plugin marketplace or compatibility command
surface. The owner retains copyright in the first-party legacy code and has
authorized its reuse under Tiber's Apache-2.0 product license.

The existing Tiber task board is authoritative EventCore history on the
independent `tiber` Git branch. Its immutable `eventstore/events/*.jsonl`
transactions must survive the repository change byte-for-byte.

## Decision

The private `jwilger/tiber` repository is the sole development home for Tiber.
Its root is a Rust multi-crate workspace. Marketplace manifests, plugin
launchers, provider evaluation runners, and marketplace CI are removed rather
than retained as disabled compatibility surfaces.

We preserve the existing `tiber` authority branch with a normal fast-forward
safe ref transfer. The former `development-workflow` authority is archived as
`archive/ai-plugins-development-workflow`; its repository-specific baseline and
diff evidence is not live state for the new source tree.

First-party legacy source required during the port lives temporarily under
`old-code-for-reference/`. It is excluded from Cargo workspace membership,
CI, packaging, and public command routing. New implementation code is ported
into native `tiber-*` crates, with no compatibility crate, second `tiber`
binary, or legacy top-level task aliases. The default `tiber` command remains
the TUI and task operations live at `tiber tasks …`. The native task surface
provides read-only queries over preserved EventCore history plus only bounded
signed completion operations: acceptance check, occurrence-safe subtask check,
and terminal `transition <ref> done`. They consume opaque command-specific
publication tokens at the Git adapter rather than reopening a generic append,
generic task mutation, or former MCP write surface. Reconciliation and workflow
scheduling remain follow-on work, not compatibility surfaces.

One bounded exception corrects malformed retained task metadata: native Tasks
may append a named, preconditioned duplicate-subtask identity correction. It
targets a specific occurrence and never edits historical transactions or
restores generic legacy task mutation.

## Consequences

- Existing marketplace installations remain end-of-life snapshots on
  `jwilger/ai-plugins`; Tiber makes no update or runtime promise to them.
- Tiber can use legacy task and workflow knowledge without treating plugin MCP
  services as runtime authority.
- Evaluation assets are not carried into the product or CI. A future
  qualitative evaluation strategy will be designed for the finished harness.
- Ported first-party code must receive current Tiber linting, event-model, and
  behavior coverage before it is shipped. The reference tree can be deleted
  after that migration is complete.
