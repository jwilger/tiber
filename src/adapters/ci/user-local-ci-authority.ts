import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { CiAuthorityObservation } from "../../core/ci/ci-authority.js";
import {
  parseCiAdapterArgument,
  parseCiAuthorityName,
  parseCiExecutableDigest,
  parseCiExecutablePath,
  parseCiGithubCheckName,
  parseCiGithubRepository,
  parseCiObservationDigest,
  parseCiRevision,
  type CiAdapterArgument,
  type CiAuthorityName,
  type CiExecutableDigest,
  type CiExecutablePath,
  type CiGithubCheckName,
  type CiGithubRepository,
  type CiRevision,
} from "../../core/ci/ci-values.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";

export interface CiAuthorityDefinition {
  readonly name: CiAuthorityName;
  readonly executable: CiExecutablePath;
  readonly executableSha256: CiExecutableDigest;
  readonly argv: readonly CiAdapterArgument[];
}

export interface GithubActionsAuthorityDefinition {
  readonly kind: "github-actions";
  readonly name: CiAuthorityName;
  readonly repository: CiGithubRepository;
  readonly requiredChecks: readonly CiGithubCheckName[];
  readonly adapterSha256: CiExecutableDigest;
}

export type ConfiguredCiAuthority =
  CiAuthorityDefinition | GithubActionsAuthorityDefinition;

export interface CiAuthorityCatalog {
  readonly schemaVersion: 1;
  readonly authorities: readonly ConfiguredCiAuthority[];
}

type CiAdapterFailure = TiberFailure<
  "TIBER_CI_ADAPTER_INVALID" | "TIBER_CI_ADAPTER_EXECUTION_FAILED",
  { readonly domain: "ci-adapter" },
  "corrected-input" | "state-change" | "retry-operation"
>;
export type CiAdapterResult<T> = TiberResult<T, CiAdapterFailure>;

function failure(
  code: CiAdapterFailure["code"],
  message: string,
): CiAdapterResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "ci-adapter",
      message,
      code === "TIBER_CI_ADAPTER_EXECUTION_FAILED"
        ? "transient"
        : "retry-after-input",
    ),
  };
}

export function invalidCiAdapter(message: string): CiAdapterResult<never> {
  return failure("TIBER_CI_ADAPTER_INVALID", message);
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(
  value: Readonly<Record<string, unknown>>,
  expected: readonly string[],
): boolean {
  return Object.keys(value).sort().join(",") === [...expected].sort().join(",");
}

export function parseCiAuthorityCatalog(
  input: unknown,
): CiAdapterResult<CiAuthorityCatalog> {
  if (
    !record(input) ||
    !exactKeys(input, ["schemaVersion", "authorities"]) ||
    input.schemaVersion !== 1 ||
    !Array.isArray(input.authorities) ||
    input.authorities.length === 0
  )
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "CI authority catalog shape is invalid",
    );

  const authorities: ConfiguredCiAuthority[] = [];
  for (const authority of input.authorities) {
    if (!record(authority))
      return failure(
        "TIBER_CI_ADAPTER_INVALID",
        "CI authority definition shape is invalid",
      );
    if (authority.kind === "github-actions") {
      if (
        !exactKeys(authority, [
          "kind",
          "name",
          "repository",
          "requiredChecks",
          "adapterSha256",
        ]) ||
        !Array.isArray(authority.requiredChecks) ||
        authority.requiredChecks.length === 0 ||
        authority.requiredChecks.length > 100
      )
        return failure(
          "TIBER_CI_ADAPTER_INVALID",
          "GitHub Actions authority shape is invalid",
        );
      const name = parseCiAuthorityName(authority.name);
      const repository = parseCiGithubRepository(authority.repository);
      const requiredChecks = authority.requiredChecks.map(
        parseCiGithubCheckName,
      );
      const adapterSha256 = parseCiExecutableDigest(authority.adapterSha256);
      if (
        !name.ok ||
        !repository.ok ||
        !adapterSha256.ok ||
        requiredChecks.some((check) => !check.ok)
      )
        return failure(
          "TIBER_CI_ADAPTER_INVALID",
          "GitHub Actions authority values are invalid",
        );
      const checks = requiredChecks.flatMap((check) =>
        check.ok ? [check.value] : [],
      );
      if (new Set(checks).size !== checks.length)
        return failure(
          "TIBER_CI_ADAPTER_INVALID",
          "GitHub Actions check names must be unique",
        );
      authorities.push({
        kind: "github-actions",
        name: name.value,
        repository: repository.value,
        requiredChecks: checks,
        adapterSha256: adapterSha256.value,
      });
      continue;
    }
    if (
      !exactKeys(authority, [
        "name",
        "executable",
        "executableSha256",
        "argv",
      ]) ||
      !Array.isArray(authority.argv)
    )
      return failure(
        "TIBER_CI_ADAPTER_INVALID",
        "CI authority definition shape is invalid",
      );
    const name = parseCiAuthorityName(authority.name);
    const executable = parseCiExecutablePath(authority.executable);
    const executableSha256 = parseCiExecutableDigest(
      authority.executableSha256,
    );
    const argv = authority.argv.map(parseCiAdapterArgument);
    if (
      !name.ok ||
      !executable.ok ||
      !executableSha256.ok ||
      argv.some((item) => !item.ok)
    )
      return failure(
        "TIBER_CI_ADAPTER_INVALID",
        "CI authority definition values are invalid",
      );
    const parsedArgv = argv.flatMap((item) => (item.ok ? [item.value] : []));
    if (parsedArgv.filter((argument) => argument === "{revision}").length !== 1)
      return failure(
        "TIBER_CI_ADAPTER_INVALID",
        "CI authority argv must contain one revision placeholder",
      );
    authorities.push({
      name: name.value,
      executable: executable.value,
      executableSha256: executableSha256.value,
      argv: parsedArgv,
    });
  }
  if (new Set(authorities.map(({ name }) => name)).size !== authorities.length)
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "CI authority names must be unique",
    );
  return { ok: true, value: { schemaVersion: 1, authorities } };
}

