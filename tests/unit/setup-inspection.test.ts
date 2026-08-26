import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { inspectSetup } from "../../src/extension/setup-tool.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("guided setup inspection", () => {
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
        prerequisites: {
          origin: { status: "missing" },
          signing: { status: "missing" },
          containment: { status: "verified", level: "host-trusted" },
        },
        integrations: {
          context7: { status: "disabled" },
          hindsight: { status: "disabled" },
          githubReview: { status: "disabled" },
          ci: { status: "missing" },
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
