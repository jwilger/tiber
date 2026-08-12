#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "../..");
const fixture = path.join(root, "scripts/tests/fake-app-server.mjs");
const probe = path.join(
  root,
  "scripts/probe-app-server-effective-authority.mjs",
);
const config = fs.readFileSync(
  path.join(root, "config/app-server.toml"),
  "utf8",
);
for (const requiredSetting of [
  'approval_policy = "never"',
  'approvals_reviewer = "user"',
  'default_permissions = "tiber-inference"',
  '":minimal" = "read"',
  '"." = "read"',
  "apps = false",
  "browser_use = false",
  "computer_use = false",
  "enabled = false",
  "image_generation = false",
  "request_permissions_tool = false",
  "shell_tool = false",
  'web_search = "disabled"',
]) {
  assert.equal(config.includes(requiredSetting), true, requiredSetting);
}
assert.equal(config.includes("sandbox_mode"), false);
const temporaryRoot = fs.mkdtempSync(
  path.join(os.tmpdir(), "tiber-authority-probe-test-"),
);
const codexHome = path.join(temporaryRoot, "codex-home");
const workspace = path.join(temporaryRoot, "workspace");
fs.mkdirSync(codexHome);
fs.mkdirSync(workspace);
fs.copyFileSync(
  path.join(root, "config/app-server.toml"),
  path.join(codexHome, "config.toml"),
);

const result = spawnSync(process.execPath, [probe, codexHome, workspace], {
  encoding: "utf8",
  env: { ...process.env, TIBER_APP_SERVER_FIXTURE: fixture },
  timeout: 10_000,
});

assert.equal(result.status, 0, result.stderr);
const evidence = JSON.parse(result.stdout);
assert.equal(evidence.activePermissionProfile.id, "tiber-inference");
assert.equal(evidence.approvalPolicy, "never");
assert.equal(
  evidence.codexRuntimeExecutable,
  fs.realpathSync(process.execPath),
);
assert.deepEqual(evidence.sandbox, {
  networkAccess: false,
  type: "readOnly",
});
assert.equal(evidence.dynamicTool.calls.length, 1);
assert.equal(evidence.dynamicTool.calls[0].tool, "tiber_authority_probe");
assert.equal(evidence.dynamicTool.effectExecutedByProbe, false);
assert.equal(evidence.commandSandbox.attempted, true);
assert.equal(evidence.commandSandbox.control.exitCode, 0);
assert.equal(evidence.commandSandbox.result.rejected, true);
assert.equal(evidence.commandSandbox.result.transportError, false);
assert.equal(evidence.unauthorizedMutation.artifactExists, false);
assert.equal(
  fs.existsSync(path.join(workspace, "tiber-unauthorized-effect")),
  false,
);

for (const [mode, expectedDiagnostic] of [
  ["wrong-home", "unexpected Codex home"],
  ["early-close", "closed while work was pending"],
  ["malformed", "invalid app-server JSON"],
  ["silent", "request timed out"],
  ["ignored-term", "request timed out"],
  ["command-error", "command/exec unavailable"],
  ["command-malformed", "no recognized terminal result"],
  ["command-timeout", "command/exec"],
  ["control-failure", "positive control"],
  ["close-after-request", "closed while work was pending"],
]) {
  const failed = spawnSync(process.execPath, [probe, codexHome, workspace], {
    encoding: "utf8",
    env: {
      ...process.env,
      TIBER_APP_SERVER_FIXTURE: fixture,
      TIBER_AUTHORITY_PROBE_TIMEOUT_MS: "500",
      TIBER_FIXTURE_MODE: mode,
    },
    timeout: 5_000,
  });
  assert.notEqual(failed.status, 0, mode);
  assert.equal(
    failed.stderr.includes("app_server_authority_probe_failed"),
    true,
    `${mode}: ${failed.stderr}`,
  );
  assert.equal(
    failed.stderr.includes(expectedDiagnostic),
    true,
    `${mode}: ${failed.stderr}`,
  );
}

fs.rmSync(temporaryRoot, { force: true, recursive: true });
