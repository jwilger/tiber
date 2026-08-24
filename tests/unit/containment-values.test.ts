import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseAttestationExpiresAt,
  parseAttestationIssuedAt,
  parseContainmentAttestationNonce,
  parseContainmentAttestationSignature,
  parseContainmentEvaluationAt,
  parseContainmentRepositoryPath,
  parseContainmentVerifierIdentity,
  type AttestationExpiresAt,
  type AttestationIssuedAt,
  type ContainmentEvaluationAt,
  type ContainmentRepositoryPath,
} from "../../src/core/containment/containment-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("containment semantic values", () => {
  it("prevents interchange of structurally identical timestamps", () => {
    expectTypeOf<AttestationIssuedAt>().not.toEqualTypeOf<AttestationExpiresAt>();
    expectTypeOf<AttestationExpiresAt>().not.toEqualTypeOf<ContainmentEvaluationAt>();
    expectTypeOf<ContainmentRepositoryPath>().not.toEqualTypeOf<ContainmentEvaluationAt>();
  });

  it("parses each timestamp for its explicit purpose", () => {
    const timestamp = "2026-08-23T16:00:00.000Z";
    expect(parseAttestationIssuedAt(timestamp).ok).toBe(true);
    expect(parseAttestationExpiresAt(timestamp).ok).toBe(true);
    expect(parseContainmentEvaluationAt(timestamp).ok).toBe(true);
    expect(parseContainmentRepositoryPath("/workspace").ok).toBe(true);
    expect(parseContainmentVerifierIdentity("verifier").ok).toBe(true);
    expect(parseContainmentAttestationNonce("nonce").ok).toBe(true);
    expect(parseContainmentAttestationSignature("signature").ok).toBe(true);
  });

  it("rejects coercible and out-of-bound containment values", () => {
    expect(parseContainmentRepositoryPath("/").ok).toBe(false);
    expect(
      parseContainmentRepositoryPath({ toString: () => "/workspace" }).ok,
    ).toBe(false);
    expect(parseAttestationIssuedAt(1_787_507_200_000).ok).toBe(false);
    const coercible = {
      length: 1,
      trim() {
        return this;
      },
    };
    expect(parseContainmentVerifierIdentity(coercible).ok).toBe(false);
    expect(parseContainmentVerifierIdentity(" verifier").ok).toBe(false);
    expect(parseContainmentVerifierIdentity("x".repeat(320)).ok).toBe(true);
    expect(parseContainmentVerifierIdentity("x".repeat(321)).ok).toBe(false);
    expect(parseContainmentAttestationNonce("x".repeat(512)).ok).toBe(true);
    expect(parseContainmentAttestationNonce("x".repeat(513)).ok).toBe(false);
    expect(parseContainmentAttestationSignature("x".repeat(4_096)).ok).toBe(
      true,
    );
    expect(parseContainmentAttestationSignature("x".repeat(4_097)).ok).toBe(
      false,
    );
  });

  it.each([
    [parseAttestationIssuedAt, "2026-08-23", "attestationIssuedAt"],
    [parseAttestationExpiresAt, "invalid", "attestationExpiresAt"],
    [parseContainmentEvaluationAt, 0, "containmentEvaluationAt"],
    [parseContainmentRepositoryPath, "relative", "containmentRepositoryPath"],
    [parseContainmentVerifierIdentity, "", "containmentVerifierIdentity"],
    [parseContainmentAttestationNonce, "", "containmentAttestationNonce"],
    [
      parseContainmentAttestationSignature,
      "",
      "containmentAttestationSignature",
    ],
  ])("rejects malformed containment values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_CONTAINMENT_VALUE_INVALID",
        field,
      ),
    });
  });
});
