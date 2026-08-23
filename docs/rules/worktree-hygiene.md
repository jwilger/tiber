# Worktree hygiene

Mutating tasks use owned branches and worktrees unless an explicit trusted
current-worktree policy says otherwise. Canonicalize and validate every path
before creation, access, or cleanup. Track owned processes and terminate their
process groups on cancellation or shutdown.

Never delete a foreign, ambiguous, actively claimed, or non-canonical path.
Before removing abandoned uncommitted source, preserve it under a bounded
private local recovery ref. Generated and ignored files may be discarded.
Recovery refs are never pushed automatically.

A task is not Done until its claim and owned worktree are resolved. Stale
heartbeat is evidence for human takeover, not automatic ownership transfer.
