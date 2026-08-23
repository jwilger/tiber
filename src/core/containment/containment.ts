import type { AssuranceLevel } from "../configuration/settings.js";

export interface ContainmentAttestation {
  readonly schemaVersion: 1;
  readonly level: Exclude<AssuranceLevel, "host-trusted">;
  readonly repositoryPath: string;
  readonly issuedAt: string;
  readonly expiresAt: string;
  readonly verifier: string;
  readonly nonce: string;
  readonly signature: string;
}

export interface ContainmentEvidence {
  readonly attestation?: ContainmentAttestation;
  readonly signatureValid: boolean;
  readonly linux: boolean;
  readonly mountNamespaceIsolated: boolean;
  readonly networkNamespaceIsolated: boolean;
  readonly seccompEnabled: boolean;
}

export interface ContainmentStatus {
  readonly state: "verified" | "lockdown";
  readonly level: AssuranceLevel;
  readonly code: string;
  readonly detail: string;
}

function lockdown(
  level: AssuranceLevel,
  code: string,
  detail: string,
): ContainmentStatus {
  return { state: "lockdown", level, code, detail };
}

export function decideContainment(
  requested: AssuranceLevel,
  repositoryPath: string,
  now: string,
  evidence: ContainmentEvidence,
): ContainmentStatus {
  if (requested === "host-trusted") {
    return {
      state: "verified",
      level: requested,
      code: "TIBER_CONTAINMENT_HOST_TRUSTED",
      detail: "No strong containment requested",
    };
  }
  const attestation = evidence.attestation;
  if (attestation === undefined) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_ATTESTATION_MISSING",
      "External containment attestation is required",
    );
  }
  if (!evidence.signatureValid) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_SIGNATURE_INVALID",
      "Attestation signature is invalid",
    );
  }
  if (
    attestation.repositoryPath !== repositoryPath ||
    attestation.level !== requested
  ) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    );
  }
  const nowMilliseconds = Date.parse(now);
  const issuedMilliseconds = Date.parse(attestation.issuedAt);
  const expiryMilliseconds = Date.parse(attestation.expiresAt);
  if (
    !Number.isFinite(nowMilliseconds) ||
    !Number.isFinite(issuedMilliseconds) ||
    !Number.isFinite(expiryMilliseconds) ||
    issuedMilliseconds > nowMilliseconds ||
    expiryMilliseconds <= nowMilliseconds
  ) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_ATTESTATION_EXPIRED",
      "Attestation is not currently valid",
    );
  }
  if (!evidence.linux || !evidence.mountNamespaceIsolated) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_LINUX_UNVERIFIED",
      "Linux mount namespace isolation was not corroborated",
    );
  }
  if (
    (requested === "workspace-and-network-isolated" ||
      requested === "hermetic") &&
    !evidence.networkNamespaceIsolated
  ) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_NETWORK_UNVERIFIED",
      "Linux network namespace isolation was not corroborated",
    );
  }
  if (requested === "hermetic" && !evidence.seccompEnabled) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_SECCOMP_UNVERIFIED",
      "Hermetic mode requires corroborated seccomp filtering",
    );
  }
  return {
    state: "verified",
    level: requested,
    code: "TIBER_CONTAINMENT_VERIFIED",
    detail: `External attestation and Linux isolation corroborated for ${requested}`,
  };
}

export function formatContainment(status: ContainmentStatus): string {
  return [
    `Containment: ${status.state}`,
    `Level: ${status.level}`,
    `Code: ${status.code}`,
    `Detail: ${status.detail}`,
  ].join("\n");
}
