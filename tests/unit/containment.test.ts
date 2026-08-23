import { describe, expect, it } from "vitest";

import {
  decideContainment,
  formatContainment,
  type ContainmentAttestation,
  type ContainmentEvidence,
} from "../../src/core/containment/containment.js";

const attestation: ContainmentAttestation = {
  schemaVersion: 1,
  level: "workspace-and-network-isolated",
  repositoryPath: "/workspace",
  issuedAt: "2026-01-01T00:00:00.000Z",
  expiresAt: "2027-01-01T00:00:00.000Z",
  verifier: "build-host",
  nonce: "unique",
  signature: "signature",
};

const evidence: ContainmentEvidence = {
  attestation,
  signatureValid: true,
  linux: true,
  mountNamespaceIsolated: true,
  networkNamespaceIsolated: true,
  seccompEnabled: true,
};

function decide(overrides: Partial<ContainmentEvidence> = {}) {
  return decideContainment(
    "workspace-and-network-isolated",
    "/workspace",
    "2026-06-01T00:00:00.000Z",
    { ...evidence, ...overrides },
  );
}

describe("containment authority", () => {
  it("does not claim strong isolation for host-trusted mode", () => {
    expect(
      decideContainment("host-trusted", "/workspace", "invalid", {
        signatureValid: false,
        linux: false,
        mountNamespaceIsolated: false,
        networkNamespaceIsolated: false,
        seccompEnabled: false,
      }),
    ).toEqual({
      state: "verified",
      level: "host-trusted",
      code: "TIBER_CONTAINMENT_HOST_TRUSTED",
      detail: "No strong containment requested",
    });
  });

  it("locks down a missing attestation", () => {
    const evidenceWithoutAttestation: ContainmentEvidence = {
      signatureValid: true,
      linux: true,
      mountNamespaceIsolated: true,
      networkNamespaceIsolated: true,
      seccompEnabled: true,
    };
    expect(
      decideContainment(
        "workspace-and-network-isolated",
        "/workspace",
        "2026-06-01T00:00:00.000Z",
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
      { attestation: { ...attestation, repositoryPath: "/other" } },
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    ],
    [
      { attestation: { ...attestation, level: "workspace-isolated" } },
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    ],
    [
      {
        attestation: { ...attestation, expiresAt: "2026-01-02T00:00:00.000Z" },
      },
      "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
      "Attestation is not currently valid",
    ],
    [
      { attestation: { ...attestation, issuedAt: "2026-12-01T00:00:00.000Z" } },
      "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
      "Attestation is not currently valid",
    ],
    [
      { attestation: { ...attestation, issuedAt: "invalid" } },
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
        "/workspace",
        attestation.issuedAt,
        evidence,
      ),
    ).toMatchObject({ state: "verified" });
    expect(
      decideContainment(
        "workspace-and-network-isolated",
        "/workspace",
        attestation.expiresAt,
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
      decideContainment("hermetic", "/workspace", "2026-06-01T00:00:00.000Z", {
        ...evidence,
        attestation: hermeticAttestation,
        seccompEnabled: false,
      }),
    ).toEqual({
      state: "lockdown",
      level: "hermetic",
      code: "TIBER_CONTAINMENT_SECCOMP_UNVERIFIED",
      detail: "Hermetic mode requires corroborated seccomp filtering",
    });
    expect(
      decideContainment("hermetic", "/workspace", "2026-06-01T00:00:00.000Z", {
        ...evidence,
        attestation: hermeticAttestation,
        networkNamespaceIsolated: false,
      }),
    ).toMatchObject({
      state: "lockdown",
      code: "TIBER_CONTAINMENT_NETWORK_UNVERIFIED",
    });
    expect(
      decideContainment(
        "workspace-isolated",
        "/workspace",
        "2026-06-01T00:00:00.000Z",
        {
          ...evidence,
          attestation: { ...attestation, level: "workspace-isolated" },
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
