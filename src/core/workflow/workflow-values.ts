import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const workflowValuePurpose: unique symbol;
type WorkflowValue<Value, Purpose extends string> = Value & {
  readonly [workflowValuePurpose]: Purpose;
};

export type WorkflowDefinitionId = WorkflowValue<
  string,
  "workflow-definition-id"
>;
export type WorkflowStepId = WorkflowValue<string, "workflow-step-id">;
export type CompiledWorkflowDigest = WorkflowValue<
  string,
  "compiled-workflow-digest"
>;
export type CanonicalWorkflowJson = WorkflowValue<
  string,
  "canonical-workflow-json"
>;
export type RedDiagnosticDigest = WorkflowValue<
  string,
  "red-diagnostic-digest"
>;
export type GreenDiagnosticDigest = WorkflowValue<
  string,
  "green-diagnostic-digest"
>;
export type SourceDiffDigest = WorkflowValue<string, "source-diff-digest">;
export type ScenarioFeatureText = WorkflowValue<
  string,
  "scenario-feature-text"
>;
export type RedReviewRationale = WorkflowValue<string, "red-review-rationale">;
export type IncrementReviewRationale = WorkflowValue<
  string,
  "increment-review-rationale"
>;
export type IncrementReviewFindingCount = WorkflowValue<
  number,
  "increment-review-finding-count"
>;

type Field =
  | "workflowDefinitionId"
  | "workflowStepId"
  | "compiledWorkflowDigest"
  | "canonicalWorkflowJson"
  | "redDiagnosticDigest"
  | "greenDiagnosticDigest"
  | "sourceDiffDigest"
  | "scenarioFeatureText"
  | "redReviewRationale"
  | "incrementReviewRationale"
  | "incrementReviewFindingCount";
type Failure = TiberFailure<
  "TIBER_WORKFLOW_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_WORKFLOW_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function digest<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<WorkflowValue<string, Purpose>> {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/u.test(value)
    ? { ok: true, value: value as WorkflowValue<string, Purpose> }
    : invalid(field);
}

export function parseWorkflowDefinitionId(
  value: unknown,
): Result<WorkflowDefinitionId> {
  return typeof value === "string" && /^[a-z][a-z0-9.-]{0,63}$/u.test(value)
    ? { ok: true, value: value as WorkflowDefinitionId }
    : invalid("workflowDefinitionId");
}

export function parseWorkflowStepId(value: unknown): Result<WorkflowStepId> {
  return typeof value === "string" && /^[a-z][a-z0-9-]{0,63}$/u.test(value)
    ? { ok: true, value: value as WorkflowStepId }
    : invalid("workflowStepId");
}

export function parseCompiledWorkflowDigest(
  value: unknown,
): Result<CompiledWorkflowDigest> {
  return digest(value, "compiledWorkflowDigest");
}

export function parseCanonicalWorkflowJson(
  value: unknown,
): Result<CanonicalWorkflowJson> {
  return typeof value === "string" && value.length > 0
    ? { ok: true, value: value as CanonicalWorkflowJson }
    : invalid("canonicalWorkflowJson");
}

export const parseRedDiagnosticDigest = (
  value: unknown,
): Result<RedDiagnosticDigest> => digest(value, "redDiagnosticDigest");
export const parseGreenDiagnosticDigest = (
  value: unknown,
): Result<GreenDiagnosticDigest> => digest(value, "greenDiagnosticDigest");
export function parseScenarioFeatureText(
  value: unknown,
): Result<ScenarioFeatureText> {
  return typeof value === "string" && value.length > 0 && value.length <= 65_536
    ? { ok: true, value: value as ScenarioFeatureText }
    : invalid("scenarioFeatureText");
}

export const parseSourceDiffDigest = (
  value: unknown,
): Result<SourceDiffDigest> => digest(value, "sourceDiffDigest");

function rationale<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<WorkflowValue<string, Purpose>> {
  return typeof value === "string" &&
    value.trim() === value &&
    value.length >= 12 &&
    value.length <= 4_000
    ? { ok: true, value: value as WorkflowValue<string, Purpose> }
    : invalid(field);
}

export const parseRedReviewRationale = (
  value: unknown,
): Result<RedReviewRationale> => rationale(value, "redReviewRationale");
export const parseIncrementReviewRationale = (
  value: unknown,
): Result<IncrementReviewRationale> =>
  rationale(value, "incrementReviewRationale");

export function parseIncrementReviewFindingCount(
  value: unknown,
): Result<IncrementReviewFindingCount> {
  // Stryker disable next-line ConditionalExpression, LogicalOperator: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? {
        ok: true,
        value: value as IncrementReviewFindingCount,
      }
    : invalid("incrementReviewFindingCount");
}
