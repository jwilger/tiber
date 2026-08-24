import { describe, expect, it } from "vitest";

import {
  decideRedAcceptance,
  projectScenarioFeature,
  type RedObservation,
  type RedReview,
} from "../../src/core/workflow/semantic-red.js";
import type { TaskSpecification } from "../../src/core/tasks/readiness.js";

const specification: TaskSpecification = {
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
};

const observation: RedObservation = {
  schemaVersion: 1,
  taskId: "2424c876-6180-4c64-976e-9ea4bd540744",
  specificationDigest: `sha256:${"a".repeat(64)}`,
  scenarioName: "delete an existing account",
  testMapping: "tests/account-deletion.test.ts",
  baselineRevision: "b".repeat(40),
  commandCatalogDigest: `sha256:${"c".repeat(64)}`,
  commandName: "test-account-deletion",
  exitCode: 1,
  diagnosticDigest: `sha256:${"d".repeat(64)}`,
};
const review: RedReview = {
  freshContext: true,
  reviewerRole: "red-classifier",
  reviewedDiagnosticDigest: observation.diagnosticDigest,
  classification: "valid-red",
  missingPublicSurface: false,
  rationale: "The mapped scenario failed because deletion behavior is absent.",
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
    expect(projectScenarioFeature(specification, "unknown")).toEqual({
      ok: false,
      code: "TIBER_SCENARIO_UNKNOWN",
    });
    const another: TaskSpecification = {
      ...specification,
      scenarios: [
        { name: "other", given: ["x"], when: ["y"], then: ["z"] },
        ...specification.scenarios,
      ],
    };
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
        { ...review, rationale: "123456789012" },
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
        { ...observation, exitCode: 2 },
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
    { observation: { ...observation, exitCode: 0 }, review },
    { observation: { ...observation, exitCode: null }, review },
    { observation: { ...observation, scenarioName: "unmapped" }, review },
    {
      observation: { ...observation, testMapping: "tests/unrelated.test.ts" },
      review,
    },
    { observation, review: { ...review, classification: "unrelated-failure" } },
    {
      observation: {
        ...observation,
        taskId: "33333333-3333-4333-8333-333333333333",
      },
      review,
    },
    { observation, review: { ...review, freshContext: false } },
    { observation, review: { ...review, reviewerRole: "other" } },
    { observation, review: { ...review, rationale: "            " } },
    {
      observation,
      review: {
        ...review,
        reviewedDiagnosticDigest: `sha256:${"e".repeat(64)}`,
      },
    },
    {
      observation: { ...observation, baselineRevision: "e".repeat(40) },
      review,
    },
    {
      observation: {
        ...observation,
        specificationDigest: `sha256:${"e".repeat(64)}`,
      },
      review,
    },
    {
      observation: {
        ...observation,
        commandCatalogDigest: `sha256:${"e".repeat(64)}`,
      },
      review,
    },
  ])(
    "rejects unrelated, passing, stale, or unbound RED %#",
    ({ observation: candidate, review: candidateReview }) => {
      expect(
        decideRedAcceptance(
          specification,
          candidate,
          candidateReview as RedReview,
          {
            taskId: observation.taskId,
            specificationDigest: observation.specificationDigest,
            baselineRevision: observation.baselineRevision,
            commandCatalogDigest: observation.commandCatalogDigest,
          },
        ),
      ).toMatchObject({ accepted: false, code: "TIBER_RED_REJECTED" });
    },
  );
});
