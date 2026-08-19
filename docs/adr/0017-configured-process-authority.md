# ADR-0017: Make trusted configuration and signed history authoritative for processes

## Status

Accepted

## Date

2026-08-18

## Context

Tiber needs to run useful repository commands from a conversation without
letting model output choose an executable, arguments, working directory,
environment, deadline, output bounds, network access, or containment policy.
Process interruption also creates the same ambiguity as any other external
effect: redispatch after a crash could duplicate work.

## Decision

Treat the repository owner's `.tiber/commands.toml` as trusted configuration.
It maps semantic command IDs to fixed absolute executables, literal direct
arguments, repository-relative working directories, cleared fixed
environments, deadlines, and stdout/stderr capture bounds. Network access is
always denied. The document is parsed and bounded once when the TUI starts; an
invalid present document fails startup. The model-facing request contains only
the operation name and configured command ID. It never receives or supplies
the execution plan.

Bind every app-server tool-call request identity to a distinct process stream
under the active durable workflow effect. `tiber-process-service` owns the
checked EventCore lifecycle. Request admission atomically publishes
`Requested` and `Prepared`, or a content-free `Refused` fact for an unknown ID.
Only verified matching requested/prepared history and unchanged trusted
configuration can mint the opaque adapter authority. Terminal facts are
`Completed`, `SpawnFailed`, `TimedOut`, `Cancelled`, or `Unknown`;
`Reconciled` records one closed read-only reconciliation result.

Execute only through the x86_64 Linux process adapter's fixed Bubblewrap
profile and private direct-argv launcher. The adapter fixes namespaces, mounts,
network denial, environment clearing, process-group cleanup, output bounds,
and the launcher handshake; neither the model nor a caller can change that
containment. Packaging keeps `tiber-process-launcher` private beside the pinned
Bubblewrap helper under `libexec`, with only `tiber` public.

Raw bounded stdout and stderr exist only long enough to form the immediate
app-server tool result. Durable signed facts and the private full-fsync adapter
journal retain only content-free identities: prepared configuration identity,
exit status or stable terminal category, byte counts, and domain-separated
digests. They never retain raw process output.

Private journal artifacts remain available throughout `Prepared` and
unreconciled `Unknown` authority. Only complete verified history ending in an
exact definitive terminal or `Reconciled` fact can mint the non-cloneable
retirement capability. The adapter then removes only that identity's journal,
reservation, and launcher-handshake artifacts and fsyncs their parent
directory. Retirement is idempotent: startup can finish cleanup after a crash
between signed publication and artifact removal without redispatching or
reconciling the closed lifecycle again.

Signed prepared history never authorizes redispatch after restart. At TUI
startup, the CLI automatically converts otherwise unterminated
`Requested`/`Prepared` history to `Unknown`, consumes the one-shot read-only
capability through the Linux adapter, and publishes `Reconciled`. The public
session projection reports `completed`, `not-completed`, or `still-unknown`.
After every process stream for the active effect is closed or reconciled,
startup records a sanitized inference interruption and advances the enclosing
workflow to a successor; it never replays the interrupted inference. There is
no explicit owner recovery command. The scan considers verified stream
identities and fails closed if more than 64 process streams match the active
effect or if any selected process history exceeds four events. Unrelated
historical streams do not consume the effect-scoped budget.

## Consequences

The repository owner can expose a small named command vocabulary without
granting the model shell or argv construction. Two calls under the same
workflow effect remain independently auditable and cannot borrow each other's
prepared history. Configuration changes between preparation and authorization
fail closed.

Output shown to the model is bounded, sanitized, and ephemeral; durable
recovery can determine only content-free outcomes. An interrupted process is
not automatically retried. Startup reconciliation is automatic and bounded;
an explicit owner-invoked recovery command remains a later integration surface.

## Alternatives considered

- Model-supplied shell text or argv was rejected because it would make
  untrusted inference output the execution authority.
- Persisting raw stdout or stderr was rejected because content is unnecessary
  for recovery authority and expands the durable sensitive-data surface.
- Reusing one effect-wide stream was rejected because distinct tool calls
  could borrow or overwrite each other's lifecycle.
- Redispatching signed preparation after restart was rejected because external
  completion may already have occurred.
