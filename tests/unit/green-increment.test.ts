import { describe, expect, it } from "vitest";

import { none, some } from "../../src/core/types/option.js";
import { commandExitCode } from "../fixtures/artifact-values.js";
import {
  commandCatalogDigest,
  commandName,
} from "../fixtures/command-values.js";
import {
  claimBaselineRevision,
  scenarioName,
  specificationDigest,
  taskId,
  testMappingPath,
} from "../fixtures/task-values.js";
import {
  greenDiagnosticDigest,
  incrementReviewFindingCount,
  incrementReviewRationale,
  redDiagnosticDigest,
  sourceDiffDigest,
} from "../fixtures/workflow-values.js";

import {
  decideGreenIncrement,
  type GreenObservation,
  type LightweightReview,
} from "../../src/core/workflow/green-increment.js";

const authority = {
  taskId: taskId("2424c876-6180-4c64-976e-9ea4bd540744"),
  specificationDigest: specificationDigest(`sha256:${"a".repeat(64)}`),
  baselineRevision: claimBaselineRevision("b".repeat(40)),
  scenarioName: scenarioName("delete an existing account"),
  testMapping: testMappingPath("tests/account-deletion.test.ts"),
  redDiagnosticDigest: redDiagnosticDigest(`sha256:${"c".repeat(64)}`),
  commandCatalogDigest: commandCatalogDigest(`sha256:${"d".repeat(64)}`),
};
const observation: GreenObservation = {
  schemaVersion: 1,
  taskId: authority.taskId,
  specificationDigest: authority.specificationDigest,
  baselineRevision: authority.baselineRevision,
  scenarioName: authority.scenarioName,
  testMapping: authority.testMapping,
  commandCatalogDigest: authority.commandCatalogDigest,
  redDiagnosticDigest: authority.redDiagnosticDigest,
  commandName: commandName("test-account-deletion"),
  exitCode: some(commandExitCode(0)),
  diagnosticDigest: greenDiagnosticDigest(`sha256:${"e".repeat(64)}`),
  sourceDiffDigest: sourceDiffDigest(`sha256:${"f".repeat(64)}`),
};
const review: LightweightReview = {
  contextFreshness: "fresh",
  reviewerRole: "lightweight-increment-reviewer",
  reviewedScenarioName: observation.scenarioName,
  reviewedSourceDiffDigest: observation.sourceDiffDigest,
  findingCount: incrementReviewFindingCount(0),
  overimplementation: false,
  rationale: incrementReviewRationale(
    "The focused change implements only the mapped scenario.",
  ),
};

describe("GREEN increment gate", () => {
  it("accepts exact GREEN with a fresh clean minimal review", () => {
    expect(decideGreenIncrement(authority, observation, review)).toEqual({
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
    });
  });

  it("returns overimplementation and findings for rework", () => {
    expect(
      decideGreenIncrement(authority, observation, {
        ...review,
        findingCount: incrementReviewFindingCount(1),
        overimplementation: true,
      }),
    ).toEqual({
      state: "rework-required",
      refactorAllowed: false,
      code: "TIBER_INCREMENT_REWORK_REQUIRED",
    });
  });

  it("requires an observed successful exit and independently clean review", () => {
    expect(
      decideGreenIncrement(
        authority,
        { ...observation, exitCode: none },
        review,
      ),
    ).toMatchObject({ state: "red-reinstated" });
    expect(
      decideGreenIncrement(authority, observation, {
        ...review,
        findingCount: incrementReviewFindingCount(1),
        overimplementation: false,
      }),
    ).toMatchObject({ state: "rework-required" });
  });

  it("rejects a scenario mismatch even when review and observation agree", () => {
    const other = scenarioName("other");
    expect(
      decideGreenIncrement(
        authority,
        { ...observation, scenarioName: other },
        { ...review, reviewedScenarioName: other },
      ),
    ).toMatchObject({ state: "invalid" });
  });

  it("revokes refactor authority when the scenario becomes RED again", () => {
    expect(
      decideGreenIncrement(
        authority,
        { ...observation, exitCode: some(commandExitCode(1)) },
        review,
      ),
    ).toEqual({
      state: "red-reinstated",
      refactorAllowed: false,
      code: "TIBER_GREEN_NOT_OBSERVED",
    });
  });

  it.each([
    {
      observation: {
        ...observation,
        taskId: taskId("33333333-3333-4333-8333-333333333333"),
      },
      review,
    },
    {
      observation: {
        ...observation,
        specificationDigest: specificationDigest(`sha256:${"0".repeat(64)}`),
      },
      review,
    },
    {
      observation: {
        ...observation,
        baselineRevision: claimBaselineRevision("0".repeat(40)),
      },
      review,
    },
    {
      observation: { ...observation, scenarioName: scenarioName("other") },
      review,
    },
    {
      observation: {
        ...observation,
        testMapping: testMappingPath("tests/other.test.ts"),
      },
      review,
    },
    {
      observation: {
        ...observation,
        commandCatalogDigest: commandCatalogDigest(`sha256:${"0".repeat(64)}`),
      },
      review,
    },
    {
      observation: {
        ...observation,
        redDiagnosticDigest: redDiagnosticDigest(`sha256:${"0".repeat(64)}`),
      },
      review,
    },
    { observation, review: { ...review, contextFreshness: "stale" as const } },
    {
      observation,
      review: { ...review, reviewedScenarioName: scenarioName("other") },
    },
    {
      observation,
      review: {
        ...review,
        reviewedSourceDiffDigest: sourceDiffDigest(`sha256:${"0".repeat(64)}`),
      },
    },
  ])(
    "rejects stale or unbound GREEN %#",
    ({ observation: candidate, review: candidateReview }) => {
      expect(
        decideGreenIncrement(authority, candidate, candidateReview),
      ).toEqual({
        state: "invalid",
        refactorAllowed: false,
        code: "TIBER_GREEN_INVALID",
      });
    },
  );
});
