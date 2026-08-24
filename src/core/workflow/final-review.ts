import type { CommandCatalogDigest } from "../commands/command-values.js";
import type { ReviewContextFreshness } from "../reviews/review-values.js";
import type { Option } from "../types/option.js";
import type { PreservedIncrement } from "../tasks/task-board.js";
import type { TaskSpecification } from "../tasks/readiness.js";
import type { SpecificationDigest, TaskClaimId } from "../tasks/task-values.js";
import type {
  FinalReviewFindingCount,
  FinalReviewRationale,
  SourceSnapshotDigest,
  VerificationDiagnosticDigest,
} from "./workflow-values.js";

export type FinalReviewLens =
  "behavior" | "architecture" | "security" | "operability";

export interface FinalReviewRiskSignals {
  readonly securityRisk: "absent" | "present";
  readonly operationalRisk: "absent" | "present";
}

export interface AcceptanceVerificationReceipt {
  readonly claimId: TaskClaimId;
  readonly specificationDigest: SpecificationDigest;
  readonly commandCatalogDigest: CommandCatalogDigest;
  readonly diagnosticDigest: VerificationDiagnosticDigest;
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
}

export interface FinalLensReview {
  readonly lens: FinalReviewLens;
  readonly contextFreshness: ReviewContextFreshness;
  readonly findingCount: FinalReviewFindingCount;
  readonly rationale: FinalReviewRationale;
}

export interface FinalReviewIteration {
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
  readonly verificationDiagnosticDigest: VerificationDiagnosticDigest;
  readonly selectedLenses: readonly FinalReviewLens[];
  readonly reviews: readonly FinalLensReview[];
}

export interface FinalReviewProgress {
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
  readonly verificationDiagnosticDigest: VerificationDiagnosticDigest;
  readonly selectedLenses: readonly FinalReviewLens[];
  readonly cleanStreak: 0 | 1 | 2 | 3;
}

export type ReviewedCompletionDecision =
  | { readonly status: "authorized" }
  | {
      readonly status: "denied";
      readonly code:
        | "TIBER_FINAL_REVIEW_STREAK_REQUIRED"
        | "TIBER_FINAL_REVIEW_SOURCE_DELTA";
    };

export function decideReviewedCompletion(
  progress: FinalReviewProgress,
  observedSourceSnapshot: SourceSnapshotDigest,
): ReviewedCompletionDecision {
  if (progress.cleanStreak !== 3)
    return { status: "denied", code: "TIBER_FINAL_REVIEW_STREAK_REQUIRED" };
  return progress.sourceSnapshotDigest === observedSourceSnapshot
    ? { status: "authorized" }
    : { status: "denied", code: "TIBER_FINAL_REVIEW_SOURCE_DELTA" };
}

export type ScopeCompletionDecision =
  | { readonly status: "complete" }
  | {
      readonly status: "incomplete";
      readonly missingScenarios: readonly string[];
      readonly missingTestMappings: readonly string[];
    };

export function decideScopeCompletion(
  specification: TaskSpecification,
  increments: readonly PreservedIncrement[],
): ScopeCompletionDecision {
  const completedScenarios = new Set(
    increments.map((increment) => increment.scenarioName),
  );
  const completedMappings = new Set(
    increments.map((increment) => increment.testMapping),
  );
  const missingScenarios = specification.scenarios
    .map((scenario) => scenario.name)
    .filter((scenario) => !completedScenarios.has(scenario));
  const missingTestMappings = specification.testMappings.filter(
    (mapping) => !completedMappings.has(mapping),
  );
  return missingScenarios.length === 0 && missingTestMappings.length === 0
    ? { status: "complete" }
    : { status: "incomplete", missingScenarios, missingTestMappings };
}

export function finalReviewRiskSignals(
  specification: TaskSpecification,
): FinalReviewRiskSignals {
  const text = JSON.stringify(specification).toLowerCase();
  return {
    securityRisk:
      /\b(?:auth|permission|secret|security|credential|network)\b/u.test(text)
        ? "present"
        : "absent",
    operationalRisk:
      /\b(?:ci|deploy|process|recovery|release|runtime|delivery)\b/u.test(text)
        ? "present"
        : "absent",
  };
}

export function selectFinalReviewLenses(
  signals: FinalReviewRiskSignals,
): readonly FinalReviewLens[] {
  return [
    "behavior",
    "architecture",
    ...(signals.securityRisk === "present" ? (["security"] as const) : []),
    ...(signals.operationalRisk === "present"
      ? (["operability"] as const)
      : []),
  ];
}

function sameLenses(
  left: readonly FinalReviewLens[],
  right: readonly FinalReviewLens[],
): boolean {
  return (
    left.length === right.length &&
    left.every((lens, index) => lens === right[index])
  );
}

function nextCleanStreak(value: 0 | 1 | 2 | 3): 1 | 2 | 3 {
  if (value === 0) return 1;
  if (value === 1) return 2;
  return 3;
}

export function advanceFinalReview(
  previous: Option<FinalReviewProgress>,
  iteration: FinalReviewIteration,
): FinalReviewProgress {
  const complete =
    iteration.reviews.length === iteration.selectedLenses.length &&
    iteration.reviews.every(
      (review, index) =>
        review.lens === iteration.selectedLenses[index] &&
        review.contextFreshness === "fresh" &&
        review.findingCount === 0,
    );
  const sameEvidence =
    previous.kind === "some" &&
    previous.value.sourceSnapshotDigest === iteration.sourceSnapshotDigest &&
    previous.value.verificationDiagnosticDigest ===
      iteration.verificationDiagnosticDigest &&
    sameLenses(previous.value.selectedLenses, iteration.selectedLenses);
  const cleanStreak = complete
    ? sameEvidence
      ? nextCleanStreak(previous.value.cleanStreak)
      : 1
    : 0;
  return {
    sourceSnapshotDigest: iteration.sourceSnapshotDigest,
    verificationDiagnosticDigest: iteration.verificationDiagnosticDigest,
    selectedLenses: iteration.selectedLenses,
    cleanStreak,
  };
}
