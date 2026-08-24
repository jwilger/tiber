import { describe, expect, it } from "vitest";

import { none, some } from "../../src/core/types/option.js";
import { commandExitCode } from "../fixtures/artifact-values.js";

import {
  decideRedAcceptance,
  projectScenarioFeature,
  type RedObservation,
  type RedReview,
} from "../../src/core/workflow/semantic-red.js";
import {
  commandCatalogDigest,
  commandName,
} from "../fixtures/command-values.js";
import { requireTaskSpecification } from "../fixtures/task-specification.js";
import {
  claimBaselineRevision,
  scenarioName,
  specificationDigest,
  taskId,
  testMappingPath,
} from "../fixtures/task-values.js";
import {
  redDiagnosticDigest,
  redReviewRationale,
} from "../fixtures/workflow-values.js";

const specification = requireTaskSpecification({
  outcome: "Expose account deletion",
  scenarios: [
    {
      name: "delete an existing account",
      given: ["an authenticated account exists"],
      when: ["the account is deleted"],
      then: ["the account can no longer authenticate"],
    },
  ],
  acceptanceCriteria: ["deletion is durable"],
  exclusions: ["account recovery"],
  dependencies: [],
  testMappings: ["tests/account-deletion.test.ts"],
  architectureImplications: "Deletion remains in the functional core.",
});

const observation: RedObservation = {
  schemaVersion: 1,
  taskId: taskId("2424c876-6180-4c64-976e-9ea4bd540744"),
  specificationDigest: specificationDigest(`sha256:${"a".repeat(64)}`),
  scenarioName: scenarioName("delete an existing account"),
  testMapping: testMappingPath("tests/account-deletion.test.ts"),
  baselineRevision: claimBaselineRevision("b".repeat(40)),
  commandCatalogDigest: commandCatalogDigest(`sha256:${"c".repeat(64)}`),
  commandName: commandName("test-account-deletion"),
  exitCode: some(commandExitCode(1)),
  diagnosticDigest: redDiagnosticDigest(`sha256:${"d".repeat(64)}`),
};
const review: RedReview = {
  contextFreshness: "fresh",
  reviewerRole: "red-classifier",
  reviewedDiagnosticDigest: observation.diagnosticDigest,
  classification: "valid-red",
  missingPublicSurface: false,
  rationale: redReviewRationale(
    "The mapped scenario failed because deletion behavior is absent.",
  ),
};

describe("semantically valid RED", () => {
  it("projects one exact task scenario into deterministic Gherkin", () => {
    expect(
      projectScenarioFeature(specification, observation.scenarioName),
    ).toEqual({
      ok: true,
      feature: [
        "Feature: Expose account deletion",
        "",
        "  Scenario: delete an existing account",
        "    Given an authenticated account exists",
        "    When the account is deleted",
        "    Then the account can no longer authenticate",
        "",
      ].join("\n"),
    });
  });

  it("rejects projection of an unknown scenario and selects by exact name", () => {
    expect(
      projectScenarioFeature(specification, scenarioName("unknown")),
    ).toEqual({
      ok: false,
      code: "TIBER_SCENARIO_UNKNOWN",
    });
    const another = requireTaskSpecification({
      ...specification,
      scenarios: [
        { name: "other", given: ["x"], when: ["y"], then: ["z"] },
        ...specification.scenarios,
      ],
    });
    const selected = projectScenarioFeature(another, observation.scenarioName);
    expect(selected.ok).toBe(true);
    if (selected.ok)
      expect(selected.feature).toContain(
        "Scenario: delete an existing account",
      );
  });

  it("accepts an exact independently classified scenario RED", () => {
    expect(
      decideRedAcceptance(specification, observation, review, {
        taskId: observation.taskId,
        specificationDigest: observation.specificationDigest,
        baselineRevision: observation.baselineRevision,
        commandCatalogDigest: observation.commandCatalogDigest,
      }),
    ).toEqual({
      accepted: true,
      receipt: {
        taskId: observation.taskId,
        specificationDigest: observation.specificationDigest,
        baselineRevision: observation.baselineRevision,
        scenarioName: observation.scenarioName,
        testMapping: observation.testMapping,
        diagnosticDigest: observation.diagnosticDigest,
        missingPublicSurface: false,
      },
    });
    expect(
      decideRedAcceptance(
        specification,
        observation,
        { ...review, rationale: redReviewRationale("123456789012") },
        {
          taskId: observation.taskId,
          specificationDigest: observation.specificationDigest,
          baselineRevision: observation.baselineRevision,
          commandCatalogDigest: observation.commandCatalogDigest,
        },
      ).accepted,
    ).toBe(true);
  });

  it("accepts a scenario-specific missing-public-surface compile failure", () => {
    expect(
      decideRedAcceptance(
        specification,
        { ...observation, exitCode: some(commandExitCode(2)) },
        { ...review, missingPublicSurface: true },
        {
          taskId: observation.taskId,
          specificationDigest: observation.specificationDigest,
          baselineRevision: observation.baselineRevision,
          commandCatalogDigest: observation.commandCatalogDigest,
        },
      ),
    ).toMatchObject({
      accepted: true,
      receipt: { missingPublicSurface: true },
    });
  });

  it.each([
    {
      observation: { ...observation, exitCode: some(commandExitCode(0)) },
      review,
    },
    { observation: { ...observation, exitCode: none }, review },
    {
      observation: { ...observation, scenarioName: scenarioName("unmapped") },
      review,
    },
    {
      observation: {
        ...observation,
        testMapping: testMappingPath("tests/unrelated.test.ts"),
      },
      review,
    },
    {
      observation,
      review: { ...review, classification: "unrelated-failure" as const },
    },
    {
      observation: {
        ...observation,
        taskId: taskId("33333333-3333-4333-8333-333333333333"),
      },
      review,
    },
    { observation, review: { ...review, contextFreshness: "stale" as const } },
    {
      observation,
      review: {
        ...review,
        rationale: redReviewRationale("valid but wrong rationale"),
        classification: "invalid-red" as const,
      },
    },
    {
      observation,
      review: {
        ...review,
        reviewedDiagnosticDigest: redDiagnosticDigest(
          `sha256:${"e".repeat(64)}`,
        ),
      },
    },
    {
      observation: {
        ...observation,
        baselineRevision: claimBaselineRevision("e".repeat(40)),
      },
      review,
    },
    {
      observation: {
        ...observation,
        specificationDigest: specificationDigest(`sha256:${"e".repeat(64)}`),
      },
      review,
    },
    {
      observation: {
        ...observation,
        commandCatalogDigest: commandCatalogDigest(`sha256:${"e".repeat(64)}`),
      },
      review,
    },
  ])(
    "rejects unrelated, passing, stale, or unbound RED %#",
    ({ observation: candidate, review: candidateReview }) => {
      expect(
        decideRedAcceptance(specification, candidate, candidateReview, {
          taskId: observation.taskId,
          specificationDigest: observation.specificationDigest,
          baselineRevision: observation.baselineRevision,
          commandCatalogDigest: observation.commandCatalogDigest,
        }),
      ).toMatchObject({ accepted: false, code: "TIBER_RED_REJECTED" });
    },
  );
});
