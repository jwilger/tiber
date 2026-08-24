import { createHash } from "node:crypto";
import type { TiberFailure } from "../failures/tiber-failure.js";
import { fail, succeed, type Result } from "../failures/tiber-failure.js";

declare const hindsightBrand: unique symbol;
type HindsightValue<P extends string> = string & {
  readonly [hindsightBrand]: P;
};
export type HindsightEndpoint = HindsightValue<"endpoint">;
export type HindsightBankId = HindsightValue<"bank-id">;
export type HindsightQuery = HindsightValue<"query">;
export type HindsightContent = HindsightValue<"content">;
export type HindsightDocumentId = HindsightValue<"document-id">;
export type HindsightScope = "global" | "private" | "shared";
export interface HindsightPermission {
  readonly recall: boolean;
  readonly retain: boolean;
}
export interface HindsightConfiguration {
  readonly endpoint: HindsightEndpoint;
  readonly banks: Readonly<Record<HindsightScope, HindsightBankId | undefined>>;
  readonly permissions: Readonly<Record<HindsightScope, HindsightPermission>>;
  readonly timeoutMs: number;
  readonly maximumResponseBytes: number;
}
export type HindsightFailureCode =
  | "TIBER_HINDSIGHT_VALUE_INVALID"
  | "TIBER_HINDSIGHT_ENDPOINT_DENIED"
  | "TIBER_HINDSIGHT_UNAVAILABLE"
  | "TIBER_HINDSIGHT_PERMISSION_DENIED"
  | "TIBER_HINDSIGHT_RESPONSE_INVALID"
  | "TIBER_HINDSIGHT_RESPONSE_OVERSIZED"
  | "TIBER_HINDSIGHT_HTTP_FAILED";
export type HindsightFailure = TiberFailure<
  HindsightFailureCode,
  { readonly domain: "hindsight" },
  | "corrected-memory-input"
  | "memory-permission"
  | "valid-memory-response"
  | "retry-memory-service"
>;
export type HindsightResult<T> = Result<T, HindsightFailure>;
function failure(
  code: HindsightFailureCode,
  message: string,
  evidence: HindsightFailure["requiredRecoveryEvidence"][number],
): HindsightResult<never> {
  return fail({
    code,
    message,
    safeContext: { domain: "hindsight" },
    causes: [],
    retryability: "retry-after-input",
    requiredRecoveryEvidence: [evidence],
    redaction: "public",
  });
}
function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function text(value: unknown, maximum: number): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    Buffer.byteLength(value) <= maximum
  );
}
function bank(value: string): HindsightBankId {
  return value as HindsightBankId;
}

