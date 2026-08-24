# Review

Review the behavior and risks, not merely the diff shape. Treat every finding as
a hypothesis to verify against the exact source and specification. Address
confirmed findings, explain rejected findings with evidence, and rerun affected
verification.

Each green increment receives a lightweight fresh-context review for
correctness, overimplementation, and semantic-type integrity. Reviewers must
identify primitive obsession: domain identifiers, revisions, digests, paths,
limits, identities, states, capabilities, or failures represented as
interchangeable primitives after boundary validation. A merely structural
wrapper such as `IsoTimestamp`, `AbsolutePath`, `GitRevision`, or `ByteLimit`
does not satisfy this rule: types are named for their domain purpose so that,
for example, claim occurrence time cannot be supplied as attestation expiry,
a worktree path cannot be supplied as a repository path, and a claim baseline
cannot be supplied as a delivered revision. Shared structural parsing may be
reused privately, but its outputs must be reified as distinct purpose-specific
semantic types. Reviewers must also reject
repeated parsing inside the domain, assertions used as validation, generic
expected-error throws, nullable or optional domain fields instead of
`Option<T>`, fallible operations outside `Result<T, TiberFailure>`, discarded
failure rails, and broad stringly typed authority records.

Scope completion receives risk-selected final-review lenses, including a
mandatory semantic-types-and-errors lens. Final review requires three
consecutive complete, finding-free iterations. Any finding, malformed result,
incomplete lens, or material source delta resets the streak.

Finish source review before creating the delivery commit. Content-identical
staging, commit metadata, or signature changes do not invalidate source review.
