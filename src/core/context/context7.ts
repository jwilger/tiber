import { createHash } from "node:crypto";
import type { TiberFailure } from "../failures/tiber-failure.js";
import { fail, succeed, type Result } from "../failures/tiber-failure.js";

declare const context7Brand: unique symbol;
type Context7Value<Purpose extends string> = string & {
  readonly [context7Brand]: Purpose;
};
export type Context7Endpoint = Context7Value<"endpoint">;
export type Context7LibraryName = Context7Value<"library-name">;
export type Context7Query = Context7Value<"query">;
export type Context7LibraryId = Context7Value<"library-id">;
export type Context7Version = Context7Value<"version">;
export type Context7CacheKey = Context7Value<"cache-key">;

export type Context7FailureCode =
  | "TIBER_CONTEXT7_VALUE_INVALID"
  | "TIBER_CONTEXT7_ENDPOINT_DENIED"
  | "TIBER_CONTEXT7_NETWORK_UNAVAILABLE"
  | "TIBER_CONTEXT7_HTTP_FAILED"
  | "TIBER_CONTEXT7_RESPONSE_INVALID"
  | "TIBER_CONTEXT7_RESPONSE_OVERSIZED"
  | "TIBER_CONTEXT7_CACHE_INVALID"
  | "TIBER_CONTEXT7_CACHE_IO";
export type Context7Failure = TiberFailure<
  Context7FailureCode,
  { readonly domain: "context7" },
  | "corrected-context7-input"
  | "network-authority"
  | "valid-context7-response"
  | "retry-context7"
  | "valid-context7-cache"
>;
export type Context7Result<T> = Result<T, Context7Failure>;

function failure(
  code: Context7FailureCode,
  message: string,
  evidence: Context7Failure["requiredRecoveryEvidence"][number],
): Context7Result<never> {
  return fail({
    code,
    message,
    safeContext: { domain: "context7" },
    causes: [],
    retryability: "retry-after-input",
    requiredRecoveryEvidence: [evidence],
    redaction: "public",
  });
}
function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function bounded(value: unknown, maximum: number): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    Buffer.byteLength(value) <= maximum
  );
}

export function parseContext7Endpoint(
  value: unknown,
): Context7Result<Context7Endpoint> {
  if (!bounded(value, 2048))
    return failure(
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 endpoint is invalid",
      "corrected-context7-input",
    );
  try {
    const url = new URL(value);
    const official =
      url.protocol === "https:" &&
      url.hostname === "context7.com" &&
      url.port === "";
    const loopback =
      url.protocol === "http:" &&
      url.hostname === "127.0.0.1" &&
      url.port !== "";
    if (
      (!official && !loopback) ||
      url.username !== "" ||
      url.password !== "" ||
      url.search !== "" ||
      url.hash !== "" ||
      url.pathname !== "/api/v2"
    )
      return failure(
        "TIBER_CONTEXT7_ENDPOINT_DENIED",
        "Context7 endpoint is not authorized",
        "network-authority",
      );
    return succeed(url.toString().replace(/\/$/u, "") as Context7Endpoint);
  } catch {
    return failure(
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 endpoint is invalid",
      "corrected-context7-input",
    );
  }
}

export interface Context7NetworkCapability {
  readonly endpoint: Context7Endpoint;
  readonly maximumResponseBytes: number;
  readonly timeoutMs: number;
}
export function parseContext7NetworkCapability(
  value: unknown,
): Context7Result<Context7NetworkCapability> {
  if (!record(value) || value.enabled !== true)
    return failure(
      "TIBER_CONTEXT7_NETWORK_UNAVAILABLE",
      "Context7 network authority is unavailable",
      "network-authority",
    );
  const endpoint = parseContext7Endpoint(value.endpoint);
  if (!endpoint.ok) return endpoint;
  const maximumResponseBytes = value.maximumResponseBytes ?? 1_048_576;
  const timeoutMs = value.timeoutMs ?? 10_000;
  if (
    !Number.isInteger(maximumResponseBytes) ||
    Number(maximumResponseBytes) < 1024 ||
    Number(maximumResponseBytes) > 4_194_304 ||
    !Number.isInteger(timeoutMs) ||
    Number(timeoutMs) < 100 ||
    Number(timeoutMs) > 60_000
  )
    return failure(
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 network bounds are invalid",
      "corrected-context7-input",
    );
  return succeed({
    endpoint: endpoint.value,
    maximumResponseBytes: Number(maximumResponseBytes),
    timeoutMs: Number(timeoutMs),
  });
}

