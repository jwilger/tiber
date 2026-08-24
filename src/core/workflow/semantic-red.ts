import type { TaskSpecification } from "../tasks/readiness.js";

export interface RedObservation {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly scenarioName: string;
  readonly testMapping: string;
  readonly baselineRevision: string;
  readonly commandCatalogDigest: string;
  readonly commandName: string;
  readonly exitCode: number | null;
  readonly diagnosticDigest: string;
}

export interface RedReview {
  readonly freshContext: boolean;
  readonly reviewerRole: string;
  readonly reviewedDiagnosticDigest: string;
  readonly classification: "valid-red" | "unrelated-failure" | "invalid-red";
  readonly missingPublicSurface: boolean;
  readonly rationale: string;
}

export type RedDecision =
  | {
      readonly accepted: true;
      readonly receipt: {
        readonly taskId: string;
        readonly specificationDigest: string;
        readonly baselineRevision: string;
        readonly scenarioName: string;
        readonly testMapping: string;
        readonly diagnosticDigest: string;
        readonly missingPublicSurface: boolean;
      };
    }
  | { readonly accepted: false; readonly code: "TIBER_RED_REJECTED" };

export function projectScenarioFeature(
  specification: TaskSpecification,
  scenarioName: string,
):
  | { readonly ok: true; readonly feature: string }
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
  return { ok: true, feature: lines.join("\n") };
}

export function decideRedAcceptance(
  specification: TaskSpecification,
  observation: RedObservation,
  review: RedReview,
  authority: {
    readonly taskId: string;
    readonly specificationDigest: string;
    readonly baselineRevision: string;
    readonly commandCatalogDigest: string;
  },
): RedDecision {
  const scenario = specification.scenarios.find(
    (candidate) => candidate.name === observation.scenarioName,
  );
  if (
    observation.exitCode === 0 ||
    observation.exitCode === null ||
    scenario === undefined ||
    !specification.testMappings.includes(observation.testMapping) ||
    observation.taskId !== authority.taskId ||
    observation.specificationDigest !== authority.specificationDigest ||
    observation.baselineRevision !== authority.baselineRevision ||
    observation.commandCatalogDigest !== authority.commandCatalogDigest ||
    !review.freshContext ||
    review.reviewerRole !== "red-classifier" ||
    review.reviewedDiagnosticDigest !== observation.diagnosticDigest ||
    review.classification !== "valid-red" ||
    review.rationale.trim().length < 12
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
