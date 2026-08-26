import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import {
  parseCiExecutableDigest,
  parseCiGithubRepository,
  type CiExecutableDigest,
  type CiGithubRepository,
} from "../../core/ci/ci-values.js";
import { none, some, type Option } from "../../core/types/option.js";
import type { GitHubHttpClient } from "../github/github-review-service.js";
import { parseGitHubCiCredential } from "../github/github-review-service.js";
import {
  invalidCiAdapter,
  parseCiAuthorityCatalog,
  type CiAdapterResult,
  type CiAuthorityCatalog,
} from "./user-local-ci-authority.js";

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function githubRepositoryFromOrigin(
  origin: string,
): CiGithubRepository | undefined {
  const match =
    /^(?:https?:\/\/github\.com\/|ssh:\/\/(?:git@)?github\.com\/|git@github\.com:)([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+?)(?:\.git)?$/iu.exec(
      origin,
    );
  if (match?.[1] === undefined || match[2] === undefined) return undefined;
  const parsed = parseCiGithubRepository(`${match[1]}/${match[2]}`);
  return parsed.ok ? parsed.value : undefined;
}

function shippingPackageVersion(): string {
  try {
    const input: unknown = JSON.parse(
      readFileSync(new URL("../../../package.json", import.meta.url), "utf8"),
    );
    return record(input) &&
      typeof input.version === "string" &&
      /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u.test(input.version)
      ? input.version
      : "invalid";
  } catch {
    return "invalid";
  }
}

export function githubActionsAdapterDigest(): CiExecutableDigest {
  const parsed = parseCiExecutableDigest(
    createHash("sha256")
      .update(
        `@jwilger/tiber:${shippingPackageVersion()}:github-actions-ci-authority:v1`,
      )
      .digest("hex"),
  );
  if (!parsed.ok) throw new Error("GitHub Actions adapter digest is invalid");
  return parsed.value;
}

export async function discoverGithubActionsCatalog(
  origin: string,
  client: GitHubHttpClient,
): Promise<CiAdapterResult<Option<CiAuthorityCatalog>>> {
  const repository = githubRepositoryFromOrigin(origin);
  if (repository === undefined) return { ok: true, value: none };
  const credential = parseGitHubCiCredential("host-gh");
  if (!credential.ok)
    return invalidCiAdapter("GitHub host credential selection failed");
  const repositoryResponse = await client.request({
    method: "GET",
    path: `/repos/${repository}`,
    credential: credential.value,
  });
  if (
    !repositoryResponse.ok ||
    !record(repositoryResponse.value) ||
    typeof repositoryResponse.value.default_branch !== "string" ||
    !/^[A-Za-z0-9._/-]{1,255}$/u.test(repositoryResponse.value.default_branch)
  )
    return invalidCiAdapter("GitHub default branch could not be discovered");
  const encodedBranch = encodeURIComponent(
    repositoryResponse.value.default_branch,
  );
  const protectedChecks = await client.request({
    method: "GET",
    path: `/repos/${repository}/branches/${encodedBranch}/protection/required_status_checks`,
    credential: credential.value,
  });
  if (
    protectedChecks.ok &&
    record(protectedChecks.value) &&
    Array.isArray(protectedChecks.value.contexts) &&
    protectedChecks.value.contexts.length > 0 &&
    protectedChecks.value.contexts.every(
      (context) => typeof context === "string",
    )
  ) {
    const parsed = parseCiAuthorityCatalog({
      schemaVersion: 1,
      authorities: [
        {
          kind: "github-actions",
          name: "github-actions",
          repository,
          requiredChecks: [...new Set(protectedChecks.value.contexts)].sort(),
          adapterSha256: githubActionsAdapterDigest(),
        },
      ],
    });
    return parsed.ok ? { ok: true, value: some(parsed.value) } : parsed;
  }
  const response = await client.request({
    method: "GET",
    path: `/repos/${repository}/commits/${encodedBranch}/check-runs?per_page=100`,
    credential: credential.value,
  });
  if (!response.ok)
    return invalidCiAdapter("GitHub Actions checks could not be discovered");
  const value = response.value;
  if (
    !record(value) ||
    !Number.isSafeInteger(value.total_count) ||
    !Array.isArray(value.check_runs) ||
    value.total_count !== value.check_runs.length ||
    value.check_runs.length > 100
  )
    return invalidCiAdapter("GitHub Actions check response is invalid");
  const checkNames: string[] = [];
  for (const check of value.check_runs) {
    if (!record(check) || typeof check.name !== "string")
      return invalidCiAdapter("GitHub Actions check entry is invalid");
    checkNames.push(check.name);
  }
  const names = [...new Set(checkNames)].sort();
  if (names.length === 0) return { ok: true, value: none };
  const parsed = parseCiAuthorityCatalog({
    schemaVersion: 1,
    authorities: [
      {
        kind: "github-actions",
        name: "github-actions",
        repository,
        requiredChecks: names,
        adapterSha256: githubActionsAdapterDigest(),
      },
    ],
  });
  return parsed.ok ? { ok: true, value: some(parsed.value) } : parsed;
}
