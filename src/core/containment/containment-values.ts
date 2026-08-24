import { isAbsolute } from "node:path";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const containmentValuePurpose: unique symbol;
type ContainmentValue<Value, Purpose extends string> = Value & {
  readonly [containmentValuePurpose]: Purpose;
};

export type ContainmentRepositoryPath = ContainmentValue<
  string,
  "containment-repository-path"
>;
export type AttestationIssuedAt = ContainmentValue<
  string,
  "attestation-issued-at"
>;
export type AttestationExpiresAt = ContainmentValue<
  string,
  "attestation-expires-at"
>;
export type ContainmentEvaluationAt = ContainmentValue<
  string,
  "containment-evaluation-at"
>;
export type ContainmentVerifierIdentity = ContainmentValue<
  string,
  "containment-verifier-identity"
>;
export type ContainmentAttestationNonce = ContainmentValue<
  string,
  "containment-attestation-nonce"
>;
export type ContainmentAttestationSignature = ContainmentValue<
  string,
  "containment-attestation-signature"
>;

type Field =
  | "containmentRepositoryPath"
  | "attestationIssuedAt"
  | "attestationExpiresAt"
  | "containmentEvaluationAt"
  | "containmentVerifierIdentity"
  | "containmentAttestationNonce"
  | "containmentAttestationSignature";
type Failure = TiberFailure<
  "TIBER_CONTAINMENT_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_CONTAINMENT_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

export function parseContainmentRepositoryPath(
  value: unknown,
): Result<ContainmentRepositoryPath> {
  return typeof value === "string" &&
    isAbsolute(value) &&
    value.length > 1 &&
    !value.includes("\0")
    ? { ok: true, value: value as ContainmentRepositoryPath }
    : invalid("containmentRepositoryPath");
}

function timestamp<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<ContainmentValue<string, Purpose>> {
  // Stryker disable next-line ConditionalExpression: canonical ISO equality below independently rejects non-strings accepted by Date.parse; typeof establishes narrowing.
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return invalid(field);
  return new Date(value).toISOString() === value
    ? { ok: true, value: value as ContainmentValue<string, Purpose> }
    : invalid(field);
}

export const parseAttestationIssuedAt = (
  value: unknown,
): Result<AttestationIssuedAt> => timestamp(value, "attestationIssuedAt");
export const parseAttestationExpiresAt = (
  value: unknown,
): Result<AttestationExpiresAt> => timestamp(value, "attestationExpiresAt");
export const parseContainmentEvaluationAt = (
  value: unknown,
): Result<ContainmentEvaluationAt> =>
  timestamp(value, "containmentEvaluationAt");

function bounded<Purpose extends string>(
  value: unknown,
  field: Field,
  maximum: number,
): Result<ContainmentValue<string, Purpose>> {
  return typeof value === "string" &&
    value.trim() === value &&
    value.length > 0 &&
    value.length <= maximum
    ? { ok: true, value: value as ContainmentValue<string, Purpose> }
    : invalid(field);
}

export const parseContainmentVerifierIdentity = (
  value: unknown,
): Result<ContainmentVerifierIdentity> =>
  bounded(value, "containmentVerifierIdentity", 320);
export const parseContainmentAttestationNonce = (
  value: unknown,
): Result<ContainmentAttestationNonce> =>
  bounded(value, "containmentAttestationNonce", 512);
export const parseContainmentAttestationSignature = (
  value: unknown,
): Result<ContainmentAttestationSignature> =>
  bounded(value, "containmentAttestationSignature", 4_096);
