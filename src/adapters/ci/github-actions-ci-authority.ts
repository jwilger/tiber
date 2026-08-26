import { createHash } from "node:crypto";

import type { CiAuthorityObservation } from "../../core/ci/ci-authority.js";
import {
  parseCiObservationDigest,
  type CiRevision,
} from "../../core/ci/ci-values.js";
import type {
  GitHubCiCredential,
  GitHubHttpClient,
} from "../github/github-review-service.js";
import { parseGitHubCiCredential } from "../github/github-review-service.js";
import { githubActionsAdapterDigest } from "./github-actions-setup.js";
import type {
  CiAdapterResult,
  GithubActionsAuthorityDefinition,
} from "./user-local-ci-authority.js";
import { parseCiAuthorityOutput } from "./user-local-ci-authority.js";

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalid(
  definition: GithubActionsAuthorityDefinition,
  revision: CiRevision,
): CiAdapterResult<CiAuthorityObservation> {
  return parseCiAuthorityOutput(
    definition.name,
    revision,
    definition.adapterSha256,
    "invalid GitHub Actions observation",
  );
}

function hostCredential(): GitHubCiCredential | undefined {
  const parsed = parseGitHubCiCredential("host-gh");
  return parsed.ok ? parsed.value : undefined;
}

export async function observeGithubActionsAuthority(
  definition: GithubActionsAuthorityDefinition,
  revision: CiRevision,
  client: GitHubHttpClient,
): Promise<CiAdapterResult<CiAuthorityObservation>> {
  const credential = hostCredential();
  if (
    credential === undefined ||
    definition.adapterSha256 !== githubActionsAdapterDigest()
  )
    return invalid(definition, revision);
  const response = await client.request({
    method: "GET",
    path: `/repos/${definition.repository}/commits/${revision}/check-runs?per_page=100`,
    credential,
  });
  if (!response.ok) return invalid(definition, revision);
  const value = response.value;
  if (
    !record(value) ||
    !Number.isSafeInteger(value.total_count) ||
    !Array.isArray(value.check_runs) ||
    value.total_count !== value.check_runs.length ||
    value.check_runs.length > 100
  )
    return invalid(definition, revision);

  const runs = new Map<
    string,
    { readonly status: string; readonly conclusion: string | null }
  >();
  for (const run of value.check_runs) {
    if (
      !record(run) ||
      typeof run.name !== "string" ||
      run.head_sha !== revision ||
      typeof run.status !== "string" ||
      (run.conclusion !== null && typeof run.conclusion !== "string")
    )
      return invalid(definition, revision);
    runs.set(run.name, {
      status: run.status,
      conclusion: run.conclusion,
    });
  }
  const required = definition.requiredChecks.map((name) => ({
    name,
    run: runs.get(name),
  }));
  const status = required.some(
    ({ run }) => run?.status === "completed" && run.conclusion !== "success",
  )
    ? "failure"
    : required.every(
          ({ run }) =>
            run?.status === "completed" && run.conclusion === "success",
        )
      ? "success"
      : "pending";
  const canonical = JSON.stringify({
    authority: definition.name,
    revision,
    status,
    checks: required.map(({ name, run }) => ({
      name,
      status: run?.status ?? "missing",
      conclusion: run?.conclusion ?? "missing",
    })),
  });
  const observationDigest = parseCiObservationDigest(
    createHash("sha256").update(canonical).digest("hex"),
  );
  return observationDigest.ok
    ? {
        ok: true,
        value: {
          authority: definition.name,
          revision,
          status,
          adapterDigest: definition.adapterSha256,
          observationDigest: observationDigest.value,
        },
      }
    : invalid(definition, revision);
}
