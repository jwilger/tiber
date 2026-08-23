# ADR 0005: In-process workers and cache epochs

Status: Accepted

## Context

Autonomous work needs isolated roles without a daemon or nested `pi` processes,
and provider token caching depends on stable request prefixes.

## Decision

Keep the visible Pi session as coordinator and use isolated in-process Pi agent
sessions for workers. Give each role typed input/output, fixed tools, one
bounded initial context pack, and hard budgets. Keep prompts, initial context,
tool schemas, and ordering byte-stable within a cache epoch. Append dynamic
state only as suffix messages; compaction deliberately creates a new epoch.

## Consequences

Workers request effects but cannot execute them. Missing configured model routes
block rather than silently substituting. Context summaries are advisory and
retain provenance to originals.
