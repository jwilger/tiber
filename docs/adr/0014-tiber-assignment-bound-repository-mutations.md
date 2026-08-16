# ADR-0014: Separate assignment-bound repository authority from Linux execution

## Status

Accepted

## Date

2026-08-14

## Context

Tiber must eventually let an assigned agent make a small, reviewable set of
repository changes while retaining Tiber as the authority for identity, policy,
receipts, recovery, and delivery. A generic filesystem API or shell runner
would make the proposed operation, authorization scope, and recovery semantics
unbounded before Linux isolation and durable recovery exist.

`tiber-store-git` already has a deliberately narrow mutation role: it publishes
signed EventCore facts to the fixed `tiber` authority branch with an exact-base
compare-and-swap. That authority-branch publisher is not a repository working
tree mutation service and must not become one.

## Decision

Introduce `tiber-repository-core` as a pure, unconnected authority boundary
for assignment-bound repository file mutations. Its closed operation vocabulary
is:

- write a repository file with either an absent-file or exact-digest
  precondition; and
- delete a repository file with an exact-digest precondition.

The core mints opaque authorization, models typed `RepositoryMutationReceipt`
and `RepositoryMutationFailure` values, and derives a read-only
`RepositoryReconciliation` handle. Its runtime-neutral `RepositoryService` port
accepts only those opaque mutation and reconciliation values; it has no
operational request-options, timeout, or cancellation surface. The core
performs no filesystem, Git, process, or network I/O; it does not add an
EventCore fact, workflow effect, runner, scheduler, CLI, TUI, or app-server
integration.

`RepositoryMutationProposal` receives authority only through
`authorize_mutation`: its complete `RepositoryMutationProvenance`, repository
identity, and component-aware `RepositoryAssignmentContext` must agree with the
trusted `RepositoryMutationPolicy`, and a `RepositoryMutationApproval` bound to
that exact safe proposal identity and policy/assignment context is mandatory.
The resulting `AuthorizedRepositoryMutation` is opaque, so a raw proposal
cannot reach a future adapter.

Each mutation has a stable identity for reconciliation. Any unknown mutation
outcome requires read-only reconciliation by that identity, not automatic
replay. A future layer may make a new explicit decision after reconciliation,
but this boundary never converts uncertainty into another mutation.

The boundary is not a generic filesystem capability or arbitrary shell-command
runner. `tiber-store-git` remains limited to its signed `tiber`
authority-branch publication contract and is never generalized into this
repository mutation authority.

S2 interprets only the bounded authorizations through
`tiber-repository-linux`, an x86_64 Linux `RepositoryService` adapter with
filesystem, process, and network isolation. It starts a fixed, private
`tiber-repository-worker` under Bubblewrap; neither model nor caller can supply
shell text, arbitrary argv, cwd, environment, mount, or network configuration.
The adapter owns operational timeout, cancellation, child cleanup, and typed
non-durable outcomes. It is not a generic `ProcessService`, shell runner, or
workflow integration.

S3 adds a private, pinned `eventcore-fs` receipt journal outside the repository
in an owner-only state root, using full file and directory fsync. It records
`Prepared` before the worker receives mutation bytes and durable terminal
`Applied`, `Failed`, or `Unknown` facts afterward. Restart scans return only
read-only ambiguity-derived reconciliation handles, never mutation authority or
automatic replay. The journal rejects corrupt, dangling, forked, or stale state
fail-closed, and the cooperative state-root then worker-lock order coordinates
concurrent owners. These facts make the journal durable; they do not claim
durability of the working-tree filesystem beyond the recorded facts.

The clean x86_64 Linux package exposes public `tiber` and keeps the worker plus
Bubblewrap helper private under `libexec`. CI's package smoke validates that
artifact layout and entry behavior only; real adapter behavior is tested
separately outside that package smoke.

## Consequences

S1 established precise semantic types and observable pure-core behavior without
claiming that Tiber had changed a repository. S2 now supplies the replaceable
isolated adapter for those bounded operations, with timeout, cancellation, and
child-cleanup controls. S3 adds the private receipt-journal, read-only recovery,
lock-ordering, and package-layout evidence without making the journal an
EventCore workflow integration or a general filesystem durability guarantee.
The boundary keeps the authority distinction between a working-tree mutation and
an EventCore authority-branch publication explicit.

S1 intentionally deferred execution, cancellation, timeout, retry bounds,
platform isolation, durable receipts, restart recovery, and packaging. S2
delivers isolated execution controls and typed non-durable outcomes; S3 adds
journal-backed recovery and package-layout evidence. An ambiguous mutation
requires reconciliation before any later fresh attempt, so callers cannot rely
on an implicit retry for a potentially applied write.

## Alternatives considered

- A generic filesystem or shell runner was rejected because it grants more
  authority than the assignment-bound operations and couples policy to an
  unbounded execution surface.
- Reusing or generalizing `tiber-store-git` was rejected because its signed
  authority-branch publication protocol is distinct from working-tree file
  mutation.
- Delivering the pure core and Linux executor together was rejected because it
  would couple the durable operation vocabulary to platform details before the
  contract can be proven independently.
- Automatically replaying an unknown mutation outcome was rejected because it
  can turn an ambiguous partial outcome into data loss, an unintended overwrite,
  or an unintended deletion.

## Revisit when

The bounded operation vocabulary cannot express a measured assigned-workflow
need, or a future platform receives equivalent isolation and recovery evidence.
Any expansion must preserve assignment-bound opaque authority, exact
preconditions, and reconciliation-before-replay semantics; it must not reopen a
generic filesystem, shell-runner, or authority-branch publication surface.
