import { createHash } from "node:crypto";

import type { TiberFailure, TiberResult } from "../failures/tiber-failure.js";
import type { ReviewContextFreshness } from "../reviews/review-values.js";
import {
  parseAcceptanceCriterion,
  parseArchitectureImplication,
  parseScenarioGivenStep,
  parseScenarioName,
  parseScenarioThenStep,
  parseScenarioWhenStep,
  parseSpecificationDependency,
  parseSpecificationDigest,
  parseSpecificationExclusion,
  parseSpecificationOutcome,
  parseTestMappingPath,
  type AcceptanceCriterion,
  type ArchitectureImplication,
  type ScenarioGivenStep,
  type ScenarioName,
  type ScenarioThenStep,
  type ScenarioWhenStep,
  type SpecificationDependency,
  type SpecificationDigest,
  type SpecificationExclusion,
  type SpecificationOutcome,
  type SpecificationReviewFindingCount,
  type TestMappingPath,
} from "./task-values.js";

export interface GherkinScenario {
  readonly name: ScenarioName;
  readonly given: readonly ScenarioGivenStep[];
  readonly when: readonly ScenarioWhenStep[];
  readonly then: readonly ScenarioThenStep[];
}

export interface TaskSpecification {
  readonly outcome: SpecificationOutcome;
  readonly scenarios: readonly GherkinScenario[];
  readonly acceptanceCriteria: readonly AcceptanceCriterion[];
  readonly exclusions: readonly SpecificationExclusion[];
  readonly dependencies: readonly SpecificationDependency[];
  readonly testMappings: readonly TestMappingPath[];
  readonly architectureImplications: ArchitectureImplication;
}

export interface ReadinessReview {
  readonly contextFreshness: ReviewContextFreshness;
  readonly reviewerRole: "specification-reviewer";
  readonly findingCount: SpecificationReviewFindingCount;
  readonly reviewedSpecificationDigest: SpecificationDigest;
}

export type ReadinessReason =
  | "review did not use fresh context"
  | "review has unresolved findings"
  | "review is stale";
export type ReadinessDecision =
  | {
      readonly status: "ready";
      readonly code: "TIBER_SPECIFICATION_READY";
      readonly reasons: readonly [];
    }
  | {
      readonly status: "not-ready";
      readonly code: "TIBER_SPECIFICATION_NOT_READY";
      readonly reasons: readonly ReadinessReason[];
    };

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives expose undefined required properties and are rejected by the parser; typeof establishes the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function semanticArray<Value>(
  value: unknown,
  parse: (
    candidate: unknown,
  ) => { readonly ok: true; readonly value: Value } | { readonly ok: false },
): readonly Value[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const parsed: Value[] = [];
  for (const candidate of value) {
    const result = parse(candidate);
    if (!result.ok) return undefined;
    parsed.push(result.value);
  }
  return parsed;
}

function parseTaskSpecificationValue(
  value: unknown,
): TaskSpecification | undefined {
  if (!isRecord(value) || !Array.isArray(value.scenarios)) return undefined;
  const outcomeResult = parseSpecificationOutcome(value.outcome);
  const outcome = outcomeResult.ok ? outcomeResult.value : undefined;
  const acceptanceCriteria = semanticArray(
    value.acceptanceCriteria,
    parseAcceptanceCriterion,
  );
  const exclusions = semanticArray(
    value.exclusions,
    parseSpecificationExclusion,
  );
  const dependencies = semanticArray(
    value.dependencies,
    parseSpecificationDependency,
  );
  const testMappings = semanticArray(value.testMappings, parseTestMappingPath);
  const architectureResult = parseArchitectureImplication(
    value.architectureImplications,
  );
  const architectureImplications = architectureResult.ok
    ? architectureResult.value
    : undefined;
  const scenarios: GherkinScenario[] = [];
  for (const candidate of value.scenarios) {
    if (!isRecord(candidate)) return undefined;
    const name = parseScenarioName(candidate.name);
    const given = semanticArray(candidate.given, parseScenarioGivenStep);
    const when = semanticArray(candidate.when, parseScenarioWhenStep);
    const then = semanticArray(candidate.then, parseScenarioThenStep);
    if (
      !name.ok ||
      given === undefined ||
      given.length === 0 ||
      when === undefined ||
      when.length === 0 ||
      then === undefined ||
      then.length === 0
    )
      return undefined;
    scenarios.push({ name: name.value, given, when, then });
  }
  if (
    outcome === undefined ||
    scenarios.length === 0 ||
    acceptanceCriteria === undefined ||
    acceptanceCriteria.length === 0 ||
    exclusions === undefined ||
    exclusions.length === 0 ||
    dependencies === undefined ||
    testMappings === undefined ||
    testMappings.length === 0 ||
    architectureImplications === undefined
  )
    return undefined;
  return {
    outcome,
    scenarios,
    acceptanceCriteria,
    exclusions,
    dependencies,
    testMappings,
    architectureImplications,
  };
}

type SpecificationParseFailure = TiberFailure<
  "TIBER_SPECIFICATION_INVALID",
  { readonly boundary: "task-specification" },
  "corrected-specification"
>;

export function parseTaskSpecification(
  value: unknown,
): TiberResult<TaskSpecification, SpecificationParseFailure> {
  const specification = parseTaskSpecificationValue(value);
  return specification === undefined
    ? {
        ok: false,
        failure: {
          code: "TIBER_SPECIFICATION_INVALID",
          message: "Task specification is malformed or incomplete",
          safeContext: { boundary: "task-specification" },
          causes: [],
          retryability: "retry-after-input",
          requiredRecoveryEvidence: ["corrected-specification"],
          redaction: "public",
        },
      }
    : { ok: true, value: specification };
}

export function digestTaskSpecification(
  specification: TaskSpecification,
): SpecificationDigest {
  const digest = parseSpecificationDigest(
    `sha256:${createHash("sha256").update(JSON.stringify(specification)).digest("hex")}`,
  );
  // Stryker disable next-line ConditionalExpression, BlockStatement: SHA-256 generation always satisfies the purpose-specific digest parser; this is a defect assertion.
  if (!digest.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: SHA-256 generation makes this defect throw unreachable in valid execution.
    throw new Error("generated specification digest violated its invariant");
  }
  return digest.value;
}

export function decideReadiness(
  expectedDigest: SpecificationDigest,
  review: ReadinessReview,
): ReadinessDecision {
  const reasons: ReadinessReason[] = [];
  if (review.contextFreshness !== "fresh")
    reasons.push("review did not use fresh context");
  if (review.findingCount !== 0) reasons.push("review has unresolved findings");
  if (review.reviewedSpecificationDigest !== expectedDigest)
    reasons.push("review is stale");
  return reasons.length === 0
    ? { status: "ready", code: "TIBER_SPECIFICATION_READY", reasons: [] }
    : {
        status: "not-ready",
        code: "TIBER_SPECIFICATION_NOT_READY",
        reasons,
      };
}
