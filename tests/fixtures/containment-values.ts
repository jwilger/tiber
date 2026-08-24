import {
  parseAttestationExpiresAt,
  parseAttestationIssuedAt,
  parseContainmentAttestationNonce,
  parseContainmentAttestationSignature,
  parseContainmentEvaluationAt,
  parseContainmentRepositoryPath,
  parseContainmentVerifierIdentity,
} from "../../src/core/containment/containment-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid containment semantic fixture");
  return result.value;
}

export const containmentRepositoryPath = (value: string) =>
  required(parseContainmentRepositoryPath(value));
export const attestationIssuedAt = (value: string) =>
  required(parseAttestationIssuedAt(value));
export const attestationExpiresAt = (value: string) =>
  required(parseAttestationExpiresAt(value));
export const containmentEvaluationAt = (value: string) =>
  required(parseContainmentEvaluationAt(value));
export const containmentVerifierIdentity = (value: string) =>
  required(parseContainmentVerifierIdentity(value));
export const containmentAttestationNonce = (value: string) =>
  required(parseContainmentAttestationNonce(value));
export const containmentAttestationSignature = (value: string) =>
  required(parseContainmentAttestationSignature(value));
