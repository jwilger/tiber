import { verify } from "node:crypto";
import { existsSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { join } from "node:path";

import {
  decideContainment,
  type ContainmentAttestation,
  type ContainmentStatus,
} from "../../core/containment/containment.js";
import {
  parseAttestationExpiresAt,
  parseAttestationIssuedAt,
  parseContainmentAttestationNonce,
  parseContainmentAttestationSignature,
  parseContainmentEvaluationAt,
  parseContainmentRepositoryPath,
  parseContainmentVerifierIdentity,
} from "../../core/containment/containment-values.js";
import type { AssuranceLevel } from "../../core/configuration/settings.js";
import { none, some } from "../../core/types/option.js";

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseAttestation(value: unknown): ContainmentAttestation | undefined {
  if (!isRecord(value) || value.schemaVersion !== 1) return undefined;
  const level = value.level;
  if (
    level !== "workspace-isolated" &&
    level !== "workspace-and-network-isolated" &&
    level !== "hermetic"
  )
    return undefined;
  const repositoryPath = parseContainmentRepositoryPath(value.repositoryPath);
  const issuedAt = parseAttestationIssuedAt(value.issuedAt);
  const expiresAt = parseAttestationExpiresAt(value.expiresAt);
  const verifier = parseContainmentVerifierIdentity(value.verifier);
  const nonce = parseContainmentAttestationNonce(value.nonce);
  const signature = parseContainmentAttestationSignature(value.signature);
  if (
    !repositoryPath.ok ||
    !issuedAt.ok ||
    !expiresAt.ok ||
    !verifier.ok ||
    !nonce.ok ||
    !signature.ok
  )
    return undefined;
  return {
    schemaVersion: 1,
    level,
    repositoryPath: repositoryPath.value,
    issuedAt: issuedAt.value,
    expiresAt: expiresAt.value,
    verifier: verifier.value,
    nonce: nonce.value,
    signature: signature.value,
  };
}

function canonicalPayload(attestation: ContainmentAttestation): string {
  return JSON.stringify({
    schemaVersion: attestation.schemaVersion,
    level: attestation.level,
    repositoryPath: attestation.repositoryPath,
    issuedAt: attestation.issuedAt,
    expiresAt: attestation.expiresAt,
    verifier: attestation.verifier,
    nonce: attestation.nonce,
  });
}

function readJson(path: string): unknown {
  const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
  return parsed;
}

function signatureIsValid(
  attestation: ContainmentAttestation,
  agentDirectory: string,
): boolean {
  try {
    const keys = readJson(
      join(agentDirectory, "tiber", "containment-verifiers.json"),
    );
    if (!isRecord(keys)) return false;
    const publicKey = keys[attestation.verifier];
    return (
      typeof publicKey === "string" &&
      verify(
        null,
        Buffer.from(canonicalPayload(attestation)),
        publicKey,
        Buffer.from(attestation.signature, "base64"),
      )
    );
  } catch {
    return false;
  }
}

function namespaceIsolated(kind: "mnt" | "net"): boolean {
  try {
    return (
      readlinkSync(`/proc/self/ns/${kind}`) !==
      readlinkSync(`/proc/1/ns/${kind}`)
    );
  } catch {
    return false;
  }
}

function seccompEnabled(): boolean {
  try {
    const status = readFileSync("/proc/self/status", "utf8");
    return /^Seccomp:\s+[12]$/mu.test(status);
  } catch {
    return false;
  }
}

export function verifyFileContainment(
  requested: AssuranceLevel,
  cwd: string,
  agentDirectory: string,
  now = new Date().toISOString(),
): ContainmentStatus {
  const repositoryPath = parseContainmentRepositoryPath(realpathSync(cwd));
  const evaluationAt = parseContainmentEvaluationAt(now);
  if (!repositoryPath.ok || !evaluationAt.ok)
    return {
      state: "lockdown",
      level: requested,
      code: "TIBER_CONTAINMENT_ATTESTATION_MISMATCH",
      detail: "Containment verification inputs are invalid",
    };
  const path = join(
    repositoryPath.value,
    ".tiber",
    "containment-attestation.json",
  );
  let attestation: ContainmentAttestation | undefined;
  if (existsSync(path)) {
    try {
      attestation = parseAttestation(readJson(path));
    } catch {
      attestation = undefined;
    }
  }
  return decideContainment(
    requested,
    repositoryPath.value,
    evaluationAt.value,
    {
      attestation: attestation === undefined ? none : some(attestation),
      signatureValid:
        attestation === undefined
          ? false
          : signatureIsValid(attestation, agentDirectory),
      linux: process.platform === "linux",
      mountNamespaceIsolated: namespaceIsolated("mnt"),
      networkNamespaceIsolated: namespaceIsolated("net"),
      seccompEnabled: seccompEnabled(),
    },
  );
}
