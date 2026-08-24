# Security policy

Report vulnerabilities privately through GitHub's security advisory interface
for `jwilger/tiber`. Do not open a public issue containing an unpatched
vulnerability, credential, private path, or sensitive artifact.

## Supported versions

After the human-gated `1.0.0` release, the latest `1.x` release is supported.
Security fixes are delivered through signed pull requests, required CI, a
human-merged release PR, GitHub provenance, and OIDC npm publication. Older
minor releases may be asked to upgrade before receiving a fix.

Tiber requires Node.js 22.23.1 through the Node 22 line and stock Pi 0.84.2 or
newer before Pi 1.0. Tiber is not a sandbox: strong containment modes verify
externally provisioned isolation rather than creating it. Models cannot grant
capability, approve readiness, weaken policy, or mint human exceptions. The
normative guarantees and trust boundaries are documented in `ARCHITECTURE.md`.

When reporting an issue, include the Tiber, Pi, Node, and operating-system
versions; the safe failure code; and minimal reproduction steps. Redact secrets,
raw private artifacts, source not needed for reproduction, and personal data.
