# Testing

Tests assert observable behavior through public boundaries, including failure,
cancellation, restart, and recovery behavior where the slice can encounter
them. Do not test source text or private implementation details as a proxy for
behavior.

Keep deterministic tests credential-free. Protocol and process boundaries use
local fixtures; live integrations are opt-in and never part of the required CI
gate. Add property or state-machine tests where a pure core has meaningful
invariants.
