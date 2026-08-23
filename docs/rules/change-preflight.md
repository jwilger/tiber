# Change preflight

Before changing behavior, identify the user-visible outcome, affected public
boundaries, applicable ADRs and architecture, likely files, risks, and the
narrowest acceptance scenario that can fail for the intended reason. Inspect
existing behavior before proposing implementation.

Classify the change as documentation-only, mechanical, behavioral, or
architectural. Architectural decisions require an ADR and corresponding
`ARCHITECTURE.md` update before implementation. A change that touches an
existing architectural divergence brings the touched behavior into conformance.

Record exclusions and adjacent discoveries. Do not silently widen scope;
future work becomes a provenance-bearing Backlog task once the shared board is
available.
