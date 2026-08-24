import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseAcceptanceCriterion,
  parseArchitectureImplication,
  parseClaimBaselineRevision,
  parseClaimOwnerIdentity,
  parseScenarioGivenStep,
  parseScenarioName,
  parseScenarioThenStep,
  parseScenarioWhenStep,
  parseSpecificationDependency,
  parseSpecificationDigest,
  parseSpecificationExclusion,
  parseSpecificationOutcome,
  parseSpecificationReviewFindingCount,
  parseTaskClaimId,
  parseTaskDescription,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  parseTaskTitle,
  parseTestMappingPath,
  type ClaimBaselineRevision,
  type TaskEventOccurredAt,
  type TaskId,
} from "../../src/core/tasks/task-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("task semantic values", () => {
  it("keeps structurally identical values distinct by purpose", () => {
    expectTypeOf<TaskId>().not.toEqualTypeOf<TaskEventOccurredAt>();
    expectTypeOf<ClaimBaselineRevision>().not.toEqualTypeOf<TaskId>();
  });

  it("parses each purpose at its trust boundary", () => {
    expect(parseTaskId("2424c876-6180-4c64-976e-9ea4bd540744").ok).toBe(true);
    expect(parseTaskEventOccurredAt("2026-08-23T12:00:00.000Z").ok).toBe(true);
    expect(parseClaimBaselineRevision("a".repeat(40)).ok).toBe(true);
    expect(parseSpecificationDigest(`sha256:${"b".repeat(64)}`).ok).toBe(true);
    expect(parseTestMappingPath("tests/unit/task-board.test.ts").ok).toBe(true);
  });

  it("returns a structured recoverable failure without reflecting input", () => {
    const result = parseTaskId("secret malformed input");
    expect(result).toEqual({
      ok: false,
      failure: {
        code: "TIBER_TASK_VALUE_INVALID",
        message: "Invalid taskId",
        safeContext: { field: "taskId" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-value"],
        redaction: "public",
      },
    });
  });

  it("rejects coercible and boundary-escaping task values", () => {
    expect(
      parseTaskId({ toString: () => "2424c876-6180-4c64-976e-9ea4bd540744" })
        .ok,
    ).toBe(false);
    expect(parseTaskEventOccurredAt(1_787_507_200_000).ok).toBe(false);
    expect(parseTaskEventOccurredAt("invalid")).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_TASK_VALUE_INVALID",
        "taskEventOccurredAt",
      ),
    });
    expect(parseTaskTitle("x".repeat(200)).ok).toBe(true);
    expect(parseTaskTitle("x".repeat(201)).ok).toBe(false);
    expect(parseTaskTitle(" title ").ok).toBe(false);
    expect(parseTaskDescription("x".repeat(10_000)).ok).toBe(true);
    expect(parseTaskDescription({ length: 1 }).ok).toBe(false);
    expect(
      parseClaimBaselineRevision({ toString: () => "a".repeat(40) }).ok,
    ).toBe(false);
    expect(parseClaimBaselineRevision(`x${"a".repeat(40)}`).ok).toBe(false);
    expect(parseClaimBaselineRevision(`${"a".repeat(40)}x`).ok).toBe(false);
    expect(
      parseSpecificationDigest({
        toString: () => `sha256:${"a".repeat(64)}`,
      }).ok,
    ).toBe(false);
    expect(parseSpecificationDigest(`xsha256:${"a".repeat(64)}`).ok).toBe(
      false,
    );
    expect(parseSpecificationDigest(`sha256:${"a".repeat(64)}x`).ok).toBe(
      false,
    );
    expect(parseTestMappingPath("x".repeat(500)).ok).toBe(true);
    expect(parseTestMappingPath("x".repeat(501)).ok).toBe(false);
    expect(parseTestMappingPath("").ok).toBe(false);
    expect(parseTestMappingPath({ length: 1 }).ok).toBe(false);
    expect(parseTestMappingPath("/absolute.ts").ok).toBe(false);
    expect(parseTestMappingPath("..").ok).toBe(false);
    expect(parseSpecificationReviewFindingCount("0").ok).toBe(false);
  });

  it.each([
    [parseTaskEventId, "bad", "taskEventId"],
    [parseTaskClaimId, "bad", "taskClaimId"],
    [parseTaskEventOccurredAt, "2026-08-23", "taskEventOccurredAt"],
    [parseTaskTitle, "", "taskTitle"],
    [parseTaskDescription, "x".repeat(10_001), "taskDescription"],
    [parseClaimOwnerIdentity, "", "claimOwnerIdentity"],
    [parseScenarioName, "", "scenarioName"],
    [parseScenarioGivenStep, "", "scenarioGivenStep"],
    [parseScenarioWhenStep, "", "scenarioWhenStep"],
    [parseScenarioThenStep, "", "scenarioThenStep"],
    [parseSpecificationOutcome, "", "specificationOutcome"],
    [parseAcceptanceCriterion, "", "acceptanceCriterion"],
    [parseSpecificationExclusion, "", "specificationExclusion"],
    [parseSpecificationDependency, "", "specificationDependency"],
    [parseArchitectureImplication, "", "architectureImplication"],
    [parseClaimBaselineRevision, "abc", "claimBaselineRevision"],
    [
      parseSpecificationDigest,
      `sha256:${"A".repeat(64)}`,
      "specificationDigest",
    ],
    [parseTestMappingPath, "../outside.ts", "testMappingPath"],
    [
      parseSpecificationReviewFindingCount,
      -1,
      "specificationReviewFindingCount",
    ],
  ])("rejects malformed external values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_TASK_VALUE_INVALID", field),
    });
  });
});
