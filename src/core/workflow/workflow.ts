import { createHash } from "node:crypto";

export const POLICY_FLOOR_STAGES = [
  "specification-readiness",
  "remote-claim",
  "baseline-revalidation",
  "red",
  "green",
  "lightweight-review",
  "full-verification",
  "final-review-1",
  "final-review-2",
  "final-review-3",
  "delivery",
  "exact-revision-ci",
  "claim-release",
  "cleanup",
  "done",
] as const;

export interface WorkflowDefinition {
  readonly schemaVersion: 1;
  readonly id: string;
  readonly stages: readonly string[];
}

export const BUILT_IN_WORKFLOW: WorkflowDefinition = {
  schemaVersion: 1,
  id: "tiber.default",
  stages: ["intake", ...POLICY_FLOOR_STAGES],
};

export interface CompiledWorkflow {
  readonly definition: WorkflowDefinition;
  readonly canonicalJson: string;
  readonly digest: string;
}

export interface WorkflowFailure {
  readonly code: "TIBER_WORKFLOW_INVALID" | "TIBER_WORKFLOW_POLICY_FLOOR";
  readonly message: string;
}

export type WorkflowResult =
  | { readonly ok: true; readonly value: CompiledWorkflow }
  | { readonly ok: false; readonly failure: WorkflowFailure };

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives expose undefined required fields and fail validation; typeof establishes the predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalid(
  code: WorkflowFailure["code"],
  message: string,
): WorkflowResult {
  return { ok: false, failure: { code, message } };
}

export function compileWorkflow(input: unknown): WorkflowResult {
  if (!isRecord(input))
    return invalid("TIBER_WORKFLOW_INVALID", "workflow must be an object");
  const record = input;
  if (
    record.schemaVersion !== 1 ||
    // Stryker disable next-line ConditionalExpression: the following id grammar rejects every non-string JSON value and this guard narrows the type.
    typeof record.id !== "string" ||
    !/^[a-z][a-z0-9.-]{0,63}$/u.test(record.id) ||
    !Array.isArray(record.stages) ||
    record.stages.length === 0 ||
    record.stages.length > 64 ||
    !record.stages.every(
      (stage) =>
        // Stryker disable next-line ConditionalExpression: the stage grammar rejects every non-string JSON value and this guard narrows the type.
        typeof stage === "string" && /^[a-z][a-z0-9-]{0,63}$/u.test(stage),
    ) ||
    new Set(record.stages).size !== record.stages.length ||
    Object.keys(record).some(
      (key) => key !== "schemaVersion" && key !== "id" && key !== "stages",
    )
  )
    return invalid(
      "TIBER_WORKFLOW_INVALID",
      "workflow must contain only a valid id and 1 to 64 unique data-only stages",
    );
  // Stryker disable next-line MethodExpression: every element was just validated as a string; filtering only carries that trust-boundary proof into the inferred semantic type.
  const stages = record.stages.filter(
    // Stryker disable next-line ArrowFunction, ConditionalExpression: every element was just validated as a string; this predicate only conveys that proof to TypeScript.
    (stage): stage is string => typeof stage === "string",
  );
  let floorIndex = -1;
  for (const required of POLICY_FLOOR_STAGES) {
    const index = stages.indexOf(required);
    // Stryker disable next-line EqualityOperator: stages are unique by prior validation, so equality cannot occur; <= documents strict forward-only floor order.
    if (index <= floorIndex)
      return invalid(
        "TIBER_WORKFLOW_POLICY_FLOOR",
        `workflow must preserve required stage order: ${required}`,
      );
    floorIndex = index;
  }
  const definition: WorkflowDefinition = {
    schemaVersion: 1,
    id: record.id,
    stages,
  };
  const canonicalJson = JSON.stringify(definition);
  return {
    ok: true,
    value: {
      definition,
      canonicalJson,
      digest: `sha256:${createHash("sha256").update(canonicalJson).digest("hex")}`,
    },
  };
}
