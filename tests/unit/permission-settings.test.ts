import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FilePermissionSettingsStore } from "../../src/adapters/permissions/file-permission-settings-store.js";
import { parseProjectId } from "../../src/core/configuration/configuration-values.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

function projectId() {
  const parsed = parseProjectId("00000000-0000-4000-8000-000000000123");
  if (!parsed.ok) throw new Error("invalid project id fixture");
  return parsed.value;
}

describe("permission experience settings", () => {
  it("starts with routine autonomy", () => {
    const agentDirectory = mkdtempSync(join(tmpdir(), "tiber-autonomy-"));
    temporaryDirectories.push(agentDirectory);
    const store = new FilePermissionSettingsStore(agentDirectory, projectId());

    expect(store.load()).toEqual({
      ok: true,
      value: { schemaVersion: 1, autonomy: "routine" },
    });
  });

  it.each(["ask-first", "routine", "repository"] as const)(
    "persists %s autonomy",
    (autonomy) => {
      const agentDirectory = mkdtempSync(join(tmpdir(), "tiber-autonomy-"));
      temporaryDirectories.push(agentDirectory);
      const store = new FilePermissionSettingsStore(
        agentDirectory,
        projectId(),
      );

      expect(store.save({ schemaVersion: 1, autonomy })).toMatchObject({
        ok: true,
      });
      expect(store.load()).toEqual({
        ok: true,
        value: { schemaVersion: 1, autonomy },
      });
    },
  );
});
