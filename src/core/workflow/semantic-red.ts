import type { CommandExitCode } from "../artifacts/artifact-values.js";
import type {
  CommandCatalogDigest,
  CommandName,
} from "../commands/command-values.js";
import type { ReviewContextFreshness } from "../reviews/review-values.js";
import type { TaskSpecification } from "../tasks/readiness.js";
import type { Option } from "../types/option.js";
import {
  parseScenarioFeatureText,
  type RedDiagnosticDigest,
  type RedReviewRationale,
  type ScenarioFeatureText,
} from "./workflow-values.js";
import type {
  ClaimBaselineRevision,
  ScenarioName,
  SpecificationDigest,
  TaskId,
  TestMappingPath,
} from "../tasks/task-values.js";

export interface RedObservation {
  readonly schemaVersion: 1;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly scenarioName: ScenarioName;
  readonly testMapping: TestMappingPath;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly commandCatalogDigest: CommandCatalogDigest;
  readonly commandName: CommandName;
  readonly exitCode: Option<CommandExitCode>;
  readonly diagnosticDigest: RedDiagnosticDigest;
}

export interface RedReview {
  readonly contextFreshness: ReviewContextFreshness;
  readonly reviewerRole: "red-classifier";
  readonly reviewedDiagnosticDigest: RedDiagnosticDigest;
  readonly classification: "valid-red" | "unrelated-failure" | "invalid-red";
  readonly missingPublicSurface: boolean;
  readonly rationale: RedReviewRationale;
}

export type RedDecision =
  | {
      readonly accepted: true;
      readonly receipt: {
        readonly taskId: TaskId;
        readonly specificationDigest: SpecificationDigest;
        readonly baselineRevision: ClaimBaselineRevision;
        readonly scenarioName: ScenarioName;
        readonly testMapping: TestMappingPath;
        readonly diagnosticDigest: RedDiagnosticDigest;
        readonly missingPublicSurface: boolean;
      };
    }
  | { readonly accepted: false; readonly code: "TIBER_RED_REJECTED" };

export function projectScenarioFeature(
  specification: TaskSpecification,
  scenarioName: ScenarioName,
):
  | { readonly ok: true; readonly feature: ScenarioFeatureText }
  | { readonly ok: false; readonly code: "TIBER_SCENARIO_UNKNOWN" } {
  const scenario = specification.scenarios.find(
    (candidate) => candidate.name === scenarioName,
  );
  if (scenario === undefined)
    return { ok: false, code: "TIBER_SCENARIO_UNKNOWN" };
  const lines = [
    `Feature: ${specification.outcome}`,
    "",
    `  Scenario: ${scenario.name}`,
    ...scenario.given.map((step) => `    Given ${step}`),
    ...scenario.when.map((step) => `    When ${step}`),
    ...scenario.then.map((step) => `    Then ${step}`),
    "",
  ];
  const feature = parseScenarioFeatureText(lines.join("\n"));
  // Stryker disable next-line ConditionalExpression, BlockStatement: every component was parsed into bounded scenario text and the generated feature remains within the parser's derived bound; this is a defect assertion.
  if (!feature.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: bounded feature generation makes this defect throw unreachable.
    throw new Error("generated scenario feature violated its invariant");
  }
  return { ok: true, feature: feature.value };
}

export function decideRedAcceptance(
  specification: TaskSpecification,
  observation: RedObservation,
  review: RedReview,
  authority: {
    readonly taskId: TaskId;
    readonly specificationDigest: SpecificationDigest;
    readonly baselineRevision: ClaimBaselineRevision;
    readonly commandCatalogDigest: CommandCatalogDigest;
  },
): RedDecision {
  const scenario = specification.scenarios.find(
    (candidate) => candidate.name === observation.scenarioName,
  );
  if (
    observation.exitCode.kind === "none" ||
    observation.exitCode.value === 0 ||
    scenario === undefined ||
    !specification.testMappings.includes(observation.testMapping) ||
    observation.taskId !== authority.taskId ||
    observation.specificationDigest !== authority.specificationDigest ||
    observation.baselineRevision !== authority.baselineRevision ||
    observation.commandCatalogDigest !== authority.commandCatalogDigest ||
    review.contextFreshness !== "fresh" ||
    review.reviewedDiagnosticDigest !== observation.diagnosticDigest ||
    review.classification !== "valid-red"
  )
    return { accepted: false, code: "TIBER_RED_REJECTED" };
  return {
    accepted: true,
    receipt: {
      taskId: observation.taskId,
      specificationDigest: observation.specificationDigest,
      baselineRevision: observation.baselineRevision,
      scenarioName: scenario.name,
      testMapping: observation.testMapping,
      diagnosticDigest: observation.diagnosticDigest,
      missingPublicSurface: review.missingPublicSurface,
    },
  };
}
