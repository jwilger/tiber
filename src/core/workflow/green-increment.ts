import type { CommandExitCode } from "../artifacts/artifact-values.js";
import type {
  CommandCatalogDigest,
  CommandName,
} from "../commands/command-values.js";
import type { ReviewContextFreshness } from "../reviews/review-values.js";
import type {
  ClaimBaselineRevision,
  ScenarioName,
  SpecificationDigest,
  TaskId,
  TestMappingPath,
} from "../tasks/task-values.js";
import type { Option } from "../types/option.js";
import type {
  GreenDiagnosticDigest,
  IncrementReviewFindingCount,
  IncrementReviewRationale,
  RedDiagnosticDigest,
  SourceDiffDigest,
} from "./workflow-values.js";

export interface GreenObservation {
  readonly schemaVersion: 1;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly scenarioName: ScenarioName;
  readonly testMapping: TestMappingPath;
  readonly commandCatalogDigest: CommandCatalogDigest;
  readonly redDiagnosticDigest: RedDiagnosticDigest;
  readonly commandName: CommandName;
  readonly exitCode: Option<CommandExitCode>;
  readonly diagnosticDigest: GreenDiagnosticDigest;
  readonly sourceDiffDigest: SourceDiffDigest;
}

export interface LightweightReview {
  readonly contextFreshness: ReviewContextFreshness;
  readonly reviewerRole: "lightweight-increment-reviewer";
  readonly reviewedScenarioName: ScenarioName;
  readonly reviewedSourceDiffDigest: SourceDiffDigest;
  readonly findingCount: IncrementReviewFindingCount;
  readonly overimplementation: boolean;
  readonly rationale: IncrementReviewRationale;
}

export type GreenDecision =
  | {
      readonly state: "review-clean";
      readonly refactorAllowed: true;
      readonly receipt: {
        readonly taskId: TaskId;
        readonly scenarioName: ScenarioName;
        readonly testMapping: TestMappingPath;
        readonly baselineRevision: ClaimBaselineRevision;
        readonly commandCatalogDigest: CommandCatalogDigest;
        readonly commandName: CommandName;
        readonly redDiagnosticDigest: RedDiagnosticDigest;
        readonly greenDiagnosticDigest: GreenDiagnosticDigest;
        readonly sourceDiffDigest: SourceDiffDigest;
        readonly reviewRationale: IncrementReviewRationale;
      };
    }
  | {
      readonly state: "rework-required";
      readonly refactorAllowed: false;
      readonly code: "TIBER_INCREMENT_REWORK_REQUIRED";
    }
  | {
      readonly state: "red-reinstated";
      readonly refactorAllowed: false;
      readonly code: "TIBER_GREEN_NOT_OBSERVED";
    }
  | {
      readonly state: "invalid";
      readonly refactorAllowed: false;
      readonly code: "TIBER_GREEN_INVALID";
    };

export function decideGreenIncrement(
  authority: {
    readonly taskId: TaskId;
    readonly specificationDigest: SpecificationDigest;
    readonly baselineRevision: ClaimBaselineRevision;
    readonly scenarioName: ScenarioName;
    readonly testMapping: TestMappingPath;
    readonly redDiagnosticDigest: RedDiagnosticDigest;
    readonly commandCatalogDigest: CommandCatalogDigest;
  },
  observation: GreenObservation,
  review: LightweightReview,
): GreenDecision {
  if (
    observation.taskId !== authority.taskId ||
    observation.specificationDigest !== authority.specificationDigest ||
    observation.baselineRevision !== authority.baselineRevision ||
    observation.scenarioName !== authority.scenarioName ||
    observation.testMapping !== authority.testMapping ||
    observation.commandCatalogDigest !== authority.commandCatalogDigest ||
    observation.redDiagnosticDigest !== authority.redDiagnosticDigest ||
    review.contextFreshness !== "fresh" ||
    review.reviewedScenarioName !== observation.scenarioName ||
    review.reviewedSourceDiffDigest !== observation.sourceDiffDigest
  )
    return {
      state: "invalid",
      refactorAllowed: false,
      code: "TIBER_GREEN_INVALID",
    };
  // Stryker disable next-line ConditionalExpression, StringLiteral: Option.none has no numeric value, so the success-code comparison independently rejects absence; the kind check documents the rail explicitly.
  if (observation.exitCode.kind === "none" || observation.exitCode.value !== 0)
    return {
      state: "red-reinstated",
      refactorAllowed: false,
      code: "TIBER_GREEN_NOT_OBSERVED",
    };
  if (review.findingCount !== 0 || review.overimplementation)
    return {
      state: "rework-required",
      refactorAllowed: false,
      code: "TIBER_INCREMENT_REWORK_REQUIRED",
    };
  return {
    state: "review-clean",
    refactorAllowed: true,
    receipt: {
      taskId: observation.taskId,
      scenarioName: observation.scenarioName,
      testMapping: observation.testMapping,
      baselineRevision: observation.baselineRevision,
      commandCatalogDigest: observation.commandCatalogDigest,
      commandName: observation.commandName,
      redDiagnosticDigest: observation.redDiagnosticDigest,
      greenDiagnosticDigest: observation.diagnosticDigest,
      sourceDiffDigest: observation.sourceDiffDigest,
      reviewRationale: review.rationale,
    },
  };
}
