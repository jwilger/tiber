# ADR 0019: Embed Codex behind typed Tiber host policy

## Status

Accepted.

This ADR supersedes the runtime transport, executable packaging, authentication
handoff, and upgrade portions of ADR-0005 and ADR-0018. Their authority analysis
and historical decisions remain accepted history.

## Context

The external `codex app-server` child and remote TUI gateway proved that Tiber
could hold prompts and effects behind durable authority. It also duplicated
process supervision, protocol transport, sockets, version negotiation,
terminal lifecycle, and authentication paths already implemented by Codex.
Porting native Plan and isolated-conversation behavior through that transport
increased drift.

Codex now exposes generic host-policy seams around its native in-process client,
server requests and notifications, slash actions, cancellation, lifecycle, and
lower-level effects. The default policy preserves upstream behavior and
contains no Tiber domain types.

## Decision

Tiber embeds the native Codex TUI and in-process backend as Rust dependencies
from one exact signed `jwilger/codex:tiber-support` commit based on the newest
reviewed stable upstream tag. `codex_tui::run_main` is invoked directly. Tiber
ships no separate Codex or app-server executable and creates no private Codex
socket or WebSocket gateway.

Tiber installs an application-owned typed host policy that:

- durably admits a prompt before inference and correlates its thread and turn;
- restricts thread start, resume, and fork to read-only/no-network execution;
- registers only Tiber-owned dynamic tools and rejects unknown client/server
  requests that can carry authority;
- intercepts file-write, process, network, and tool effects at the linked core
  boundary while leaving default Codex behavior unchanged outside embedding;
- waits for durable terminal observation before forwarding completion and may
  suppress it on durable failure;
- keeps cancellation and shutdown responsive while policy awaits durable work;
- recovers interrupted inference and prepared effects without replay; and
- admits and observes native `/plan`, `/side`, and `/btw` actions with distinct
  durable authority, including Plan accept/cancel and isolated-child recovery.

Codex retains ownership of subscription authentication, credential storage and
refresh, account selection, streaming, and terminal presentation through its
linked APIs. Credentials do not become Tiber domain data or effect authority.

The tracked `codex-source.toml` is the provenance record. `just update-codex`
uses a tracked script to update the upstream mirror and support branch safely,
run focused fork checks, and update the exact Cargo revision, lockfile, Nix
source hash, stable tag, and upstream/fork commits as one reproducible change.
It never force-pushes and is a no-op when already current.

## Consequences

Users receive the actual Codex interface without an installed `codex` binary,
runtime version match, child app-server, or transport fixture. Tiber policy is
compiled against public typed boundaries instead of JSON envelopes. Clean Nix
packaging contains Tiber and its private effect helpers only.

The fork is a reviewed supply-chain input and its generic seams require focused
upstream tests. Every update must preserve default behavior, policy propagation
through replacement TUI instances, fail-closed Tiber behavior, exact
provenance, and provider-free package smoke evidence. An authenticated
owner-run coding task remains a manual final dogfood gate because deterministic
CI must not consume owner credentials.