export function parseHindsightConfiguration(
  value: unknown,
): HindsightResult<HindsightConfiguration> {
  if (
    !record(value) ||
    !text(value.endpoint, 2048) ||
    !text(value.repositoryIdentity, 4096) ||
    !text(value.userIdentity, 4096) ||
    !record(value.permissions)
  )
    return failure(
      "TIBER_HINDSIGHT_UNAVAILABLE",
      "Hindsight configuration is unavailable",
      "corrected-memory-input",
    );
  let endpoint: URL;
  try {
    endpoint = new URL(value.endpoint);
  } catch {
    return failure(
      "TIBER_HINDSIGHT_VALUE_INVALID",
      "Hindsight endpoint is invalid",
      "corrected-memory-input",
    );
  }
  const loopback =
    endpoint.protocol === "http:" &&
    endpoint.hostname === "127.0.0.1" &&
    endpoint.port !== "";
  const remote = endpoint.protocol === "https:";
  if (
    (!loopback && !remote) ||
    endpoint.username !== "" ||
    endpoint.password !== "" ||
    endpoint.search !== "" ||
    endpoint.hash !== "" ||
    endpoint.pathname !== "/"
  )
    return failure(
      "TIBER_HINDSIGHT_ENDPOINT_DENIED",
      "Hindsight endpoint is not authorized",
      "memory-permission",
    );
  const permissions: Readonly<Record<string, unknown>> = value.permissions;
  const parsePermission = (
    scope: HindsightScope,
  ): HindsightPermission | undefined => {
    const candidate = permissions[scope];
    return record(candidate) &&
      typeof candidate.recall === "boolean" &&
      typeof candidate.retain === "boolean"
      ? { recall: candidate.recall, retain: candidate.retain }
      : undefined;
  };
  const global = parsePermission("global");
  const privatePermission = parsePermission("private");
  const shared = parsePermission("shared");
  if (
    global === undefined ||
    privatePermission === undefined ||
    shared === undefined
  )
    return failure(
      "TIBER_HINDSIGHT_VALUE_INVALID",
      "Hindsight permissions are invalid",
      "corrected-memory-input",
    );
  const sharedBank = value.sharedBankId;
  if (
    sharedBank !== undefined &&
    (!text(sharedBank, 200) || !/^[A-Za-z0-9._-]+$/u.test(sharedBank))
  )
    return failure(
      "TIBER_HINDSIGHT_VALUE_INVALID",
      "Hindsight shared bank is invalid",
      "corrected-memory-input",
    );
  if ((shared.recall || shared.retain) && sharedBank === undefined)
    return failure(
      "TIBER_HINDSIGHT_PERMISSION_DENIED",
      "Shared memory requires an explicit bank opt-in",
      "memory-permission",
    );
  const digest = createHash("sha256")
    .update(value.repositoryIdentity)
    .digest("hex")
    .slice(0, 32);
  const userDigest = createHash("sha256")
    .update(value.userIdentity)
    .digest("hex")
    .slice(0, 32);
  return succeed({
    endpoint: endpoint.toString() as HindsightEndpoint,
    banks: {
      global: bank(`tiber-global-${userDigest}`),
      private: bank(`tiber-private-${digest}`),
      // Stryker disable next-line ConditionalExpression: bank is an identity brand constructor, so bank(undefined) is the same undefined runtime value; the branch establishes the semantic optional type.
      shared: sharedBank === undefined ? undefined : bank(sharedBank),
    },
    permissions: { global, private: privatePermission, shared },
    timeoutMs: 10_000,
    maximumResponseBytes: 262_144,
  });
}

export interface HindsightRecallRequest {
  readonly scope: HindsightScope;
  readonly query: HindsightQuery;
  readonly phase: "initial" | "explicit";
  readonly maximumTokens: number;
}
export function parseHindsightRecallRequest(
  value: unknown,
): HindsightResult<HindsightRecallRequest> {
  if (
    !record(value) ||
    (value.scope !== "global" &&
      value.scope !== "private" &&
      value.scope !== "shared") ||
    !text(value.query, 2000) ||
    (value.phase !== "initial" && value.phase !== "explicit")
  )
    return failure(
      "TIBER_HINDSIGHT_VALUE_INVALID",
      "Hindsight recall request is invalid",
      "corrected-memory-input",
    );
  return succeed({
    scope: value.scope,
    query: value.query.trim() as HindsightQuery,
    phase: value.phase,
    maximumTokens: value.phase === "initial" ? 256 : 1024,
  });
}

export interface HindsightRetentionCandidate {
  readonly scope: HindsightScope;
  readonly kind: "checkpoint" | "completion";
  readonly content: HindsightContent;
  readonly documentId: HindsightDocumentId;
  readonly reviewedCompletion: boolean;
  readonly includesRawOutput: boolean;
  readonly includesSource: boolean;
  readonly includesDiff: boolean;
}
export function parseHindsightRetentionCandidate(
  value: unknown,
): HindsightResult<HindsightRetentionCandidate> {
  if (
    !record(value) ||
    (value.scope !== "global" &&
      value.scope !== "private" &&
      value.scope !== "shared") ||
    (value.kind !== "checkpoint" && value.kind !== "completion") ||
    !text(value.content, 16_384) ||
    !text(value.documentId, 500) ||
    typeof value.reviewedCompletion !== "boolean" ||
    typeof value.includesRawOutput !== "boolean" ||
    typeof value.includesSource !== "boolean" ||
    typeof value.includesDiff !== "boolean"
  )
    return failure(
      "TIBER_HINDSIGHT_VALUE_INVALID",
      "Hindsight retention candidate is invalid",
      "corrected-memory-input",
    );
  return succeed({
    scope: value.scope,
    kind: value.kind,
    content: value.content as HindsightContent,
    documentId: value.documentId as HindsightDocumentId,
    reviewedCompletion: value.reviewedCompletion,
    includesRawOutput: value.includesRawOutput,
    includesSource: value.includesSource,
    includesDiff: value.includesDiff,
  });
}
export type HindsightRetentionDecision =
  | {
      readonly status: "authorized";
      readonly candidate: HindsightRetentionCandidate;
    }
  | {
      readonly status: "denied";
      readonly code:
        | "TIBER_HINDSIGHT_RAW_MATERIAL_EXCLUDED"
        | "TIBER_HINDSIGHT_SECRET_EXCLUDED"
        | "TIBER_HINDSIGHT_SHARED_COMPLETION_REQUIRED";
    };
