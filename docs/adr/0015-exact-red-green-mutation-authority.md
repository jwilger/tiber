# ADR 0015: Exact RED–GREEN mutation authority

- Status: Accepted
- Date: 2026-08-16

## Context

Prompt guidance can ask an agent to use test-driven development, but guidance
alone cannot prevent anticipatory product behavior or prove after generation
that a production delta stayed within the failure that authorized it. An
outermost BDD scenario also frequently fails across several plausible causes;
treating that broad failure as unrestricted implementation authority defeats
the purpose of RED–GREEN discipline.

Tiber owns workflow state, repository mutation authority, process execution,
review, and delivery. It can therefore enforce the relationship among observed
failures, authorized source changes, and post-generation review rather than
merely including TDD advice in model context.

## Decision

Every new or changed first-party product behavior is one durable RED–GREEN
increment. Before Tiber may authorize a production mutation, EventCore facts
record:

- the public scenario identity and declared behavioral scope;
- the predicted failure and the exact observed failure evidence;
- either a runtime behavioral failure or a predicted matching compiler
  diagnostic for an intentionally missing type, API, trait implementation, or
  exhaustive case; and
- the active drill-down chain when an outer failure did not isolate one obvious
  cause.

An ambiguous outer failure grants no mutation authority. Tiber authorizes
progressively narrower test-only changes until one leaf RED has a single
predicted cause. Only that leaf evidence can mint an opaque repository-mutation
authorization, and that authorization is bound to the leaf scenario and
behavioral scope.

After generated production changes, Tiber must assign an independent
fresh-context exact-failure-conformance review. The reviewer receives the RED
evidence, drill-down chain, declared scope, and complete source delta. A clean
typed result is required before GREEN may be accepted and before the workflow
can begin another RED, verification, final review, or delivery. Any finding that
the production delta adds behavior beyond the exact leaf failure invalidates
the mutation result and requires remediation or replay from RED.

Simple development-environment scripts, CI workflow changes, behavior-covered
refactors, and pure removals use explicit typed exemptions. An exemption never
authorizes a tautological test that reads committed source, documentation,
policy, manifests, or workflow text merely to assert expected text exists.

All durable decisions and review results are command-specific EventCore facts,
registered with experimental modeling and model-check facilities. Read
projections never grant mutation or phase-transition authority.

## Consequences

- TDD discipline is a mechanical workflow precondition and post-generation
  review gate, not only prompt context.
- Broad BDD outcomes remain end-to-end acceptance evidence while narrow REDs
  constrain each implementation step.
- Expected compiler failures can start an increment without weakening the
  requirement to predict and match the exact diagnostic.
- Tiber must preserve enough evidence to resume the active drill-down and
  conformance review after restart without silently widening scope.
- While development still runs under advisory Codex tooling, contributors must
  apply the same protocol manually and treat conformance-review findings as
  blocking; full mechanical enforcement begins when Tiber owns mutation and
  workflow execution.
