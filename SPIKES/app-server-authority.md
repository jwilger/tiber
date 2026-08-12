# App-server authority compatibility spike

## Question

Can Tiber use `codex app-server` for inference while ensuring that no operation
outside Tiber policy produces an effect and every Tiber-declared tool remains
inert until the harness authorizes it?

## Environment

- Platform: x86_64 Linux
- Codex CLI: 0.147.0
- Protocol source: `codex app-server generate-json-schema --experimental`
- Official reference: <https://learn.chatgpt.com/docs/app-server>

## Reproduction

```shell
schema_dir="$(mktemp -d)"
codex app-server generate-json-schema --experimental --out "$schema_dir"
bash scripts/check-app-server-authority-fixture.sh \
  "$schema_dir/codex_app_server_protocol.v2.schemas.json"
cargo run -p tiber -- \
  app-server-probe \
  crates/tiber-app-server/tests/fixtures/codex-0.147.0-authority-surface.json
```

Expected result for Codex 0.147.0:

```text
app-server protocol exposes the reviewed Tiber control surface; runtime policy must cover: thread-item:commandExecution:runtime-policy-controlled, thread-item:fileChange:runtime-policy-controlled
```

The command exits successfully. The pinned-schema verifier must pass before the
deterministic projection is used as decision evidence; ordinary CI checks the
projection behavior but intentionally cannot regenerate a Codex-owned schema.

## Evidence

The generated protocol contains `commandExecution` and `fileChange` thread
items, named `permissions`, approval settings, and client-executed
`dynamicTools`. Operation-item availability is not authority: the effective
permission profile and client response path determine whether an effect can
occur. The schema probe accepts only the provenance-bound projection of the
exact reviewed V2 `ThreadStartParams` fields and `ThreadItem` discriminators,
then fails closed on every structural or version change.

Official OpenAI documentation states that permission profiles constrain local
sandboxed command filesystem and network effects, unsupported Linux policies
are refused instead of silently running unsandboxed, app-server sends command
and file approvals to the client, and dynamic tools execute through an
`item/tool/call` client request. Connectors, MCP, browser, and Computer Use have
separate controls, so Tiber disables those surfaces in its isolated-home
configuration rather than assuming the permission profile covers them.

The deterministic fixture is a projection of the generated 0.147.0 schema,
records the SHA-256 of its 609,050-byte source, and can be regenerated with the
checked-in jq program:

```shell
schema="$schema_dir/codex_app_server_protocol.v2.schemas.json"
schema_sha256="$(sha256sum "$schema" | cut -d' ' -f1)"
jq --arg codex_version 0.147.0 --arg schema_sha256 "$schema_sha256" \
  -f scripts/extract-app-server-authority-surface.jq "$schema" \
  | prettier --parser json

bash scripts/check-app-server-authority-fixture.sh "$schema"
```

The live probe uses `config/app-server.toml` in a disposable Tiber-owned Codex
home. It is an opt-in compatibility check, not an evaluation and not a CI gate.
Run it only in a trusted environment. Authenticate that isolated home through
app-server itself; do not copy credentials or configuration from a user Codex
home:

```shell
probe_root="$(mktemp -d)"
mkdir -p "$probe_root/workspace"
env -u OPENAI_API_KEY XDG_STATE_HOME="$probe_root/state" \
  cargo run -p tiber -- auth login
env -u OPENAI_API_KEY node \
  scripts/probe-app-server-effective-authority.mjs \
  "$probe_root/state/tiber/codex" "$probe_root/workspace"
```

The probe resolves the exact Codex executable and replaces the template's
`TIBER_CODEX_RUNTIME_READ_GRANT` marker with a read-only grant for that file.
Codex uses its own executable as the Linux sandbox helper; granting only that
resolved file avoids broad access to the source user home.

Observed on Codex 0.147.0/x86_64 Linux: the active profile was
`tiber-inference`; the effective sandbox was read-only with command network
disabled; hosted web search was disabled independently; a positive-control
command using the probe's known Node executable exited zero; the same executable's
`command/exec` write attempt failed and created no file; and the sentinel
dynamic tool arrived exactly once through `item/tool/call` while the probe
executed no dynamic-tool effect. Read-only, non-shell repository observation is
an intentional policy capability whose output remains untrusted context. The
probe declines any approval or permission request if one is emitted.

## Decision

The spike passes the corrected effective-authority contract. Tiber retains
app-server as its sole inference transport and may proceed to conversation,
authentication delegation, and TUI work. Every supported Codex upgrade must
pass the pinned schema check and live effective-authority probe before Tiber
widens the compatible protocol range.

The next checked increment added that transport boundary in Rust. The adapter
starts an isolated child, completes browser-login handoff and account
operations through app-server, streams assistant deltas, and returns each
dynamic-tool call as inert typed data after sending an unsuccessful/no-effect
response. API-key-mode setup invokes the isolated `codex login --with-api-key`
child with inherited owner stdin, stripped ambient API-key environment
variables, and suppressed child output, then requires app-server to report the
resulting API-key account state. Tiber never reads, copies, serializes, or
forwards the key. The child has the initial CLI's configured ten-minute
operation deadline and is killed and reaped after expiry; Codex's required
non-terminal stdin keeps the owner-supplied key out of Tiber's memory and
domain handling. Its fake-server tests also prove typed timeout and child
cleanup behavior. This is a source-level executable spike in the standalone
workspace; it is not yet the complete Codex-compatible TUI or durable harness
conversation lifecycle.

## Review-orchestration invariant

This decision does not remove or narrow review orchestration. The native
contract remains recorded in `ARCHITECTURE.md`. The former marketplace
implementation is retained only as porting reference while Tiber builds the
same multi-agent final-review behavior into the harness.
