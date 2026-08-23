import { createHash } from "node:crypto";

export interface GherkinScenario {
  readonly name: string;
  readonly given: readonly string[];
  readonly when: readonly string[];
  readonly then: readonly string[];
}

export interface TaskSpecification {
  readonly outcome: string;
  readonly scenarios: readonly GherkinScenario[];
  readonly acceptanceCriteria: readonly string[];
  readonly exclusions: readonly string[];
  readonly dependencies: readonly string[];
  readonly testMappings: readonly string[];
  readonly architectureImplications: string;
}

export interface ReadinessReview {
  readonly freshContext: boolean;
  readonly reviewerRole: "specification-reviewer";
  readonly findingCount: number;
  readonly reviewedSpecificationDigest: string;
}

export interface ReadinessDecision {
  readonly ready: boolean;
  readonly code: string;
  readonly reasons: readonly string[];
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives expose undefined required properties and are rejected by the parser; typeof establishes the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringArray(value: unknown): readonly string[] | undefined {
  return Array.isArray(value) && value.every((item) => typeof item === "string")
    ? value
    : undefined;
}

export function parseTaskSpecification(
  value: unknown,
): TaskSpecification | undefined {
  if (!isRecord(value) || !Array.isArray(value.scenarios)) return undefined;
  const outcome = typeof value.outcome === "string" ? value.outcome : undefined;
  const acceptanceCriteria = stringArray(value.acceptanceCriteria);
  const exclusions = stringArray(value.exclusions);
  const dependencies = stringArray(value.dependencies);
  const testMappings = stringArray(value.testMappings);
  const architectureImplications =
    typeof value.architectureImplications === "string"
      ? value.architectureImplications
      : undefined;
  const scenarios: GherkinScenario[] = [];
  for (const candidate of value.scenarios) {
    if (!isRecord(candidate) || typeof candidate.name !== "string")
      return undefined;
    const given = stringArray(candidate.given);
    const when = stringArray(candidate.when);
    const then = stringArray(candidate.then);
    if (given === undefined || when === undefined || then === undefined)
      return undefined;
    scenarios.push({ name: candidate.name, given, when, then });
  }
  if (
    outcome === undefined ||
    acceptanceCriteria === undefined ||
    exclusions === undefined ||
    dependencies === undefined ||
    testMappings === undefined ||
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

export function digestTaskSpecification(
  specification: TaskSpecification,
): string {
  return `sha256:${createHash("sha256").update(JSON.stringify(specification)).digest("hex")}`;
}

export function decideReadiness(
  specification: TaskSpecification,
  expectedDigest: string,
  review: ReadinessReview,
): ReadinessDecision {
  const reasons: string[] = [];
  if (specification.outcome.trim().length === 0)
    reasons.push("outcome is missing");
  if (specification.scenarios.length === 0)
    reasons.push("scenarios are missing");
  if (
    specification.scenarios.some(
      (scenario) =>
        scenario.name.trim().length === 0 ||
        scenario.given.length === 0 ||
        scenario.when.length === 0 ||
        scenario.then.length === 0,
    )
  )
    reasons.push("a scenario is structurally incomplete");
  if (specification.acceptanceCriteria.length === 0)
    reasons.push("acceptance criteria are missing");
  if (specification.exclusions.length === 0)
    reasons.push("exclusions are missing");
  if (specification.testMappings.length === 0)
    reasons.push("test mappings are missing");
  if (specification.architectureImplications.trim().length === 0)
    reasons.push("architecture implications are missing");
  if (!review.freshContext) reasons.push("review did not use fresh context");
  if (review.findingCount !== 0) reasons.push("review has unresolved findings");
  if (review.reviewedSpecificationDigest !== expectedDigest)
    reasons.push("review is stale");
  return reasons.length === 0
    ? { ready: true, code: "TIBER_SPECIFICATION_READY", reasons: [] }
    : { ready: false, code: "TIBER_SPECIFICATION_NOT_READY", reasons };
}
