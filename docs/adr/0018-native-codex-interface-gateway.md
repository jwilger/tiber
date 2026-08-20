# ADR 0018: Native Codex interface through a Tiber policy gateway

## Status

Accepted.

## Context

Tiber's extracted Ratatui projection proved durable workflow behavior, but it
did not provide the Codex terminal experience. Reimplementing Codex rendering,
keyboard behavior, composer state, history, and future UI changes would create
permanent presentation drift. Connecting Codex TUI directly to app-server would
instead let effect-bearing protocol requests bypass Tiber's durable authority.

Codex 0.147.0 provides a reviewed `--remote unix://…` client boundary and a Unix
socket app-server transport. Its wire envelopes are JSON-RPC-shaped but do not
require a `jsonrpc` member.

## Decision

The public no-argument `tiber` command launches the exact reviewed Codex TUI.
Tiber binds a private Unix-socket WebSocket endpoint, starts an isolated Codex
app-server behind a second private socket, and terminates both connections.

The gateway:

- bounds messages to one MiB and nesting to 64 levels before policy handling;
- rewrites `thread/start` to Tiber-owned read-only, offline authority;
- forwards presentation notifications and responses byte-for-byte;
- intercepts approvals and dynamic-tool calls for application policy, while
  explicitly brokering the reviewed Codex authentication-refresh exchange only
  between the bounded TUI and app-server transports;
- holds each text-only `turn/start` until its exact prompt and workflow request
  are durably published, then binds the response's thread and turn identities;
- holds the matching terminal notification until a successful observation or
  sanitized failed/interrupted outcome and workflow advance are durable;
- requires one application-owned, kind-matched completion for each intercepted
  request; and
- fails closed on unknown effect-bearing server requests or effective-policy
drift.

If Tiber stops after durable turn admission but before backend dispatch or
terminal observation, the next native turn first records a content-free
interruption and advances the stopped workflow. Recovery is idempotent across
the atomic observation/advance boundary and never replays the abandoned model
request or fabricates assistant output.

The reviewed Codex executable is a fixed-output Nix input used by the package
and development shell. Startup checks its exact version before terminal
takeover. The first native dynamic tool is `tiber_tasks`, which accepts only the
existing bounded task CLI grammar and executes through signed task authority.
The former projection TUI remains only as a debug-only integration-test driver.

## Consequences

Users receive the actual Codex interface while Tiber retains deterministic
workflow and effect authority. Upgrading Codex requires a new reviewed binary,
schema/effective-authority evidence, and native PTY compatibility test.

Tiber must continue to interpret protocol evolution at the gateway; a
transparent byte proxy is insufficient. Repository, process, delivery, and
other mutation tools remain unavailable in the native interface until their
existing typed durable boundaries are explicitly connected. No model-supplied
shell, executable, environment, working directory, network policy, or generic
EventCore append is introduced by this decision.

Subscription refresh frames are transient transport data: the gateway bounds
and parses their JSON-RPC envelope for correlation, but never sends token fields
to application policy, logs or persists them, or treats them as domain
authority. This is distinct from API-key login, whose inherited stdin never
enters the Tiber process at all.
