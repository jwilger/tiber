# CI and delivery

Git delivery, CI authority, and review service are separate. A push receipt is
not CI success, and a forge permission is not Git authority. Every required CI
provider must report terminal success for the exact delivered revision.

Local commits use the fast hook gate; pushes have no heavy local hook. Full
verification runs remotely and superseded revisions may be cancelled. A
terminal CI failure creates a repository-wide delivery hold until causally
resolved; do not evade it with unrelated work or retries without a diagnosis.

Release automation maintains a release PR. A human explicitly merges that PR.
Only then may immutable tagging, GitHub Release creation, and OIDC/provenance
npm publication proceed.
