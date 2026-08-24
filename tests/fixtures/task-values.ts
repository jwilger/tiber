import {
  parseClaimBaselineRevision,
  parseTaskClaimId,
  parseTaskEventOccurredAt,
  parseScenarioName,
  parseSpecificationDigest,
  parseTaskId,
  parseTestMappingPath,
} from "../../src/core/tasks/task-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid semantic value fixture");
  return result.value;
}

export const taskId = (value: string) => required(parseTaskId(value));
export const specificationDigest = (value: string) =>
  required(parseSpecificationDigest(value));
export const scenarioName = (value: string) =>
  required(parseScenarioName(value));
export const testMappingPath = (value: string) =>
  required(parseTestMappingPath(value));
export const claimBaselineRevision = (value: string) =>
  required(parseClaimBaselineRevision(value));
export const taskClaimId = (value: string) => required(parseTaskClaimId(value));
export const taskEventOccurredAt = (value: string) =>
  required(parseTaskEventOccurredAt(value));