export function parseCiAuthorityOutput(
  expectedAuthorityInput: unknown,
  expectedRevisionInput: unknown,
  expectedAdapterDigestInput: unknown,
  output: string,
): CiAdapterResult<CiAuthorityObservation> {
  const expectedAuthority = parseCiAuthorityName(expectedAuthorityInput);
  const expectedRevision = parseCiRevision(expectedRevisionInput);
  const expectedAdapterDigest = parseCiExecutableDigest(
    expectedAdapterDigestInput,
  );
  let input: unknown;
  try {
    input = JSON.parse(output);
  } catch {
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "CI authority output is not JSON",
    );
  }
  if (
    !expectedAuthority.ok ||
    !expectedRevision.ok ||
    !expectedAdapterDigest.ok ||
    !record(input) ||
    !exactKeys(input, ["schemaVersion", "authority", "revision", "status"]) ||
    input.schemaVersion !== 1 ||
    (input.status !== "pending" &&
      input.status !== "success" &&
      input.status !== "failure")
  )
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "CI authority output schema is invalid",
    );
  const authority = parseCiAuthorityName(input.authority);
  const revision = parseCiRevision(input.revision);
  const digest = parseCiObservationDigest(
    createHash("sha256").update(output).digest("hex"),
  );
  if (
    !authority.ok ||
    !revision.ok ||
    !digest.ok ||
    authority.value !== expectedAuthority.value ||
    revision.value !== expectedRevision.value
  )
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "CI authority output identity does not match the request",
    );
  return {
    ok: true,
    value: {
      authority: authority.value,
      revision: revision.value,
      status: input.status,
      adapterDigest: expectedAdapterDigest.value,
      observationDigest: digest.value,
    },
  };
}

export function observeCiAuthority(
  definition: ConfiguredCiAuthority,
  revision: CiRevision,
): CiAdapterResult<CiAuthorityObservation> {
  if ("kind" in definition)
    return failure(
      "TIBER_CI_ADAPTER_INVALID",
      "GitHub Actions authority requires the first-party observer",
    );
  let executionDirectory: string | undefined;
  try {
    const executable = readFileSync(definition.executable);
    const observedDigest = createHash("sha256")
      .update(executable)
      .digest("hex");
    if (observedDigest !== definition.executableSha256)
      return failure(
        "TIBER_CI_ADAPTER_INVALID",
        "CI executable digest does not match its pin",
      );
    executionDirectory = mkdtempSync(join(tmpdir(), "tiber-ci-executable-"));
    const pinnedExecutable = join(executionDirectory, "adapter");
    writeFileSync(pinnedExecutable, executable, { mode: 0o700, flag: "wx" });
    chmodSync(pinnedExecutable, 0o700);
    const output = execFileSync(
      pinnedExecutable,
      definition.argv.map((argument) =>
        argument === "{revision}" ? revision : argument,
      ),
      {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
        timeout: 30_000,
        maxBuffer: 1_048_576,
        env: { PATH: process.env.PATH ?? "" },
      },
    );
    return parseCiAuthorityOutput(
      definition.name,
      revision,
      definition.executableSha256,
      output,
    );
  } catch {
    return failure(
      "TIBER_CI_ADAPTER_EXECUTION_FAILED",
      "CI authority execution failed",
    );
  } finally {
    if (executionDirectory !== undefined)
      rmSync(executionDirectory, { recursive: true, force: true });
  }
}
