# Semantic types and errors

Represent identifiers, revisions, canonical paths, digests, limits, signer
identities, workflow states, and capabilities with types that make invalid
states difficult to construct. Parse strings, JSON, model output, Git output,
HTTP responses, and environment values exactly once at their boundary.

Represent legitimate absence in domain models with `Option<T>`, not optional
properties, `undefined`, or `null`. Represent operations that can fail as
`Result<T, TiberFailure>`. Use railway combinators only where they preserve
visible closed failure and effect types; authorization and consequential-effect
stages must remain explicit and cannot discard the failure rail.

Expected failure uses a Tiber failure carrying a stable code, safe context,
causes, retryability, required recovery evidence, and redaction class. Do not
throw generic errors for expected domain outcomes or leak secrets through
messages, logs, snapshots, or test fixtures.

Unknown data remains `unknown` until validated. A type assertion is not
validation and cannot cross a trust boundary.
