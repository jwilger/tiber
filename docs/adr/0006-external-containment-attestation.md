# ADR 0006: External containment attestation

Status: Accepted

## Context

An in-process extension can govern requested effects but cannot honestly prove
that authorized project code is sandboxed or provision its own enclosing
isolation.

## Decision

Expose `host-trusted`, `workspace-isolated`,
`workspace-and-network-isolated`, and `hermetic` assurance levels. Strong levels
require an external attestation plus local corroborating checks. Define the
protocol but do not provision containers, namespaces, or VMs. Verify strong
levels first on Linux. Failed requirements enter configuration-only lockdown by
default, with optional graceful shutdown.

## Consequences

Unsupported platforms fail closed for strong requirements. Local heuristics
alone never assert isolation. Stock Pi must support pre-inference abort and
complete executable-extension inventory for governed mode.
