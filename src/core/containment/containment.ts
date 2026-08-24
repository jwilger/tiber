import type { AssuranceLevel } from "../configuration/settings.js";
import type { Option } from "../types/option.js";
import type {
  AttestationExpiresAt,
  AttestationIssuedAt,
  ContainmentAttestationNonce,
  ContainmentAttestationSignature,
  ContainmentEvaluationAt,
  ContainmentRepositoryPath,
  ContainmentVerifierIdentity,
} from "./containment-values.js";

export interface ContainmentAttestation {
  readonly schemaVersion: 1;
  readonly level: Exclude<AssuranceLevel, "host-trusted">;
  readonly repositoryPath: ContainmentRepositoryPath;
  readonly issuedAt: AttestationIssuedAt;
  readonly expiresAt: AttestationExpiresAt;
  readonly verifier: ContainmentVerifierIdentity;
  readonly nonce: ContainmentAttestationNonce;
  readonly signature: ContainmentAttestationSignature;
}

export interface ContainmentEvidence {
  readonly attestation: Option<ContainmentAttestation>;
  readonly signatureValid: boolean;
  readonly linux: boolean;
  readonly mountNamespaceIsolated: boolean;
  readonly networkNamespaceIsolated: boolean;
  readonly seccompEnabled: boolean;
}

export interface ContainmentStatus {
  readonly state: "verified" | "lockdown";
  readonly level: AssuranceLevel;
  readonly code: ContainmentStatusCode;
  readonly detail: string;
}

export type ContainmentStatusCode =
  | "TIBER_CONTAINMENT_ATTESTATION_EXPIRED"
  | "TIBER_CONTAINMENT_ATTESTATION_MISMATCH"
  | "TIBER_CONTAINMENT_ATTESTATION_MISSING"
  | "TIBER_CONTAINMENT_HOST_TRUSTED"
  | "TIBER_CONTAINMENT_CONFIGURATION_INVALID"
  | "TIBER_CONTAINMENT_LINUX_UNVERIFIED"
  | "TIBER_CONTAINMENT_NOT_INITIALIZED"
  | "TIBER_CONTAINMENT_NETWORK_UNVERIFIED"
  | "TIBER_CONTAINMENT_SECCOMP_UNVERIFIED"
  | "TIBER_CONTAINMENT_SIGNATURE_INVALID"
  | "TIBER_CONTAINMENT_VERIFIED"
  | "TIBER_TOOL_INVENTORY_COMPLETE"
  | "TIBER_TOOL_INVENTORY_INCOMPLETE";

function lockdown(
  level: AssuranceLevel,
  code: ContainmentStatusCode,
  detail: string,
): ContainmentStatus {
  return { state: "lockdown", level, code, detail };
}

export function decideContainment(
  requested: AssuranceLevel,
  repositoryPath: ContainmentRepositoryPath,
  now: ContainmentEvaluationAt,
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
  if (attestation.kind === "none") {
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
    attestation.value.repositoryPath !== repositoryPath ||
    attestation.value.level !== requested
  ) {
    return lockdown(
      requested,
      "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      "Attestation does not match the repository and requested level",
    );
  }
  const nowMilliseconds = Date.parse(now);
  const issuedMilliseconds = Date.parse(attestation.value.issuedAt);
  const expiryMilliseconds = Date.parse(attestation.value.expiresAt);
  if (
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
