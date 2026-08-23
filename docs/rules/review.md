# Review

Review the behavior and risks, not merely the diff shape. Treat every finding as
a hypothesis to verify against the exact source and specification. Address
confirmed findings, explain rejected findings with evidence, and rerun affected
verification.

Each green increment receives a lightweight fresh-context review for
correctness and overimplementation. Scope completion receives risk-selected
final-review lenses. Final review requires three consecutive complete,
finding-free iterations. Any finding, malformed result, incomplete lens, or
material source delta resets the streak.

Finish source review before creating the delivery commit. Content-identical
staging, commit metadata, or signature changes do not invalidate source review.
