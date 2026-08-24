import { createHash } from "node:crypto";
import {
  fail,
  semanticValueFailure,
  succeed,
  type Result,
  type TiberFailure,
} from "../failures/tiber-failure.js";

declare const tokenCountBrand: unique symbol;
type TokenCount = number & { readonly [tokenCountBrand]: "TokenCount" };
declare const byteCountBrand: unique symbol;
type ByteCount = number & { readonly [byteCountBrand]: "ByteCount" };
export type ContextPriority =
  "authority" | "verification" | "goal" | "working" | "optional";
export interface ContextBudget {
  readonly contextWindowTokens: TokenCount;
  readonly reserveTokens: TokenCount;
  readonly hardInputTokens: TokenCount;
  readonly segmentByteLimit: ByteCount;
}
export interface ContextSegment {
  readonly id: string;
  readonly priority: ContextPriority;
  readonly content: string;
  readonly provenance: string;
}
export interface StableContext {
  readonly prompt: string;
  readonly initialContext: readonly string[];
  readonly toolSchemas: readonly string[];
}
export interface ContextPlan {
  readonly epochId: string;
  readonly stablePrefix: string;
  readonly dynamicSuffix: string;
  readonly context: string;
  readonly estimatedInputTokens: number;
  readonly includedSegmentIds: readonly string[];
  readonly omittedSegmentIds: readonly string[];
}
type ContextFailure = TiberFailure<string, unknown, unknown>;

const nonempty = (value: unknown): value is string =>
  typeof value === "string" && value.length > 0;
const invalid = (field: string) =>
  semanticValueFailure(
    "TIBER_CONTEXT_VALUE_INVALID",
    "context planning values",
    field,
  );
const exhausted = (): ContextFailure => ({
  code: "TIBER_CONTEXT_BUDGET_EXHAUSTED",
  message: "Mandatory context exceeds the hard input budget",
  safeContext: { domain: "context-headroom" },
  causes: [],
  retryability: "retry-after-state-change",
  requiredRecoveryEvidence: ["smaller-authoritative-context-or-larger-budget"],
  redaction: "public",
});
const hash = (value: string): string =>
  createHash("sha256").update(value).digest("hex");
const tokens = (value: string): number =>
  // Stryker disable next-line StringLiteral: Node treats an empty encoding argument as UTF-8, making that mutation behaviorally equivalent.
  Math.ceil(Buffer.byteLength(value, "utf8") / 4);

export function parseContextBudget(
  value: unknown,
): Result<ContextBudget, ContextFailure> {
  const document: Record<string, unknown> = Object.fromEntries(
    Object.entries(value ?? {}),
  );
  if (
    !Number.isSafeInteger(document.contextWindowTokens) ||
    !Number.isSafeInteger(document.reserveTokens) ||
    (document.reserveTokens as number) < 0 ||
    !Number.isSafeInteger(document.hardInputTokens) ||
    (document.hardInputTokens as number) <= 0 ||
    !Number.isSafeInteger(document.segmentByteLimit) ||
    (document.segmentByteLimit as number) <= 0 ||
    (document.hardInputTokens as number) >
      (document.contextWindowTokens as number) -
        (document.reserveTokens as number)
  )
    return fail(invalid("context budget"));
  return succeed({
    contextWindowTokens: document.contextWindowTokens as TokenCount,
    reserveTokens: document.reserveTokens as TokenCount,
    hardInputTokens: document.hardInputTokens as TokenCount,
    segmentByteLimit: document.segmentByteLimit as ByteCount,
  });
}

export function parseContextSegment(
  value: unknown,
): Result<ContextSegment, ContextFailure> {
  const document: Record<string, unknown> = Object.fromEntries(
    Object.entries(value ?? {}),
  );
  if (
    !nonempty(document.id) ||
    (document.priority !== "authority" &&
      document.priority !== "verification" &&
      document.priority !== "goal" &&
      document.priority !== "working" &&
      document.priority !== "optional") ||
    !nonempty(document.content) ||
    !nonempty(document.provenance)
  )
    return fail(invalid("context segment"));
  return succeed({
    id: document.id,
    priority: document.priority,
    content: document.content,
    provenance: document.provenance,
  });
}

function stablePrefix(stable: StableContext): string {
  return [
    "<tiber-cache-prefix-v1>",
    `prompt:${stable.prompt}`,
    ...stable.initialContext.map((value) => `context:${value}`),
    ...stable.toolSchemas.map((value) => `tool:${value}`),
    "</tiber-cache-prefix-v1>\n",
  ].join("\n");
}
function segmentText(segment: ContextSegment): string {
  return `<tiber-context priority="${segment.priority}" id="${segment.id}" provenance="${segment.provenance}">\n${segment.content}\n</tiber-context>\n`;
}
const order: Readonly<Record<ContextPriority, number>> = {
  authority: 0,
  verification: 1,
  goal: 2,
  working: 3,
  optional: 4,
};

