import { describe, expect, it } from "vitest";

import { observeGithubActionsAuthority } from "../../src/adapters/ci/github-actions-ci-authority.js";
import { githubActionsAdapterDigest } from "../../src/adapters/ci/github-actions-setup.js";
import type { GithubActionsAuthorityDefinition } from "../../src/adapters/ci/user-local-ci-authority.js";
import type { GitHubHttpClient } from "../../src/adapters/github/github-review-service.js";
import {
  parseCiExecutableDigest,
  parseCiGithubCheckName,
  parseCiGithubRepository,
  parseCiAuthorityName,
  parseCiRevision,
} from "../../src/core/ci/ci-values.js";

function definition(): GithubActionsAuthorityDefinition {
  const name = parseCiAuthorityName("github-actions");
  const repository = parseCiGithubRepository("jwilger/tiber");
  const check = parseCiGithubCheckName("CI");
  const digest = parseCiExecutableDigest(githubActionsAdapterDigest());
  if (!name.ok || !repository.ok || !check.ok || !digest.ok)
    throw new Error("invalid GitHub Actions fixture");
  return {
    kind: "github-actions",
    name: name.value,
    repository: repository.value,
    requiredChecks: [check.value],
    adapterSha256: digest.value,
  };
}

function differentAdapterDigest() {
  const parsed = parseCiExecutableDigest("a".repeat(64));
  if (!parsed.ok) throw new Error("invalid adapter digest fixture");
  return parsed.value;
}

function revision() {
  const parsed = parseCiRevision("b".repeat(40));
  if (!parsed.ok) throw new Error("invalid revision fixture");
  return parsed.value;
}

describe("first-party GitHub Actions CI authority", () => {
  it("accepts success only when every configured check succeeds at the exact revision", async () => {
    const client: GitHubHttpClient = {
      request: () =>
        Promise.resolve({
          ok: true,
          value: {
            total_count: 1,
            check_runs: [
              {
                name: "CI",
                head_sha: "b".repeat(40),
                status: "completed",
                conclusion: "success",
              },
            ],
          },
        }),
    };

    expect(
      await observeGithubActionsAuthority(definition(), revision(), client),
    ).toMatchObject({
      ok: true,
      value: {
        authority: "github-actions",
        revision: "b".repeat(40),
        status: "success",
        adapterDigest: githubActionsAdapterDigest(),
      },
    });
  });

  it("rejects a catalog pinned to a different Tiber adapter", async () => {
    const current = definition();
    const client: GitHubHttpClient = {
      request: () =>
        Promise.resolve({
          ok: true,
          value: { total_count: 0, check_runs: [] },
        }),
    };

    expect(
      await observeGithubActionsAuthority(
        { ...current, adapterSha256: differentAdapterDigest() },
        revision(),
        client,
      ),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_CI_ADAPTER_INVALID" },
    });
  });

  it("reports a configured pending check as pending", async () => {
    const client: GitHubHttpClient = {
      request: () =>
        Promise.resolve({
          ok: true,
          value: {
            total_count: 1,
            check_runs: [
              {
                name: "CI",
                head_sha: "b".repeat(40),
                status: "in_progress",
                conclusion: null,
              },
            ],
          },
        }),
    };

    expect(
      await observeGithubActionsAuthority(definition(), revision(), client),
    ).toMatchObject({ ok: true, value: { status: "pending" } });
  });
});
