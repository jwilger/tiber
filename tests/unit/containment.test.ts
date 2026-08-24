import { describe, expect, it } from "vitest";

import { none, some } from "../../src/core/types/option.js";
import {
  attestationExpiresAt,
  attestationIssuedAt,
  containmentAttestationNonce,
  containmentAttestationSignature,
  containmentEvaluationAt,
  containmentRepositoryPath,
  containmentVerifierIdentity,
} from "../fixtures/containment-values.js";

import {
  decideContainment,
  formatContainment,
  type ContainmentAttestation,
  type ContainmentEvidence,
} from "../../src/core/containment/containment.js";

const attestation: ContainmentAttestation = {
  schemaVersion: 1,
  level: "workspace-and-network-isolated",
  repositoryPath: containmentRepositoryPath("/workspace"),
  issuedAt: attestationIssuedAt("2026-01-01T00:00:00.000Z"),
  expiresAt: attestationExpiresAt("2027-01-01T00:00:00.000Z"),
  verifier: containmentVerifierIdentity("build-host"),
  nonce: containmentAttestationNonce("unique"),
  signature: containmentAttestationSignature("signature"),
};

const evidence: ContainmentEvidence = {
  attestation: some(attestation),
  signatureValid: true,
  linux: true,
  mountNamespaceIsolated: true,
  networkNamespaceIsolated: true,
  seccompEnabled: true,
};

function decide(overrides: Partial<ContainmentEvidence> = {}) {
  return decideContainment(
    "workspace-and-network-isolated",
    containmentRepositoryPath("/workspace"),
    containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
    { ...evidence, ...overrides },
  );
}

describe("containment authority", () => {
  it("does not claim strong isolation for host-trusted mode", () => {
    expect(
      decideContainment(
        "host-trusted",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
        {
          attestation: none,
          signatureValid: false,
          linux: false,
          mountNamespaceIsolated: false,
          networkNamespaceIsolated: false,
          seccompEnabled: false,
        },
      ),
    ).toEqual({
      state: "verified",
      level: "host-trusted",
      code: "TIBER_CONTAINMENT_HOST_TRUSTED",
      detail: "No strong containment requested",
    });
  });

  it("locks down a missing attestation", () => {
    const evidenceWithoutAttestation: ContainmentEvidence = {
      attestation: none,
      signatureValid: true,
      linux: true,
      mountNamespaceIsolated: true,
      networkNamespaceIsolated: true,
      seccompEnabled: true,
    };
    expect(
      decideContainment(
        "workspace-and-network-isolated",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
        evidenceWithoutAttestation,
      ),
    ).toEqual({
      state: "lockdown",
      level: "workspace-and-network-isolated",
      code: "TIBER_CONTAINMENT_ATTESTATION_MISSING",
      detail: "External containment attestation is required",
    });
  });

  it.each([
    [
      { signatureValid: false },
      "TIBER_CONTAINMENT_SIGNATURE_INVALID",
      "Attestation signature is invalid",
    ],
    [
      {
        attestation: some({
          ...attestation,
          repositoryPath: containmentRepositoryPath("/other"),
        }),
      },
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    ],
    [
      {
        attestation: some({
          ...attestation,
          level: "workspace-isolated" as const,
        }),
      },
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    ],
    [
      {
        attestation: some({
          ...attestation,
          expiresAt: attestationExpiresAt("2026-01-02T00:00:00.000Z"),
        }),
      },
      "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
      "Attestation is not currently valid",
    ],
    [
      {
        attestation: some({
          ...attestation,
          issuedAt: attestationIssuedAt("2026-12-01T00:00:00.000Z"),
        }),
      },
      "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
      "Attestation is not currently valid",
    ],
    [
      { linux: false },
      "TIBER_CONTAINMENT_LINUX_UNVERIFIED",
      "Linux mount namespace isolation was not corroborated",
    ],
    [
      { mountNamespaceIsolated: false },
      "TIBER_CONTAINMENT_LINUX_UNVERIFIED",
      "Linux mount namespace isolation was not corroborated",
    ],
    [
      { networkNamespaceIsolated: false },
      "TIBER_CONTAINMENT_NETWORK_UNVERIFIED",
      "Linux network namespace isolation was not corroborated",
    ],
  ] as const)(
    "locks down invalid evidence with %s",
    (overrides, code, detail) => {
      expect(decide(overrides)).toEqual({
        state: "lockdown",
        level: "workspace-and-network-isolated",
        code,
        detail,
      });
    },
  );

  it("treats issue time as inclusive and expiry time as exclusive", () => {
    expect(
      decideContainment(
        "workspace-and-network-isolated",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt(attestation.issuedAt),
        evidence,
      ),
    ).toMatchObject({ state: "verified" });
    expect(
      decideContainment(
        "workspace-and-network-isolated",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt(attestation.expiresAt),
        evidence,
      ),
    ).toMatchObject({
      state: "lockdown",
      code: "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
    });
  });

  it("verifies matching signed and corroborated evidence", () => {
    expect(decide()).toEqual({
      state: "verified",
      level: "workspace-and-network-isolated",
      code: "TIBER_CONTAINMENT_VERIFIED",
      detail:
        "External attestation and Linux isolation corroborated for workspace-and-network-isolated",
    });
  });

  it("requires seccomp only for hermetic mode", () => {
    const hermeticAttestation = { ...attestation, level: "hermetic" } as const;
    expect(
      decideContainment(
        "hermetic",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
        {
          ...evidence,
          attestation: some(hermeticAttestation),
          seccompEnabled: false,
        },
      ),
    ).toEqual({
      state: "lockdown",
      level: "hermetic",
      code: "TIBER_CONTAINMENT_SECCOMP_UNVERIFIED",
      detail: "Hermetic mode requires corroborated seccomp filtering",
    });
    expect(
      decideContainment(
        "hermetic",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
        {
          ...evidence,
          attestation: some(hermeticAttestation),
          networkNamespaceIsolated: false,
        },
      ),
    ).toMatchObject({
      state: "lockdown",
      code: "TIBER_CONTAINMENT_NETWORK_UNVERIFIED",
    });
    expect(
      decideContainment(
        "workspace-isolated",
        containmentRepositoryPath("/workspace"),
        containmentEvaluationAt("2026-06-01T00:00:00.000Z"),
        {
          ...evidence,
          attestation: some({
            ...attestation,
            level: "workspace-isolated" as const,
          }),
          networkNamespaceIsolated: false,
          seccompEnabled: false,
        },
      ),
    ).toMatchObject({ state: "verified" });
  });

  it("formats diagnostics without hiding the stable denial", () => {
    expect(formatContainment(decide({ signatureValid: false }))).toBe(
      "Containment: lockdown\nLevel: workspace-and-network-isolated\nCode: TIBER_CONTAINMENT_SIGNATURE_INVALID\nDetail: Attestation signature is invalid",
    );
  });
});
