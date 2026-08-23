import { verify } from "node:crypto";
import { existsSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { join } from "node:path";

import {
  decideContainment,
  type ContainmentAttestation,
  type ContainmentStatus,
} from "../../core/containment/containment.js";
import type { AssuranceLevel } from "../../core/configuration/settings.js";

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
  const fields = [
    "repositoryPath",
    "issuedAt",
    "expiresAt",
    "verifier",
    "nonce",
    "signature",
  ] as const;
  if (fields.some((field) => typeof value[field] !== "string"))
    return undefined;
  return {
    schemaVersion: 1,
    level,
    repositoryPath: String(value.repositoryPath),
    issuedAt: String(value.issuedAt),
    expiresAt: String(value.expiresAt),
    verifier: String(value.verifier),
    nonce: String(value.nonce),
    signature: String(value.signature),
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
  return JSON.parse(readFileSync(path, "utf8")) as unknown;
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
  const repositoryPath = realpathSync(cwd);
  const path = join(repositoryPath, ".tiber", "containment-attestation.json");
  let attestation: ContainmentAttestation | undefined;
  if (existsSync(path)) {
    try {
      attestation = parseAttestation(readJson(path));
    } catch {
      attestation = undefined;
    }
  }
  return decideContainment(requested, repositoryPath, now, {
    ...(attestation === undefined ? {} : { attestation }),
    signatureValid:
      attestation === undefined
        ? false
        : signatureIsValid(attestation, agentDirectory),
    linux: process.platform === "linux",
    mountNamespaceIsolated: namespaceIsolated("mnt"),
    networkNamespaceIsolated: namespaceIsolated("net"),
    seccompEnabled: seccompEnabled(),
  });
}
