# ADR-0010: Support only x86_64 Linux in v1

## Status

Accepted

## Date

2026-08-10

## Context

Process, filesystem, and network isolation require platform-specific behavior.
A multi-platform v1 would dilute evidence and delay a trustworthy vertical
slice.

## Decision

Test and package only x86_64 Linux for v1. Keep Linux isolation behind a
platform port so future Apple silicon support is not needlessly obstructed.

## Consequences

The initial support promise and clean-machine matrix are precise. Other
platforms are unsupported until they receive equivalent implementation and
evidence.

## Alternatives considered

Immediate Linux/macOS support and platform-neutral isolation claims were
rejected as unproven scope expansion.

## Revisit when

The Linux roadmap is delivered and there is measured demand for another target.
