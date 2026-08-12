# Proportional threat modeling

Tiber v1 is a local, single-owner development harness. The owner, repository,
local environment, installed toolchain, PATH, and explicitly configured local
settings are trusted by default.

Design for ordinary mistakes, model mistakes, malformed external data,
interruption, crashes, stale or corrupt state, partial I/O, ambiguous remote
results, and remote data loss. Do not block local-tool work on malicious local
root, intentional owner bypass, a compromised trusted toolchain, or adversarial
local races unless a later product boundary explicitly brings them into scope.

External tool schemas, model output, process output, remote responses, and
recalled memory are untrusted input. Reduce unnecessary authority and surface
partial failure, reconciliation, and recovery explicitly.
