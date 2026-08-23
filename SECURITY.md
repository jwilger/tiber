# Security policy

Please report security vulnerabilities privately through GitHub's security
advisory interface for `jwilger/tiber`. Do not open a public issue containing an
unpatched vulnerability or secret.

Tiber is not a sandbox. Its strong containment modes verify externally
provisioned isolation; they do not create it. The bootstrap release is
read-only and not yet a complete authority boundary. Supported guarantees are
documented in `ARCHITECTURE.md` and release notes.
