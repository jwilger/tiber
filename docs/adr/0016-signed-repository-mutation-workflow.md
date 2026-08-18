# ADR-0016: Make signed EventCore history authoritative for repository mutation

## Status

Accepted

## Date

2026-08-17

## Context

ADR-0014 established a deliberately unconnected pure repository authority, a
fixed Linux adapter, and a private operational receipt journal. Tiber now needs
the first usable conversation-to-working-tree vertical slice without letting a
model proposal, presentation state, or adapter journal become business
authority.

## Decision

Connect exact repository write proposals through a new
`tiber-repository-service` domain using EventCore 2.0.1 command-specific models.
The signed authority branch records proposal or reproposal, explicit owner
approval/denial/cancellation, preparation, terminal outcome, and reconciliation.
Every shipping command participates in the experimental checked-model graph and
must report `Verified` with no provenance warnings.

A model tool call is inert structured input. Tiber rereads the canonical
root-relative target and constructs the actual diff. A changed preimage cannot
inherit approval: Tiber records an exact-digest reproposal and requires another
explicit decision. Denial and cancellation never reach the adapter.

Preparation is a two-phase authority boundary. The pure command first yields a
content-free `Prepared` publication. Only after that fact is signed and verified
may the core consume the exact raw proposal and approval to mint opaque
`AuthorizedRepositoryMutation`. The shell then interprets that value through
the existing fixed Bubblewrap worker; neither model nor owner input supplies
shell text, argv, environment, mounts, network configuration, or worker paths.

On restart, signed `Prepared` without a terminal fact yields only a read-only
reconciliation handle. The adapter may consult its private journal as
operational evidence, but that journal cannot trigger recovery independently of
signed history. Reconciliation never redispatches. Tiber signs exactly one
`Reconciled` fact, after which later restarts perform no additional query.

## Consequences

Owners can inspect an actual diff, approve, deny, or cancel it, and observe a
durable outcome through the packaged Tiber conversation. Stale working-tree
state is fail-closed and requires reproposal. A crash after signed preparation
is recoverable without replay.

The current public slice remains intentionally narrow: exact bounded file
writes only, one owner, one repository, and the existing x86_64 Linux adapter.
It does not create a generic filesystem API, shell runner, or model-executable
tool surface. The private Linux journal remains an adapter implementation detail
and does not duplicate EventCore business authority.

## Alternatives considered

- Treating the Linux receipt journal as the workflow authority was rejected
  because it would bypass signed task-bound history and split business truth.
- Dispatching before signed `Prepared` was rejected because a crash could leave
  an unrecorded mutation with no authoritative recovery trigger.
- Automatically retrying after restart was rejected because an uncertain write
  could be applied twice or overwrite newer bytes.
- Persisting raw replacement content in signed facts was rejected because safe
  identity, digest, and receipt data are sufficient for authority and audit.
