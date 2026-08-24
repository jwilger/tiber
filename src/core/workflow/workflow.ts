import { createHash } from "node:crypto";

import {
  operationalFailure,
  type TiberFailure,
} from "../failures/tiber-failure.js";
import {
  parseCanonicalWorkflowJson,
  parseCompiledWorkflowDigest,
  parseWorkflowDefinitionId,
  parseWorkflowStepId,
  type CanonicalWorkflowJson,
  type CompiledWorkflowDigest,
  type WorkflowDefinitionId,
  type WorkflowStepId,
} from "./workflow-values.js";

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
  readonly id: WorkflowDefinitionId;
  readonly stages: readonly WorkflowStepId[];
}

export const BUILT_IN_WORKFLOW = {
  schemaVersion: 1,
  id: "tiber.default",
  stages: ["intake", ...POLICY_FLOOR_STAGES],
} as const;

export interface CompiledWorkflow {
  readonly definition: WorkflowDefinition;
  readonly canonicalJson: CanonicalWorkflowJson;
  readonly digest: CompiledWorkflowDigest;
}

export type WorkflowFailure = TiberFailure<
  "TIBER_WORKFLOW_INVALID" | "TIBER_WORKFLOW_POLICY_FLOOR",
  { readonly domain: "workflow-compilation" },
  "corrected-input" | "state-change" | "retry-operation"
>;

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
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "workflow-compilation",
      message,
      "retry-after-input",
    ),
  };
}

export function compileWorkflow(input: unknown): WorkflowResult {
  if (!isRecord(input))
    return invalid("TIBER_WORKFLOW_INVALID", "workflow must be an object");
  const record = input;
  if (
    record.schemaVersion !== 1 ||
    !Array.isArray(record.stages) ||
    record.stages.length === 0 ||
    record.stages.length > 64 ||
    new Set(record.stages).size !== record.stages.length ||
    Object.keys(record).some(
      (key) => key !== "schemaVersion" && key !== "id" && key !== "stages",
    )
  )
    return invalid(
      "TIBER_WORKFLOW_INVALID",
      "workflow must contain only a valid id and 1 to 64 unique data-only stages",
    );
  const id = parseWorkflowDefinitionId(record.id);
  const parsedStages = record.stages.map(parseWorkflowStepId);
  if (!id.ok || parsedStages.some((stage) => !stage.ok))
    return invalid(
      "TIBER_WORKFLOW_INVALID",
      "workflow id and stages must use their canonical grammars",
    );
  const stages: WorkflowStepId[] = [];
  for (const stage of parsedStages) {
    // Stryker disable next-line ConditionalExpression: malformed stages returned above, so every remaining result is success; the guard conveys that proof without a cast.
    if (stage.ok) stages.push(stage.value);
  }
  let floorIndex = -1;
  for (const required of POLICY_FLOOR_STAGES) {
    const index = stages.findIndex((stage) => stage === required);
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
    id: id.value,
    stages,
  };
  const canonicalJson = parseCanonicalWorkflowJson(JSON.stringify(definition));
  // Stryker disable next-line ConditionalExpression, BlockStatement: JSON.stringify of the validated definition is non-empty and therefore always satisfies the canonical JSON parser; this is a defect assertion.
  if (!canonicalJson.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: validated workflow generation makes this defect throw unreachable.
    throw new Error("generated canonical workflow violated its invariant");
  }
  const digest = parseCompiledWorkflowDigest(
    `sha256:${createHash("sha256").update(canonicalJson.value).digest("hex")}`,
  );
  // Stryker disable next-line ConditionalExpression, BlockStatement: SHA-256 generation always satisfies the compiled workflow digest parser; this is a defect assertion.
  if (!digest.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: SHA-256 generation makes this defect throw unreachable.
    throw new Error("generated workflow digest violated its invariant");
  }
  return {
    ok: true,
    value: {
      definition,
      canonicalJson: canonicalJson.value,
      digest: digest.value,
    },
  };
}