const RAW_MATERIAL =
  /(?:^|\n)(?:diff --git |@@ |--- a\/|\+\+\+ b\/|```|--- stdout ---|--- stderr ---|(?:import|export|class|function|interface|type)\s)/u;
const SECRET =
  /(?:gh[pousr]_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----|(?:password|secret|token|api[_-]?key)\s*[:=]\s*\S{8,})/iu;
export function decideHindsightRetention(
  candidate: HindsightRetentionCandidate,
): HindsightRetentionDecision {
  if (
    candidate.includesRawOutput ||
    candidate.includesSource ||
    candidate.includesDiff ||
    RAW_MATERIAL.test(candidate.content)
  )
    return { status: "denied", code: "TIBER_HINDSIGHT_RAW_MATERIAL_EXCLUDED" };
  if (SECRET.test(candidate.content))
    return { status: "denied", code: "TIBER_HINDSIGHT_SECRET_EXCLUDED" };
  if (
    candidate.scope === "shared" &&
    (candidate.kind !== "completion" || !candidate.reviewedCompletion)
  )
    return {
      status: "denied",
      code: "TIBER_HINDSIGHT_SHARED_COMPLETION_REQUIRED",
    };
  return { status: "authorized", candidate };
}

export interface HindsightMemory {
  readonly id: string;
  readonly text: string;
  readonly type: "world" | "experience" | "observation";
  readonly tags: readonly string[];
}
export function parseHindsightRecallResponse(
  value: unknown,
): HindsightResult<readonly HindsightMemory[]> {
  if (
    !record(value) ||
    !Array.isArray(value.results) ||
    value.results.length > 20
  )
    return failure(
      "TIBER_HINDSIGHT_RESPONSE_INVALID",
      "Hindsight recall response is invalid",
      "valid-memory-response",
    );
  const memories: HindsightMemory[] = [];
  for (const item of value.results) {
    if (
      !record(item) ||
      !text(item.id, 500) ||
      !text(item.text, 16_384) ||
      (item.type !== "world" &&
        item.type !== "experience" &&
        item.type !== "observation") ||
      (item.tags !== undefined &&
        item.tags !== null &&
        (!Array.isArray(item.tags) ||
          item.tags.length > 20 ||
          !item.tags.every((tag) => text(tag, 200))))
    )
      return failure(
        "TIBER_HINDSIGHT_RESPONSE_INVALID",
        "Hindsight recall response is invalid",
        "valid-memory-response",
      );
    memories.push({
      id: item.id,
      text: item.text,
      type: item.type,
      tags: Array.isArray(item.tags) ? item.tags : [],
    });
  }
  return succeed(memories);
}
export function authorizeHindsightOperation(
  configuration: HindsightConfiguration,
  scope: HindsightScope,
  operation: "recall" | "retain",
): HindsightResult<HindsightBankId> {
  const target = configuration.banks[scope];
  if (!configuration.permissions[scope][operation] || target === undefined)
    return failure(
      "TIBER_HINDSIGHT_PERMISSION_DENIED",
      `Hindsight ${operation} is not permitted for ${scope} memory`,
      "memory-permission",
    );
  return succeed(target);
}
