# ADR 0013: Usable autonomy and permission gates

Status: Accepted

Supersedes: ADR 0012; the generic-CI-command and distinct-GitHub-token decisions of ADR 0008

## Context

ADR 0012 made setup complete but exposed Tiber's internal authority model as a
long interrogation. A person who installed Tiber to obtain a governed software
development workflow had to understand layered settings, containment
attestations, command catalogs, digest grants, CI adapters, and separately
provisioned GitHub token capabilities before the repository was usable. The
result was more difficult than an ordinary coding agent and left no supported
way for setup to create required CI authority.

Requiring an exhaustive command catalog before work also confuses two kinds of
authority. Tiber must deterministically enforce workflow and role ceilings, but
an eligible agent still needs a usable way to request ordinary repository
operations. A first-use permission can authorize such an operation without
letting inference authorize itself or bypass the immutable workflow floor.

Users already configure Git credentials, signing, and GitHub CLI authentication.
Duplicating those credentials in Tiber-specific environment variables adds
friction without strengthening the boundary. The host can invoke those clients
without exposing credentials to a model.

## Decision

Replace conversational catalog interrogation with a deterministic guided setup
that presents a short series of plain-language choices. The primary choices
are autonomy and isolation:

- **Ask first** prompts before any consequential operation not required merely
  to inspect the current repository.
- **Routine autonomy** performs recognized low-risk development operations and
  asks before unfamiliar, destructive, boundary-crossing, publication, or
  authority-changing effects. This is the recommended default.
- **Repository autonomy** performs eligible operations inside the trusted
  repository without first-use prompts, but still asks at publication,
  privilege, external-resource, arbitrary-shell, and policy-exception
  boundaries.

Isolation is selected independently:

- **Use this repository** applies deterministic path, role, workflow, and
  permission enforcement without claiming OS isolation. This is the default.
- **Require an isolated workspace** requires matching external workspace
  isolation evidence.
- **Require workspace and network isolation** additionally requires network
  isolation evidence.
- **Require a hermetic environment** additionally requires the strongest
  supported process filtering evidence.

The ordinary path recommends the two defaults, explains consequences in user
terms, and discovers the repository's existing tools. Internal settings remain
available through explicit diagnostics and recovery surfaces rather than being
forced into ordinary setup. Setup remains a
typed, repository-bound, human-confirmed, journaled operation. Models may help
explain choices but do not grant setup authority.

Every requested effect is evaluated in this order:

1. immutable workflow policy floor;
2. agent-role capability ceiling;
3. repository and path boundary;
4. built-in deterministic risk policy;
5. exact repository-local remembered permission;
6. interactive human decision when the effect remains undecided.

An earlier denial cannot be weakened by a later stage. Interactive decisions
are **Deny once**, **Always deny**, **Allow once**, and **Always allow**.
Persistent decisions are private, repository-bound, audited, and keyed by a
host-derived semantic action scope. Models cannot choose the scope, observe a
replayable approval token, or persist a decision. Persistent allow is not
available for arbitrary shell, privilege escalation, force operations,
publication, exceptions, or actions whose arguments cannot be safely scoped.
Those actions require an exact single-use approval whenever the role and
workflow permit them at all.

Planning, readiness, review, setup, and semantic-classifier agents have no
arbitrary process capability. Implementation agents request shell-free
executable-plus-argv operations. Tiber resolves the executable from the host;
the model does not supply an executable path. Invoking a shell interpreter or
using shell text is a separately classified arbitrary-shell effect and always
requires exact interactive permission when the role permits it. Delivery and
CI agents use dedicated Git and forge effects bound to repository, ref,
revision, and workflow state. Permission never overrides these role ceilings.

Tiber uses the user's ordinary `git` configuration, signing setup, credential
helpers, SSH agent, and environment. For GitHub repositories it uses an
installed and authenticated `gh` client. These adapters receive the host
process environment as needed but never return credential material to model
context, logs, receipts, or persisted Tiber state. Consequential Git and GitHub
operations remain typed effects and pass the same deterministic workflow and
permission decisions.

Guided setup detects the Git remote, installed `git` and `gh`, GitHub
authentication, repository manifests, and GitHub Actions. For a standard GitHub
repository it creates the private validated CI authority catalog itself using
the shipping first-party GitHub Actions observer and the human-approved set of
required checks. The catalog pins the Tiber adapter implementation and
repository identity; it does not require a user-authored executable or
Tiber-specific GitHub token variables. Generic third-party CI adapters remain
an advanced option and retain digest-pinned executable/argv validation.

The runtime has no install-time native dependency. Tiber may learn UX patterns
from permission extensions, including mode selection and first-use prompts, but
its authorization decisions remain typed, deterministic, role-aware, and
integrated with the workflow floor.

## Consequences

A normal installation can become usable by accepting recommended autonomy and
isolation choices. Users do not predeclare every command or duplicate existing
Git/GitHub credentials. Unfamiliar eligible operations prompt at the moment the
user has enough context to decide, and remembered decisions reduce repeat
friction.

Tiber still cannot claim that host-trusted execution is sandboxed. Strong
isolation remains externally evidenced. Tiber also does not turn permissions
into exceptions: immutable workflow, role, revision, claim, review, and release
boundaries remain deterministic and fail closed.

The ADR 0012 setup plan, mandatory catalog-by-catalog conversation, exact
project command-catalog grant, and externally provisioned-only CI catalog are
obsolete. The affected portions of ADR 0008 requiring generic executable CI
commands and separate Tiber-specific GitHub credentials are obsolete. Its
separation of Git transport, CI evidence, review evidence, and merge authority
remains active.
