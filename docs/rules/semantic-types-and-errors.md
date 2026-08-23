# Semantic types and errors

Represent identifiers, revisions, canonical paths, digests, limits, signer
identities, workflow states, and capabilities with types that make invalid
states difficult to construct. Parse strings, JSON, model output, Git output,
HTTP responses, and environment values exactly once at their boundary.

Expected failure uses a Tiber failure carrying a stable code, safe context,
causes, retryability, required recovery evidence, and redaction class. Do not
throw generic errors for expected domain outcomes or leak secrets through
messages, logs, snapshots, or test fixtures.

Unknown data remains `unknown` until validated. A type assertion is not
validation and cannot cross a trust boundary.
