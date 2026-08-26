import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCiAuthorityStore } from "../../src/adapters/ci/file-ci-authority-store.js";
import { discoverGithubActionsCatalog } from "../../src/adapters/ci/github-actions-setup.js";
import type { GitHubHttpClient } from "../../src/adapters/github/github-review-service.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

const client: GitHubHttpClient = {
  request: (request) =>
    Promise.resolve(
      request.path.endsWith("/check-runs?per_page=100")
        ? {
            ok: true,
            value: {
              total_count: 2,
              check_runs: [{ name: "CI" }, { name: "Mutation" }],
            },
          }
        : { ok: true, value: { default_branch: "main" } },
    ),
};

describe("GitHub Actions setup discovery", () => {
  it.each([
    "https://github.com/jwilger/tiber.git",
    "git@github.com:jwilger/tiber.git",
    "ssh://git@github.com/jwilger/tiber.git",
  ])(
    "creates a CI authority catalog from active workflows at %s",
    async (origin) => {
      expect(await discoverGithubActionsCatalog(origin, client)).toMatchObject({
        ok: true,
        value: {
          kind: "some",
          value: {
            schemaVersion: 1,
            authorities: [
              {
                kind: "github-actions",
                name: "github-actions",
                repository: "jwilger/tiber",
                requiredChecks: ["CI", "Mutation"],
              },
            ],
          },
        },
      });
    },
  );

  it("prefers branch-protection required checks", async () => {
    const protectedClient: GitHubHttpClient = {
      request: (request) =>
        Promise.resolve(
          request.path.endsWith("required_status_checks")
            ? { ok: true, value: { contexts: ["Required CI"] } }
            : { ok: true, value: { default_branch: "main" } },
        ),
    };

    expect(
      await discoverGithubActionsCatalog(
        "https://github.com/jwilger/tiber.git",
        protectedClient,
      ),
    ).toMatchObject({
      ok: true,
      value: {
        kind: "some",
        value: {
          authorities: [{ requiredChecks: ["Required CI"] }],
        },
      },
    });
  });

  it("persists the setup-generated catalog without manual file editing", async () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-github-ci-"));
    temporaryDirectories.push(root);
    const repository = join(root, "repository");
    const agentDirectory = join(root, "agent");
    mkdirSync(repository);
    mkdirSync(agentDirectory);
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    const discovered = await discoverGithubActionsCatalog(
      "https://github.com/jwilger/tiber.git",
      client,
    );
    if (!discovered.ok || discovered.value.kind === "none")
      throw new Error("expected CI catalog fixture");
    const store = new FileCiAuthorityStore(repository, agentDirectory);

    expect(store.saveCatalog(discovered.value.value)).toMatchObject({
      ok: true,
    });
    expect(store.loadCatalog()).toMatchObject({
      ok: true,
      value: {
        authorities: [
          {
            kind: "github-actions",
            requiredChecks: ["CI", "Mutation"],
          },
        ],
      },
    });

    const otherRepository = join(root, "other-repository");
    mkdirSync(otherRepository);
    execFileSync("git", ["init", "--quiet"], { cwd: otherRepository });
    const otherStore = new FileCiAuthorityStore(
      otherRepository,
      agentDirectory,
    );
    expect(otherStore.catalogExists()).toBe(false);
  });

  it.each([
    "https://gitlab.example.test/team/project.git",
    "https://evilgithub.com/team/project.git",
    "https://example.test/github.com/team/project.git",
  ])(
    "does not invent CI authority for non-GitHub remote %s",
    async (origin) => {
      expect(await discoverGithubActionsCatalog(origin, client)).toEqual({
        ok: true,
        value: { kind: "none" },
      });
    },
  );
});
