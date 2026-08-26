import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileSettingsStore } from "../../src/adapters/settings/file-settings-store.js";
import { some } from "../../src/core/types/option.js";
import {
  inspectActiveSetup,
  inspectSetup,
} from "../../src/extension/setup-tool.js";
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

describe("guided setup inspection", () => {
  it("preserves actionable Git-repository recovery evidence when inspection cannot start", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-no-git-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);

    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_SETUP_INSPECTION_FAILED",
        causes: [
          {
            code: "TIBER_SETTINGS_REPOSITORY_REQUIRED",
            safeSummary: "project settings require a Git repository",
          },
        ],
      },
    });
  });

  it("ends setup mode when active inspection fails", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-failed-active-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(join(agentDirectory, "tiber"), { recursive: true });
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    writeFileSync(join(agentDirectory, "tiber", "settings.json"), "{}\n");
    const parsedAgentDirectory = parseSetupAgentDirectoryPath(agentDirectory);
    const parsedRepository = parseSetupRepositoryPath(repository);
    expect(parsedAgentDirectory.ok).toBe(true);
    expect(parsedRepository.ok).toBe(true);
    if (!parsedAgentDirectory.ok || !parsedRepository.ok) return;
    const ended: string[] = [];

    const response = inspectActiveSetup(
      parsedAgentDirectory.value,
      parsedRepository.value,
      {
        beginConversation: () => undefined,
        endConversation: (endedRepository) => {
          ended.push(endedRepository);
          return {
            state: "verified",
            level: "host-trusted",
            code: "TIBER_CONTAINMENT_HOST_TRUSTED",
            detail: "No strong containment requested",
          };
        },
        isConversationActive: () => true,
      },
    );

    expect(response.details.disposition).toBe("denied");
    expect(response.content[0]?.text).toContain(
      "TIBER_SETUP_INSPECTION_FAILED",
    );
    expect(ended).toEqual([repository]);
  });

  it("reports safe defaults and actionable missing prerequisites in a fresh repository", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-inspection-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    for (const key of [
      "user.name",
      "user.email",
      "user.signingkey",
      "gpg.format",
      "gpg.ssh.allowedSignersFile",
    ]) {
      execFileSync("git", ["config", "--local", key, ""], {
        cwd: repository,
      });
    }

    const inspected = inspectSetup(agentDirectory, repository, {});

    expect(inspected).toMatchObject({
      ok: true,
      value: {
        settings: {
          effective: {
            assuranceLevel: {
              value: "host-trusted",
              source: "built-in",
            },
            outputPreviewBytes: { value: 16_384, source: "built-in" },
            worktreeMode: { value: "isolated", source: "built-in" },
          },
        },
        commandCatalog: { status: "missing" },
        projectWorkflow: { status: "built-in" },
        prerequisites: {
          executables: {
            git: { status: "missing" },
            npm: { status: "missing" },
            npx: { status: "missing" },
          },
          origin: { status: "missing" },
          signing: { status: "missing" },
          containment: { status: "verified", level: "host-trusted" },
        },
        integrations: {
          context7: { network: "disabled", endpoint: "default" },
          hindsight: { endpoint: "disabled", sharedBank: "missing" },
          githubReview: { status: "not-github", client: "gh" },
          ci: { status: "missing" },
        },
      },
    });
  });

  it("reports a project strong-containment override without attestation as a blocker", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-containment-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });

    const store = new FileSettingsStore(agentDirectory, repository);
    const initial = store.load();
    expect(initial.ok).toBe(true);
    if (!initial.ok) return;
    expect(
      store.saveProject(initial.value.projectId, {
        assuranceLevel: some("workspace-isolated"),
        outputPreviewBytes: { kind: "none" },
        worktreeMode: { kind: "none" },
      }).ok,
    ).toBe(true);

    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        settings: {
          effective: {
            assuranceLevel: {
              value: "workspace-isolated",
              source: "project",
            },
          },
        },
        prerequisites: {
          containment: {
            status: "lockdown",
            level: "workspace-isolated",
            code: "TIBER_CONTAINMENT_ATTESTATION_MISSING",
            detail: "External containment attestation is required",
          },
        },
      },
    });
  });

  it("does not follow a project declaration file symlink outside the repository", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-file-symlink-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    const externalCatalog = join(root, "external-commands.json");
    mkdirSync(join(repository, ".tiber"), { recursive: true });
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    writeFileSync(
      externalCatalog,
      `${JSON.stringify({
        schemaVersion: 1,
        commands: [
          {
            name: "external-leak",
            executable: "/usr/bin/node",
            purpose: "test",
            argv: ["--test"],
            cwd: "worktree",
            environment: {},
            timeoutMs: 60_000,
            maxOutputBytes: 1_048_576,
          },
        ],
      })}\n`,
    );
    symlinkSync(externalCatalog, join(repository, ".tiber", "commands.json"));

    const inspected = inspectSetup(agentDirectory, repository, {});
    expect(inspected).toMatchObject({
      ok: true,
      value: { commandCatalog: { status: "invalid" } },
    });
    expect(JSON.stringify(inspected)).not.toContain("external-leak");
  });

  it("reports a malformed local command grant instead of treating it as merely ungranted", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-grant-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(join(repository, ".tiber"), { recursive: true });
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    writeFileSync(
      join(repository, ".tiber", "commands.json"),
      `${JSON.stringify({
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
      })}\n`,
    );
    mkdirSync(join(repository, ".git", "tiber"), { recursive: true });
    writeFileSync(
      join(repository, ".git", "tiber", "command-grant.v1.json"),
      "{}\n",
    );

    expect(inspectSetup(agentDirectory, repository, {})).toMatchObject({
      ok: true,
      value: {
        commandCatalog: {
          status: "invalid",
          failure: "command grant document is invalid",
        },
      },
    });
  });

  it("does not expose credentials embedded in a configured origin", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-setup-origin-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    execFileSync(
      "git",
      [
        "remote",
        "add",
        "origin",
        "https://private-user:private-token@example.test/project.git",
      ],
      {
        cwd: repository,
      },
    );

    const inspected = inspectSetup(agentDirectory, repository, {});
    const rendered = JSON.stringify(inspected);

    expect(inspected).toMatchObject({
      ok: true,
      value: { prerequisites: { origin: { status: "configured" } } },
    });
    expect(rendered).not.toContain("private-user");
    expect(rendered).not.toContain("private-token");
  });
});
