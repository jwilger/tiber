import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCommandAuthority } from "../../src/adapters/commands/file-command-authority.js";
import { FileAuthorityStore } from "../../src/adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../../src/adapters/settings/file-settings-store.js";
import {
  applySetupPlan,
  inspectSetup,
} from "../../src/extension/setup-tool.js";
import { parseSetupPlan } from "../../src/core/configuration/setup.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("guided setup application", () => {
  it("persists a validated complete setup and grants its exact command catalog", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });

    const parsed = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: 32_768,
        worktreeMode: "isolated",
      },
      minimumAssuranceLevel: "host-trusted",
      secretReferences: {
        context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
      },
      commandCatalog: {
        action: "replace",
        definition: {
          schemaVersion: 1,
          commands: [
            {
              name: "unit",
              executable: "/usr/bin/node",
              purpose: "test",
              argv: ["--test"],
              cwd: "worktree",
              environment: {},
              timeoutMs: 60_000,
              maxOutputBytes: 1_048_576,
            },
          ],
        },
      },
    });
    if (!parsed.ok) throw new Error(parsed.failure.message);

    const applied = applySetupPlan(agentDirectory, repository, parsed.value);

    expect(applied).toMatchObject({ ok: true, commandCatalog: "granted" });
    const settings = new FileSettingsStore(agentDirectory, repository).load();
    expect(settings).toMatchObject({
      ok: true,
      value: {
        projectValues: {
          assuranceLevel: { kind: "some", value: "host-trusted" },
          outputPreviewBytes: { kind: "some", value: 32_768 },
          worktreeMode: { kind: "some", value: "isolated" },
        },
      },
    });
    expect(new FileAuthorityStore(agentDirectory).load()).toMatchObject({
      ok: true,
      value: {
        ceilings: {
          minimumAssuranceLevel: {
            kind: "some",
            value: "host-trusted",
          },
        },
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      },
    });
    const commands = new FileCommandAuthority(repository);
    const catalog = commands.loadCatalog();
    if (!catalog.ok) throw new Error(catalog.failure.message);
    expect(commands.readGrant()).toEqual({
      ok: true,
      value: { kind: "some", value: catalog.value.digest },
    });
    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        commandCatalog: {
          status: "granted",
          digest: catalog.value.digest,
        },
      },
    });
  });
});
