import type { DeliveryCommitRevision } from "../delivery/git-delivery-values.js";
import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

export type CiAuthorityName = string & { readonly __brand: "CiAuthorityName" };
export type CiRevision = DeliveryCommitRevision;
export type CiObservationDigest = string & {
  readonly __brand: "CiObservationDigest";
};
export type CiDiagnosis = string & { readonly __brand: "CiDiagnosis" };
export type CiExecutablePath = string & {
  readonly __brand: "CiExecutablePath";
};
export type CiExecutableDigest = string & {
  readonly __brand: "CiExecutableDigest";
};
export type CiAdapterArgument = string & {
  readonly __brand: "CiAdapterArgument";
};
export type CiGithubRepository = string & {
  readonly __brand: "CiGithubRepository";
};
export type CiGithubCheckName = string & {
  readonly __brand: "CiGithubCheckName";
};

export type CiValueResult<T> = TiberResult<
  T,
  TiberFailure<"TIBER_CI_VALUE_INVALID", { readonly field: "ci" }, string>
>;

function invalid(name: string): CiValueResult<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_CI_VALUE_INVALID",
      "ci",
      `${name} is invalid`,
    ),
  };
}

export function parseCiAuthorityName(
  input: unknown,
): CiValueResult<CiAuthorityName> {
  return typeof input === "string" && /^[a-z][a-z0-9-]{0,63}$/.test(input)
    ? { ok: true, value: input as CiAuthorityName }
    : invalid("CI authority name");
}

export function parseCiRevision(input: unknown): CiValueResult<CiRevision> {
  return typeof input === "string" && /^[0-9a-f]{40}$/.test(input)
    ? { ok: true, value: input as DeliveryCommitRevision }
    : invalid("CI revision");
}

export function parseCiObservationDigest(
  input: unknown,
): CiValueResult<CiObservationDigest> {
  return typeof input === "string" && /^[0-9a-f]{64}$/.test(input)
    ? { ok: true, value: input as CiObservationDigest }
    : invalid("CI observation digest");
}

export function parseCiDiagnosis(input: unknown): CiValueResult<CiDiagnosis> {
  return typeof input === "string" &&
    input.trim() === input &&
    input.length >= 16 &&
    input.length <= 2_000
    ? { ok: true, value: input as CiDiagnosis }
    : invalid("CI diagnosis");
}

export function parseCiExecutablePath(
  input: unknown,
): CiValueResult<CiExecutablePath> {
  return typeof input === "string" &&
    input.startsWith("/") &&
    input.length <= 4_096 &&
    !input.includes("\0")
    ? { ok: true, value: input as CiExecutablePath }
    : invalid("CI executable path");
}

export function parseCiExecutableDigest(
  input: unknown,
): CiValueResult<CiExecutableDigest> {
  return typeof input === "string" && /^[0-9a-f]{64}$/.test(input)
    ? { ok: true, value: input as CiExecutableDigest }
    : invalid("CI executable digest");
}

export function parseCiGithubRepository(
  input: unknown,
): CiValueResult<CiGithubRepository> {
  return typeof input === "string" &&
    /^[A-Za-z0-9_.-]{1,100}\/[A-Za-z0-9_.-]{1,100}$/u.test(input)
    ? { ok: true, value: input as CiGithubRepository }
    : invalid("GitHub CI repository");
}

export function parseCiGithubCheckName(
  input: unknown,
): CiValueResult<CiGithubCheckName> {
  return typeof input === "string" &&
    input.trim() === input &&
    input.length >= 1 &&
    input.length <= 256 &&
    !input.includes("\0")
    ? { ok: true, value: input as CiGithubCheckName }
    : invalid("GitHub CI check name");
}

export function parseCiAdapterArgument(
  input: unknown,
): CiValueResult<CiAdapterArgument> {
  return typeof input === "string" &&
    input.length <= 4_096 &&
    !input.includes("\0")
    ? { ok: true, value: input as CiAdapterArgument }
    : invalid("CI adapter argument");
}
