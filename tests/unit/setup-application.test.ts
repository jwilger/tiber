import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCommandAuthority } from "../../src/adapters/commands/file-command-authority.js";
import { FilePermissionSettingsStore } from "../../src/adapters/permissions/file-permission-settings-store.js";
import { FileAuthorityStore } from "../../src/adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../../src/adapters/settings/file-settings-store.js";
import { FileSetupJournal } from "../../src/adapters/settings/file-setup-journal.js";
import { FileWorkflowConfiguration } from "../../src/adapters/workflows/file-workflow-configuration.js";
import {
  applySetupPlan,
  inspectSetup,
  reconcilePendingSetup,
} from "../../src/extension/setup-tool.js";
import { parseSetupPlan } from "../../src/core/configuration/setup.js";
import { none, some } from "../../src/core/types/option.js";
import {
  parseSetupAgentDirectoryPath,
  parseSetupRepositoryPath,
} from "../../src/core/configuration/setup-values.js";

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
      projectWorkflow: {
        action: "replace",
        definition: {
          schemaVersion: 1,
          id: "project.workflow",
          stages: [
            "intake",
            "specification-readiness",
            "remote-claim",
            "baseline-revalidation",
            "red",
            "green",
            "lightweight-review",
            "full-verification",
            "final-review-1",
            "final-review-2",
            "final-review-3",
            "delivery",
            "exact-revision-ci",
            "claim-release",
            "cleanup",
            "done",
          ],
        },
      },
    });
    const initial = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    if (!parsed.ok || !initial.ok) throw new Error("invalid setup fixture");

    const applied = applySetupPlan(
      agentDirectory,
      repository,
      initial.value,
      parsed.value,
    );

    expect(applied).toMatchObject({
      ok: true,
      value: { commandCatalog: "granted", projectWorkflow: "replaced" },
    });
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
    const repositoryPath = parseSetupRepositoryPath(repository);
    if (!repositoryPath.ok) throw new Error(repositoryPath.failure.message);
    const workflow = new FileWorkflowConfiguration(repositoryPath.value).load();
    expect(workflow).toMatchObject({
      ok: true,
      value: {
        kind: "some",
        value: { definition: { id: "project.workflow" } },
      },
    });
    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        commandCatalog: {
          status: "granted",
          digest: catalog.value.digest,
        },
        projectWorkflow: { status: "configured", id: "project.workflow" },
      },
    });

    const removal = parseSetupPlan({
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
      commandCatalog: { action: "remove" },
      projectWorkflow: { action: "built-in" },
    });
    if (!removal.ok) throw new Error(removal.failure.message);
    expect(
      applySetupPlan(agentDirectory, repository, parsed.value, removal.value),
    ).toMatchObject({
      ok: true,
      value: { commandCatalog: "removed", projectWorkflow: "built-in" },
    });
    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        commandCatalog: { status: "missing" },
        projectWorkflow: { status: "built-in" },
      },
    });
  });

  it("recovers a durable confirmed intent after a partial application", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-recovery-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    const initial = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    const proposed = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: 24_576,
        worktreeMode: "isolated",
      },
      projectSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: 24_576,
        worktreeMode: "isolated",
      },
      minimumAssuranceLevel: "host-trusted",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "built-in" },
    });
    if (!initial.ok || !proposed.ok) throw new Error("invalid setup fixture");
    const settingsStore = new FileSettingsStore(agentDirectory, repository);
    const settings = settingsStore.load();
    if (!settings.ok) throw new Error(settings.failure.message);
    const agentPath = parseSetupAgentDirectoryPath(agentDirectory);
    const repositoryPath = parseSetupRepositoryPath(repository);
    if (!agentPath.ok || !repositoryPath.ok)
      throw new Error("invalid setup paths");
    const journal = new FileSetupJournal(
      agentPath.value,
      repositoryPath.value,
      settings.value.projectId,
    );
    expect(
      journal.begin(initial.value, proposed.value, {
        permissionSettings: some({
          schemaVersion: 1,
          autonomy: "repository",
        }),
        ciCatalog: none,
      }).ok,
    ).toBe(true);
    expect(settingsStore.saveGlobal(proposed.value.globalSettings).ok).toBe(
      true,
    );

    expect(reconcilePendingSetup(agentDirectory, repository)).toEqual({
      ok: true,
      value: "recovered",
    });
    expect(
      new FilePermissionSettingsStore(
        agentDirectory,
        settings.value.projectId,
      ).load(),
    ).toEqual({
      ok: true,
      value: { schemaVersion: 1, autonomy: "repository" },
    });
    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        settings: {
          project: {
            assuranceLevel: { kind: "some", value: "host-trusted" },
          },
        },
        authority: {
          ceilings: {
            minimumAssuranceLevel: {
              kind: "some",
              value: "host-trusted",
            },
          },
        },
        recovery: { status: "clean" },
      },
    });

    const next = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "hermetic",
        outputPreviewBytes: 32_768,
        worktreeMode: "isolated",
      },
      projectSettings: {
        assuranceLevel: "hermetic",
        outputPreviewBytes: 32_768,
        worktreeMode: "isolated",
      },
      minimumAssuranceLevel: "hermetic",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "built-in" },
    });
    const externalDrift = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "workspace-isolated",
        outputPreviewBytes: 65_536,
        worktreeMode: "current",
      },
      projectSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: 24_576,
        worktreeMode: "isolated",
      },
      minimumAssuranceLevel: "host-trusted",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "built-in" },
    });
    if (!next.ok || !externalDrift.ok) throw new Error("invalid setup fixture");
    expect(journal.begin(proposed.value, next.value).ok).toBe(true);
    expect(
      settingsStore.saveGlobal(externalDrift.value.globalSettings).ok,
    ).toBe(true);

    expect(reconcilePendingSetup(agentDirectory, repository)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SETUP_CONFIGURATION_CHANGED" },
    });
    expect(settingsStore.load()).toMatchObject({
      ok: true,
      value: {
        globalValues: {
          assuranceLevel: { kind: "some", value: "workspace-isolated" },
        },
      },
    });
  });

  it("requires invalid declarations to be repaired instead of silently kept", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-invalid-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(join(repository, ".tiber"), { recursive: true });
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    writeFileSync(join(repository, ".tiber", "commands.json"), "{}\n");
    const setup = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    if (!setup.ok) throw new Error(setup.failure.message);

    expect(
      applySetupPlan(agentDirectory, repository, setup.value, setup.value),
    ).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_SETUP_APPLY_FAILED",
        message:
          "invalid project declarations must be replaced or removed during setup",
      },
    });
    expect(
      new FileSettingsStore(agentDirectory, repository).load(),
    ).toMatchObject({
      ok: true,
      value: { globalValues: { assuranceLevel: { kind: "none" } } },
    });

    rmSync(join(repository, ".tiber", "commands.json"));
    writeFileSync(join(repository, ".tiber", "workflow.json"), "{}\n");
    expect(
      applySetupPlan(agentDirectory, repository, setup.value, setup.value),
    ).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_SETUP_APPLY_FAILED",
        message:
          "invalid project declarations must be replaced or removed during setup",
      },
    });
  });

  it("refuses project declaration writes through a repository symlink escape", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-symlink-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    const external = join(root, "external");
    for (const directory of [repository, agentDirectory, external])
      mkdirSync(directory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    symlinkSync(external, join(repository, ".tiber"), "dir");
    const initial = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    const proposed = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: 16_384,
        worktreeMode: "isolated",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
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
      projectWorkflow: { action: "keep" },
    });
    if (!initial.ok || !proposed.ok) throw new Error("invalid setup fixture");

    expect(
      applySetupPlan(agentDirectory, repository, initial.value, proposed.value),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SETUP_APPLY_FAILED" },
    });
    expect(existsSync(join(external, "commands.json"))).toBe(false);
    expect(existsSync(join(external, "workflow.json"))).toBe(false);
  });

  it("serializes global setup authority across repositories", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-global-"));
    temporaryDirectories.push(root);
    const agentDirectory = join(root, "agent");
    const firstRepository = join(root, "first");
    const secondRepository = join(root, "second");
    for (const directory of [agentDirectory, firstRepository, secondRepository])
      mkdirSync(directory);
    for (const repository of [firstRepository, secondRepository])
      execFileSync("git", ["init", "--quiet"], { cwd: repository });
    const setup = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    const agentPath = parseSetupAgentDirectoryPath(agentDirectory);
    const firstPath = parseSetupRepositoryPath(firstRepository);
    const secondPath = parseSetupRepositoryPath(secondRepository);
    const firstSettings = new FileSettingsStore(
      agentDirectory,
      firstRepository,
    ).load();
    const secondSettings = new FileSettingsStore(
      agentDirectory,
      secondRepository,
    ).load();
    if (
      !setup.ok ||
      !agentPath.ok ||
      !firstPath.ok ||
      !secondPath.ok ||
      !firstSettings.ok ||
      !secondSettings.ok
    )
      throw new Error("invalid setup fixture");

    expect(
      new FileSetupJournal(
        agentPath.value,
        firstPath.value,
        firstSettings.value.projectId,
      ).begin(setup.value, setup.value).ok,
    ).toBe(true);
    expect(
      new FileSetupJournal(
        agentPath.value,
        secondPath.value,
        secondSettings.value.projectId,
      ).begin(setup.value, setup.value),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SETUP_JOURNAL_CONFLICT" },
    });
  });

  it("refuses to apply a plan after the confirmed authority state changes", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-guided-setup-drift-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });

    const expected = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "host-trusted",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "unlocked",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    const proposed = parseSetupPlan({
      schemaVersion: 1,
      globalSettings: {
        assuranceLevel: "hermetic",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      projectSettings: {
        assuranceLevel: "inherit",
        outputPreviewBytes: "inherit",
        worktreeMode: "inherit",
      },
      minimumAssuranceLevel: "hermetic",
      secretReferences: {},
      commandCatalog: { action: "keep" },
      projectWorkflow: { action: "keep" },
    });
    if (!expected.ok || !proposed.ok) throw new Error("invalid setup fixture");

    expect(
      applySetupPlan(
        agentDirectory,
        repository,
        expected.value,
        proposed.value,
      ),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SETUP_CONFIGURATION_CHANGED" },
    });
    expect(
      new FileSettingsStore(agentDirectory, repository).load(),
    ).toMatchObject({
      ok: true,
      value: { globalValues: { assuranceLevel: { kind: "none" } } },
    });
  });
});
