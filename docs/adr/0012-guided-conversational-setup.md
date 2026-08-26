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

Expose one small typed `tiber_setup` tool during an explicitly active,
repository-bound setup conversation. The model may inspect a closed setup
catalog, propose a complete setup plan, or cancel setup. The deterministic host
parses that plan, applies user-global ceilings, validates repository
declarations, and presents its own exact interactive confirmation before any
write or local grant. The model cannot invoke setup effects from an ordinary
conversation, cross repositories, bypass omitted questions, approve its own
proposal, loosen an assurance ceiling, grant a command catalog, or write
arbitrary setup paths.

The setup catalog is generated from shipping semantic settings and declaration
schemas rather than copied prompt prose. Inspection reports current global,
project, effective, and authority values; command-catalog and project-workflow
status; Git and signed task prerequisites; containment evidence; and each
optional external integration capability. Unsupported or externally
provisioned capabilities are reported as explicit blockers with safe
alternatives. Setup never requests secret values in model context and persists
only permitted environment-variable references.

Writes use existing validated atomic settings and authority stores. Generated
`.tiber/commands.json` and `.tiber/workflow.json` declarations are compiled
before persistence. The exact command-catalog digest requires separate
interactive human confirmation before the private local grant is recorded.
The host re-observes authority immediately before writes and refuses a stale
confirmation. After all exact confirmations, it durably records the complete
semantic plan before any configuration effect, observes the resulting state,
and records a digest-bound receipt. Startup reconciles an interrupted confirmed
plan idempotently before reevaluating containment. Tool cancellation is checked
after every interactive boundary and again inside the serialized mutation
boundary. Cancellation before intent persistence leaves the prior valid value
in place; an interruption afterward leaves a recoverable confirmed intent.

An explicit `/tiber-setup` invocation in an interactive trusted project also
opens the only conversational path through configuration-only containment
lockdown. Tiber handles ordinary input before agent startup so lockdown is
proven to stop provider dispatch. For the repository-bound setup conversation,
Pi activates only governed repository reads and `tiber_setup`; automatic
workflow context, memory recall, and Tiber compaction are suppressed, and all
other tools remain
denied. Applying or cancelling restores the ordinary fixed tool inventory and
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
