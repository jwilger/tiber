import {
  parseCompiledWorkflowDigest,
  parseFinalReviewFindingCount,
  parseFinalReviewRationale,
  parseGreenDiagnosticDigest,
  parseIncrementReviewFindingCount,
  parseIncrementReviewRationale,
  parseRedDiagnosticDigest,
  parseRedReviewRationale,
  parseSourceDiffDigest,
  parseSourceSnapshotDigest,
  parseVerificationDiagnosticDigest,
} from "../../src/core/workflow/workflow-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid workflow semantic fixture");
  return result.value;
}

export const compiledWorkflowDigest = (value: string) =>
  required(parseCompiledWorkflowDigest(value));
export const redDiagnosticDigest = (value: string) =>
  required(parseRedDiagnosticDigest(value));
export const greenDiagnosticDigest = (value: string) =>
  required(parseGreenDiagnosticDigest(value));
export const sourceDiffDigest = (value: string) =>
  required(parseSourceDiffDigest(value));
export const redReviewRationale = (value: string) =>
  required(parseRedReviewRationale(value));
export const incrementReviewRationale = (value: string) =>
  required(parseIncrementReviewRationale(value));
export const incrementReviewFindingCount = (value: number) =>
  required(parseIncrementReviewFindingCount(value));
export const finalReviewRationale = (value: string) =>
  required(parseFinalReviewRationale(value));
export const finalReviewFindingCount = (value: number) =>
  required(parseFinalReviewFindingCount(value));
export const sourceSnapshotDigest = (value: string) =>
  required(parseSourceSnapshotDigest(value));
export const verificationDiagnosticDigest = (value: string) =>
  required(parseVerificationDiagnosticDigest(value));
