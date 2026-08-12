# Error handling

Expected failures are typed values with a stable machine-readable code,
actionable context, causal chain, and retryability classification. Map external
transport and storage failures at the adapter boundary before they enter the
core.

Reserve panics for unrecoverable programmer defects. Shipping library code does
not use `unwrap`, `expect`, `panic`, `todo`, or `unimplemented`; surface
recoverable failure at an owner-facing boundary instead.
