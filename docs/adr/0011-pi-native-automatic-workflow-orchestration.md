# ADR 0011: Pi-native automatic workflow orchestration

Status: Accepted

## Context

Tiber's first workflow surfaces required a human to translate ordinary intent
into a sequence of `/tiber:*` commands. Worse, bootstrap policy could deny the
shell before that human had established the task authority needed to use it.
This made Tiber an operator-driven state machine and created a dogfooding
bootstrap deadlock.

Pi already has a normal agent loop, typed tools, lifecycle events, and extension
context. Users should describe outcomes and steer work in normal conversation;
they should not need to know Tiber's transition command vocabulary.

## Decision

Expose one small, stable, typed workflow-request tool to Pi. The model may use it
to request a semantic operation inferred from normal conversation. A request is
untrusted input, not authority. The extension parses it once and passes it to
the deterministic workflow host, which reads signed state and either performs
the already-authorized transition or returns a typed denial and compliant next
step.

Keep slash commands as optional inspection, diagnostics, and explicit recovery
surfaces. They are not required on the ordinary path. The bootstrap policy
always permits the effect-free request surface and the reads needed to discover
workflow state. It never permits repository mutation merely because the model
requested it.

The host automatically advances deterministic transitions when their evidence
is complete. It asks a human only at an accepted human boundary: project trust,
claim takeover, exact exception approval, authority loosening, or release-PR
merge. Model inference may select or describe intended work but cannot approve
readiness, grant a claim, weaken policy, mint an exception, or authorize an
effect.

## Consequences

Normal Pi conversation can establish and follow a governed workflow without
manual command choreography. Invalid, ambiguous, stale, or unauthorized
requests fail closed. Request and resulting receipt remain distinct audit
facts. Bootstrap remains usable without opening an ungoverned mutation path.