export function planContext(input: {
  readonly budget: ContextBudget;
  readonly stable: StableContext;
  readonly dynamic: readonly ContextSegment[];
}): Result<ContextPlan, ContextFailure> {
  const prefix = stablePrefix(input.stable);
  const hardTokens = input.budget.hardInputTokens;
  if (tokens(prefix) > hardTokens) return fail(exhausted());
  const sorted = [...input.dynamic].sort(
    (left, right) => order[left.priority] - order[right.priority],
  );
  const included: ContextSegment[] = [];
  const omitted: string[] = [];
  let used = tokens(prefix);
  for (const value of sorted) {
    if (
      // Stryker disable next-line StringLiteral: Node treats an empty encoding argument as UTF-8, making that mutation behaviorally equivalent.
      Buffer.byteLength(value.content, "utf8") > input.budget.segmentByteLimit
    ) {
      if (value.priority === "authority" || value.priority === "verification")
        return fail(exhausted());
      omitted.push(value.id);
      continue;
    }
    const cost = tokens(segmentText(value));
    if (used + cost > hardTokens) {
      if (value.priority === "authority" || value.priority === "verification")
        return fail(exhausted());
      omitted.push(value.id);
      continue;
    }
    included.push(value);
    used += cost;
  }
  const suffix = included.map(segmentText).join("");
  return succeed({
    epochId: hash(prefix),
    stablePrefix: prefix,
    dynamicSuffix: suffix,
    context: `${prefix}${suffix}`,
    estimatedInputTokens: used,
    includedSegmentIds: included.map((value) => value.id),
    omittedSegmentIds: omitted,
  });
}

export type HeadroomDecision =
  | { readonly kind: "proceed"; readonly remainingTokens: number }
  | { readonly kind: "compact"; readonly reason: "reserve-bound" }
  | { readonly kind: "block"; readonly code: "TIBER_CONTEXT_BUDGET_EXHAUSTED" };
export function decideHeadroom(input: {
  readonly budget: ContextBudget;
  readonly plannedInputTokens: number;
  readonly observedContextTokens: number | null;
}): HeadroomDecision {
  if (input.plannedInputTokens > input.budget.hardInputTokens)
    return { kind: "block", code: "TIBER_CONTEXT_BUDGET_EXHAUSTED" };
  const observed = input.observedContextTokens ?? input.plannedInputTokens;
  const remaining = input.budget.contextWindowTokens - observed;
  return remaining <= input.budget.reserveTokens
    ? { kind: "compact", reason: "reserve-bound" }
    : { kind: "proceed", remainingTokens: remaining };
}

declare const cacheEpochIdBrand: unique symbol;
export type CacheEpochId = string & {
  readonly [cacheEpochIdBrand]: "CacheEpochId";
};
declare const compactionArtifactDigestBrand: unique symbol;
export type CompactionArtifactDigest = string & {
  readonly [compactionArtifactDigestBrand]: "CompactionArtifactDigest";
};
declare const compactionSummaryDigestBrand: unique symbol;
export type CompactionSummaryDigest = string & {
  readonly [compactionSummaryDigestBrand]: "CompactionSummaryDigest";
};
declare const compactionEntryIdBrand: unique symbol;
export type CompactionEntryId = string & {
  readonly [compactionEntryIdBrand]: "CompactionEntryId";
};
export interface CacheEpochTransition {
  readonly previousEpochId: CacheEpochId;
  readonly sourceArtifactDigest: CompactionArtifactDigest;
  readonly summaryDigest: CompactionSummaryDigest;
  readonly firstKeptEntryId: CompactionEntryId;
}
export interface CacheEpoch extends CacheEpochTransition {
  readonly epochId: CacheEpochId;
}
export function parseCacheEpochTransition(
  value: unknown,
): Result<CacheEpochTransition, ContextFailure> {
  const document: Record<string, unknown> = Object.fromEntries(
    Object.entries(value ?? {}),
  );
  if (
    typeof document.previousEpochId !== "string" ||
    !/^[a-f0-9]{64}$/u.test(document.previousEpochId) ||
    typeof document.sourceArtifactDigest !== "string" ||
    !/^[a-f0-9]{64}$/u.test(document.sourceArtifactDigest) ||
    typeof document.summaryDigest !== "string" ||
    !/^[a-f0-9]{64}$/u.test(document.summaryDigest) ||
    !nonempty(document.firstKeptEntryId)
  )
    return fail(invalid("cache epoch transition"));
  return succeed({
    previousEpochId: document.previousEpochId as CacheEpochId,
    sourceArtifactDigest:
      document.sourceArtifactDigest as CompactionArtifactDigest,
    summaryDigest: document.summaryDigest as CompactionSummaryDigest,
    firstKeptEntryId: document.firstKeptEntryId as CompactionEntryId,
  });
}
export function advanceCacheEpoch(input: CacheEpochTransition): CacheEpoch {
  return {
    ...input,
    epochId: hash(JSON.stringify(input)) as CacheEpochId,
  };
}
