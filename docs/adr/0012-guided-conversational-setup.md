# ADR 0012: Guided conversational setup

Status: Accepted

## Context

Loading Tiber in a repository with no Tiber state currently exposes diagnostics
and low-level settings commands, but it does not establish a usable project.
Users must discover every setting, declaration, authority boundary, and external
prerequisite, then translate those discoveries into commands and JSON edits.
That recreates the operator-driven bootstrap problem rejected by ADR 0011.

A setup assistant needs enough information to explain every supported option and
enough capability to persist valid choices. Giving the model ordinary file or
shell authority during bootstrap would let inference become configuration
authority and would weaken Tiber's fail-closed boundary.

## Decision

Ship one extension command, `/tiber-setup`, backed by a packaged setup-agent
prompt, as the ordinary setup and reconfiguration entry point. The internal
prompt is not exposed as a second user command. It conducts a normal multi-turn
conversation: it inspects first, explains one configurable area at a time,
recommends a safe value from observed repository facts, never asks for secret
material, presents a complete preview, and obtains explicit user approval.

Expose one small typed `tiber_setup` tool. The model may inspect a closed setup
catalog and propose a complete setup plan. The deterministic host parses that
plan, applies user-global ceilings, validates repository declarations, and
presents its own exact interactive confirmation before any write or local
grant. The model cannot bypass omitted questions, approve its own proposal,
loosen an assurance ceiling, grant a command catalog, or write arbitrary setup
paths.

The setup catalog is generated from shipping semantic settings and declaration
schemas rather than copied prompt prose. Inspection reports current global,
project, effective, and authority values; command-catalog status; Git and signed
task prerequisites; containment evidence; and optional external integrations.
Unsupported or externally provisioned capabilities are reported as explicit
blockers with safe alternatives. Setup never requests secret values in model
context and persists only permitted environment-variable references.

Writes use existing validated atomic settings and authority stores. A generated
`.tiber/commands.json` is compiled before persistence, and its exact canonical
digest requires separate interactive human confirmation before the private
local grant is recorded. Cancellation leaves the prior valid value in place or
a valid but ungranted declaration that setup can resume safely.

An explicit `/tiber-setup` invocation also opens the only conversational path
through configuration-only containment lockdown. For that bounded conversation,
Pi activates only governed repository reads and `tiber_setup`; all other tools
remain denied. Applying a plan restores the ordinary fixed tool inventory and
immediately re-evaluates containment. Setup mode itself grants no mutation,
command, secret, or workflow authority.

The package must already be loaded through Pi's normal installed, project-local,
or temporary package mechanism. Package installation and project trust remain
Pi-owned boundaries; Tiber does not add a launcher or install itself.

## Consequences

A user invokes one command and remains in conversation instead of editing JSON
or learning transition syntax. Setup is repeatable and can modify an existing
configuration. The prompt helps with choices, while schemas and human
confirmation—not inference—authorize effects.

External secret provisioning, strong-containment attestation, and third-party
service administration remain genuine external boundaries. Setup diagnoses and
explains them but does not pretend to provision authority it cannot verify.
