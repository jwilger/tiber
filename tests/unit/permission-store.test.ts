import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FilePermissionStore } from "../../src/adapters/permissions/file-permission-store.js";
import { parseProjectId } from "../../src/core/configuration/configuration-values.js";
import {
  parsePermissionDecisionAt,
  permissionScope,
  type PermissionDecisionAt,
} from "../../src/core/permissions/permission-values.js";

function projectId(value: string) {
  const parsed = parseProjectId(value);
  if (!parsed.ok) throw new Error("invalid project id fixture");
  return parsed.value;
}

function decisionAt(value: string): PermissionDecisionAt {
  const parsed = parsePermissionDecisionAt(value);
  if (!parsed.ok) throw new Error("invalid permission timestamp fixture");
  return parsed.value;
}

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

describe("repository-local permission store", () => {
  it("persists a host-derived repository-bound permission decision", () => {
    const agentDirectory = mkdtempSync(join(tmpdir(), "tiber-permissions-"));
    temporaryDirectories.push(agentDirectory);
    const store = new FilePermissionStore(
      agentDirectory,
      projectId("00000000-0000-4000-8000-000000000123"),
    );
    const scope = permissionScope({
      role: "implementation",
      effect: "process",
      executable: "npm",
      purpose: "test",
    });

    expect(
      store.remember(scope, "allow", decisionAt("2026-08-26T00:00:00.000Z")),
    ).toEqual({ ok: true, value: undefined });
    expect(store.lookup(scope)).toEqual({
      ok: true,
      value: { kind: "some", value: "allow" },
    });
  });

  it("keeps permissions separate by repository identity", () => {
    const agentDirectory = mkdtempSync(join(tmpdir(), "tiber-permissions-"));
    temporaryDirectories.push(agentDirectory);
    const scope = permissionScope({
      role: "implementation",
      effect: "process",
      executable: "npm",
      purpose: "test",
    });
    const first = new FilePermissionStore(
      agentDirectory,
      projectId("00000000-0000-4000-8000-000000000001"),
    );
    const second = new FilePermissionStore(
      agentDirectory,
      projectId("00000000-0000-4000-8000-000000000002"),
    );

    expect(
      first.remember(scope, "deny", decisionAt("2026-08-26T00:00:00.000Z")),
    ).toMatchObject({ ok: true });
    expect(second.lookup(scope)).toEqual({
      ok: true,
      value: { kind: "none" },
    });
  });

  it("rejects malformed persisted authority instead of ignoring it", () => {
    const agentDirectory = mkdtempSync(join(tmpdir(), "tiber-permissions-"));
    temporaryDirectories.push(agentDirectory);
    const repositoryId = "00000000-0000-4000-8000-000000000123";
    const store = new FilePermissionStore(
      agentDirectory,
      projectId(repositoryId),
    );
    const stateDirectory = join(
      agentDirectory,
      "tiber",
      "projects",
      repositoryId,
    );
    mkdirSync(stateDirectory, { recursive: true });
    writeFileSync(join(stateDirectory, "permissions.v1.json"), "{}\n");

    expect(
      store.lookup(
        permissionScope({
          role: "implementation",
          effect: "process",
          executable: "npm",
          purpose: "test",
        }),
      ),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_PERMISSION_STATE_INVALID" },
    });
  });
});
