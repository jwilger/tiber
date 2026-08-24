import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseCanonicalWorkflowJson,
  parseCompiledWorkflowDigest,
  parseGreenDiagnosticDigest,
  parseIncrementReviewFindingCount,
  parseIncrementReviewRationale,
  parseRedDiagnosticDigest,
  parseRedReviewRationale,
  parseScenarioFeatureText,
  parseSourceDiffDigest,
  parseWorkflowDefinitionId,
  parseWorkflowStepId,
  type CompiledWorkflowDigest,
  type GreenDiagnosticDigest,
  type RedDiagnosticDigest,
  type ScenarioFeatureText,
  type SourceDiffDigest,
} from "../../src/core/workflow/workflow-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("workflow semantic values", () => {
  it("keeps RED, GREEN, and source diff digests distinct", () => {
    expectTypeOf<RedDiagnosticDigest>().not.toEqualTypeOf<GreenDiagnosticDigest>();
    expectTypeOf<GreenDiagnosticDigest>().not.toEqualTypeOf<SourceDiffDigest>();
    expectTypeOf<CompiledWorkflowDigest>().not.toEqualTypeOf<SourceDiffDigest>();
    expectTypeOf<ScenarioFeatureText>().not.toEqualTypeOf<SourceDiffDigest>();
  });

  it("parses purpose-specific workflow values", () => {
    const digest = `sha256:${"a".repeat(64)}`;
    expect(parseCanonicalWorkflowJson("{}").ok).toBe(true);
    expect(parseCompiledWorkflowDigest(digest).ok).toBe(true);
    expect(parseWorkflowDefinitionId("tiber.default").ok).toBe(true);
    expect(parseWorkflowStepId("remote-claim").ok).toBe(true);
    expect(parseRedDiagnosticDigest(digest).ok).toBe(true);
    expect(parseGreenDiagnosticDigest(digest).ok).toBe(true);
    expect(parseSourceDiffDigest(digest).ok).toBe(true);
    expect(parseScenarioFeatureText("Feature: account deletion").ok).toBe(true);
    expect(
      parseRedReviewRationale("A sufficiently detailed RED rationale.").ok,
    ).toBe(true);
    expect(
      parseIncrementReviewRationale("A sufficiently detailed review rationale.")
        .ok,
    ).toBe(true);
    expect(parseIncrementReviewFindingCount(0).ok).toBe(true);
  });

  it("rejects coercible and out-of-bound workflow values", () => {
    expect(parseCanonicalWorkflowJson({ length: 1 }).ok).toBe(false);
    expect(parseCanonicalWorkflowJson("").ok).toBe(false);
    expect(
      parseWorkflowDefinitionId({ toString: () => "tiber.default" }).ok,
    ).toBe(false);
    expect(parseWorkflowStepId({ toString: () => "remote-claim" }).ok).toBe(
      false,
    );
    expect(parseScenarioFeatureText({ length: 1 }).ok).toBe(false);
    expect(parseScenarioFeatureText("x".repeat(65_536)).ok).toBe(true);
    expect(parseScenarioFeatureText("x".repeat(65_537)).ok).toBe(false);
    expect(
      parseCompiledWorkflowDigest({
        toString: () => `sha256:${"a".repeat(64)}`,
      }).ok,
    ).toBe(false);
    expect(
      parseRedReviewRationale({
        length: 20,
        trim() {
          return this;
        },
      }).ok,
    ).toBe(false);
    expect(parseRedReviewRationale(" leading whitespace rationale").ok).toBe(
      false,
    );
    expect(parseRedReviewRationale("x".repeat(4_000)).ok).toBe(true);
    expect(parseIncrementReviewRationale("x".repeat(4_001)).ok).toBe(false);
    expect(parseIncrementReviewFindingCount("0").ok).toBe(false);
  });

  it.each([
    [parseCanonicalWorkflowJson, "", "canonicalWorkflowJson"],
    [parseCompiledWorkflowDigest, "sha256:bad", "compiledWorkflowDigest"],
    [parseWorkflowDefinitionId, "Bad Workflow", "workflowDefinitionId"],
    [parseWorkflowStepId, "Bad Stage", "workflowStepId"],
    [parseRedDiagnosticDigest, "sha256:bad", "redDiagnosticDigest"],
    [parseGreenDiagnosticDigest, "sha256:bad", "greenDiagnosticDigest"],
    [parseSourceDiffDigest, "sha256:bad", "sourceDiffDigest"],
    [parseScenarioFeatureText, "", "scenarioFeatureText"],
    [parseRedReviewRationale, "short", "redReviewRationale"],
    [parseIncrementReviewRationale, "short", "incrementReviewRationale"],
    [parseIncrementReviewFindingCount, -1, "incrementReviewFindingCount"],
  ])("rejects malformed workflow values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_WORKFLOW_VALUE_INVALID", field),
    });
  });
});
