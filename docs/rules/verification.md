# Verification

Evidence must be fresh, relevant, and tied to the exact source snapshot. Run
the narrowest check that can answer the current question, then expand according
to risk. Never claim success from stale output, a different revision, partial
logs, or an adapter's unvalidated assertion.

Local TDD uses focused scenarios and fast checks. Git's hook owns the fast
commit gate. Full acceptance, integration, recovery, package, and mutation
verification runs in CI. Do not run the full CI suite locally merely to mimic
remote delivery.

When verification fails, preserve the diagnostic, identify its causal scope,
fix the cause, and rerun the failed check before broader checks.
