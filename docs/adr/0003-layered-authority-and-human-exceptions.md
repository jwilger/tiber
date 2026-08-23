# ADR 0003: Layered authority and human exceptions

Status: Accepted

## Context

Repository content is controlled by the code being inspected and cannot grant
itself host authority. Broad model-visible approval tokens are replayable and
unsafe.

## Decision

Resolve settings from project explicit, global explicit, and built-in default,
then apply restrictive global locks and the Tiber floor. Keep project trust and
secret references user-local and bind them to generated repository identity,
canonical Git common directory, and expected remotes. Human exceptions freeze
one exact operation and bind it to task, run, revision, paths, preimages,
arguments, expiry, and one use.

## Consequences

Repository declarations can narrow but never broaden permission. Models cannot
mint, approve, possess, or replay exceptions. Tightening is immediate;
loosening requires a new or explicitly rebound run.