export interface Context7ResolveRequest {
  readonly libraryName: Context7LibraryName;
  readonly query: Context7Query;
}
export function parseContext7ResolveRequest(
  value: unknown,
): Context7Result<Context7ResolveRequest> {
  if (
    !record(value) ||
    !bounded(value.libraryName, 200) ||
    !bounded(value.query, 2000)
  )
    return failure(
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 resolution request is invalid",
      "corrected-context7-input",
    );
  return succeed({
    libraryName: value.libraryName.trim() as Context7LibraryName,
    query: value.query.trim() as Context7Query,
  });
}
export interface Context7QueryRequest {
  readonly libraryId: Context7LibraryId;
  readonly query: Context7Query;
}
export function parseContext7QueryRequest(
  value: unknown,
): Context7Result<Context7QueryRequest> {
  if (
    !record(value) ||
    !bounded(value.libraryId, 500) ||
    !/^\/[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+(?:\/[A-Za-z0-9._-]+)?$/u.test(
      value.libraryId,
    ) ||
    !bounded(value.query, 4000)
  )
    return failure(
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 documentation request is invalid",
      "corrected-context7-input",
    );
  return succeed({
    libraryId: value.libraryId as Context7LibraryId,
    query: value.query.trim() as Context7Query,
  });
}
export function context7CacheKey(
  kind: "resolve" | "docs",
  endpoint: Context7Endpoint,
  request: Context7ResolveRequest | Context7QueryRequest,
): Context7CacheKey {
  return createHash("sha256")
    .update(JSON.stringify({ kind, endpoint, request }))
    .digest("hex") as Context7CacheKey;
}

export interface Context7Library {
  readonly libraryId: Context7LibraryId;
  readonly title: string;
  readonly description: string;
  readonly version: Context7Version;
  readonly totalTokens: number;
}
export function parseContext7Libraries(
  value: unknown,
): Context7Result<readonly Context7Library[]> {
  if (
    !record(value) ||
    !Array.isArray(value.results) ||
    value.results.length > 20
  )
    return failure(
      "TIBER_CONTEXT7_RESPONSE_INVALID",
      "Context7 resolution response is invalid",
      "valid-context7-response",
    );
  const libraries: Context7Library[] = [];
  for (const item of value.results) {
    if (
      !record(item) ||
      !bounded(item.id, 500) ||
      !bounded(item.title, 500) ||
      typeof item.description !== "string" ||
      Buffer.byteLength(item.description) > 4000 ||
      !bounded(item.branch, 200) ||
      !Number.isSafeInteger(item.totalTokens) ||
      Number(item.totalTokens) < 0
    )
      return failure(
        "TIBER_CONTEXT7_RESPONSE_INVALID",
        "Context7 resolution response is invalid",
        "valid-context7-response",
      );
    const libraryId = parseContext7QueryRequest({
      libraryId: item.id,
      query: "validate",
    });
    if (!libraryId.ok)
      return failure(
        "TIBER_CONTEXT7_RESPONSE_INVALID",
        "Context7 library identifier is invalid",
        "valid-context7-response",
      );
    libraries.push({
      libraryId: libraryId.value.libraryId,
      title: item.title,
      description: item.description,
      version: item.branch as Context7Version,
      totalTokens: Number(item.totalTokens),
    });
  }
  return succeed(libraries);
}

export interface Context7Documentation {
  readonly text: string;
  readonly version: Context7Version;
}
export function parseContext7Documentation(
  value: unknown,
  libraryId?: Context7LibraryId,
): Context7Result<Context7Documentation> {
  if (!record(value))
    return failure(
      "TIBER_CONTEXT7_RESPONSE_INVALID",
      "Context7 documentation response is invalid",
      "valid-context7-response",
    );
  const structuredPayload =
    Array.isArray(value.codeSnippets) &&
    value.codeSnippets.length <= 100 &&
    value.codeSnippets.every(record) &&
    Array.isArray(value.infoSnippets) &&
    value.infoSnippets.length <= 100 &&
    value.infoSnippets.every(record)
      ? { codeSnippets: value.codeSnippets, infoSnippets: value.infoSnippets }
      : undefined;
  const structured =
    // Stryker disable next-line ConditionalExpression: JSON.stringify(undefined) also yields undefined; the branch preserves the semantic optional boundary explicitly.
    structuredPayload === undefined
      ? undefined
      : JSON.stringify(structuredPayload);
  const text =
    typeof value.text === "string"
      ? value.text
      : typeof value.content === "string"
        ? value.content
        : structured;
  const idVersion = libraryId?.split("/")[3];
  const version =
    typeof value.version === "string"
      ? value.version
      : (idVersion ?? "unspecified");
  if (!bounded(text, 4_194_304) || !bounded(version, 200))
    return failure(
      "TIBER_CONTEXT7_RESPONSE_INVALID",
      "Context7 documentation response is invalid",
      "valid-context7-response",
    );
  return succeed({ text, version: version as Context7Version });
}
