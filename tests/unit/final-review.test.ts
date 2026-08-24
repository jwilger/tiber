import { describe, expect, it } from "vitest";

import {
  advanceFinalReview,
  decideReviewedCompletion,
  decideScopeCompletion,
  finalReviewRiskSignals,
  selectFinalReviewLenses,
  type FinalReviewIteration,
} from "../../src/core/workflow/final-review.js";
import { none, some } from "../../src/core/types/option.js";
import {
  commandCatalogDigest,
  commandName,
} from "../fixtures/command-values.js";
import {
  claimBaselineRevision,
  scenarioName,
  testMappingPath,
} from "../fixtures/task-values.js";
import {
  requireTaskSpecification,
  validTaskSpecificationDocument,
} from "../fixtures/task-specification.js";
import {
  finalReviewFindingCount,
  finalReviewRationale,
  greenDiagnosticDigest,
  incrementReviewRationale,
  redDiagnosticDigest,
  sourceDiffDigest,
  sourceSnapshotDigest,
  verificationDiagnosticDigest,
} from "../fixtures/workflow-values.js";

function increment(name: string, mapping: string) {
  return {
    scenarioName: scenarioName(name),
    testMapping: testMappingPath(mapping),
    baselineRevision: claimBaselineRevision("a".repeat(40)),
    commandCatalogDigest: commandCatalogDigest(`sha256:${"b".repeat(64)}`),
    commandName: commandName("acceptance"),
    redDiagnosticDigest: redDiagnosticDigest(`sha256:${"c".repeat(64)}`),
    greenDiagnosticDigest: greenDiagnosticDigest(`sha256:${"d".repeat(64)}`),
    sourceDiffDigest: sourceDiffDigest(`sha256:${"e".repeat(64)}`),
    reviewRationale: incrementReviewRationale(
      "The increment is minimal and clean.",
    ),
  };
}

const validTaskSpecification = requireTaskSpecification(
  validTaskSpecificationDocument,
);
const baseScenario = validTaskSpecification.scenarios[0];
if (baseScenario === undefined) throw new Error("missing scenario fixture");
const behaviorReview = {
  lens: "behavior" as const,
  contextFreshness: "fresh" as const,
  findingCount: finalReviewFindingCount(0),
  rationale: finalReviewRationale("Behavior is complete and correctly tested."),
};
const securityReview = {
  lens: "security" as const,
  contextFreshness: "fresh" as const,
  findingCount: finalReviewFindingCount(0),
  rationale: finalReviewRationale(
    "Security boundaries remain safely enforced.",
  ),
};
const snapshot = sourceSnapshotDigest(`sha256:${"1".repeat(64)}`);
const verification = verificationDiagnosticDigest(`sha256:${"2".repeat(64)}`);
const clean: FinalReviewIteration = {
  sourceSnapshotDigest: snapshot,
  verificationDiagnosticDigest: verification,
  selectedLenses: ["behavior", "security"],
  reviews: [behaviorReview, securityReview],
};

describe("multi-scenario completion", () => {
  it("rejects partial scenario and test-mapping completion", () => {
    const specification = {
      ...validTaskSpecification,
      scenarios: [
        baseScenario,
        { ...baseScenario, name: scenarioName("second") },
      ],
      testMappings: [
        testMappingPath("tests/first.test.ts"),
        testMappingPath("tests/second.test.ts"),
      ],
    };
    expect(
      decideScopeCompletion(specification, [
        increment(baseScenario.name, "tests/first.test.ts"),
      ]),
    ).toEqual({
      status: "incomplete",
      missingScenarios: ["second"],
      missingTestMappings: ["tests/second.test.ts"],
    });
    expect(
      decideScopeCompletion(specification, [
        increment(baseScenario.name, "tests/first.test.ts"),
        increment("second", "tests/first.test.ts"),
      ]),
    ).toMatchObject({
      status: "incomplete",
      missingScenarios: [],
      missingTestMappings: ["tests/second.test.ts"],
    });
    expect(
      decideScopeCompletion(specification, [
        increment(baseScenario.name, "tests/first.test.ts"),
        increment(baseScenario.name, "tests/second.test.ts"),
      ]),
    ).toMatchObject({
      status: "incomplete",
      missingScenarios: ["second"],
      missingTestMappings: [],
    });
    expect(
      decideScopeCompletion(specification, [
        increment(baseScenario.name, "tests/first.test.ts"),
        increment("second", "tests/second.test.ts"),
      ]),
    ).toEqual({ status: "complete" });
  });
});

describe("risk-selected complete final review", () => {
  it("selects only deterministic applicable lenses", () => {
    expect(finalReviewRiskSignals(validTaskSpecification)).toEqual({
      securityRisk: "absent",
      operationalRisk: "absent",
    });
    expect(
      finalReviewRiskSignals(
        requireTaskSpecification({
          ...validTaskSpecificationDocument,
          outcome: "SECURITY and RELEASE behavior",
        }),
      ),
    ).toEqual({ securityRisk: "present", operationalRisk: "present" });
    expect(
      selectFinalReviewLenses({
        securityRisk: "absent",
        operationalRisk: "present",
      }),
    ).toEqual(["behavior", "architecture", "operability"]);
    expect(
      selectFinalReviewLenses({
        securityRisk: "present",
        operationalRisk: "absent",
      }),
    ).toEqual(["behavior", "architecture", "security"]);
  });

  it("authorizes completion only for the exact three-clean source snapshot", () => {
    const progress = advanceFinalReview(
      some(advanceFinalReview(some(advanceFinalReview(none, clean)), clean)),
      clean,
    );
    expect(decideReviewedCompletion(progress, snapshot)).toEqual({
      status: "authorized",
    });
    expect(
      decideReviewedCompletion(
        progress,
        sourceSnapshotDigest(`sha256:${"9".repeat(64)}`),
      ),
    ).toEqual({
      status: "denied",
      code: "TIBER_FINAL_REVIEW_SOURCE_DELTA",
    });
    expect(
      decideReviewedCompletion({ ...progress, cleanStreak: 2 }, snapshot),
    ).toEqual({
      status: "denied",
      code: "TIBER_FINAL_REVIEW_STREAK_REQUIRED",
    });
  });

  it("requires three consecutive complete clean reviews", () => {
    const first = advanceFinalReview(none, clean);
    const second = advanceFinalReview(some(first), clean);
    const third = advanceFinalReview(some(second), clean);
    expect([first.cleanStreak, second.cleanStreak, third.cleanStreak]).toEqual([
      1, 2, 3,
    ]);
  });

  it("resets on findings, incomplete lenses, stale context, or evidence delta", () => {
    const previous = advanceFinalReview(
      some(advanceFinalReview(none, clean)),
      clean,
    );
    const candidates: FinalReviewIteration[] = [
      {
        ...clean,
        reviews: [
          behaviorReview,
          { ...securityReview, findingCount: finalReviewFindingCount(1) },
        ],
      },
      { ...clean, reviews: [behaviorReview] },
      {
        ...clean,
        reviews: [
          { ...behaviorReview, contextFreshness: "stale" },
          securityReview,
        ],
      },
    ];
    for (const candidate of candidates) {
      expect(advanceFinalReview(some(previous), candidate).cleanStreak).toBe(0);
    }
    const evidenceDeltas: FinalReviewIteration[] = [
      {
        ...clean,
        sourceSnapshotDigest: sourceSnapshotDigest(`sha256:${"3".repeat(64)}`),
      },
      {
        ...clean,
        verificationDiagnosticDigest: verificationDiagnosticDigest(
          `sha256:${"4".repeat(64)}`,
        ),
      },
      {
        ...clean,
        selectedLenses: ["behavior"],
        reviews: [behaviorReview],
      },
      {
        ...clean,
        selectedLenses: ["behavior", "architecture"],
        reviews: [behaviorReview, { ...securityReview, lens: "architecture" }],
      },
    ];
    for (const delta of evidenceDeltas) {
      expect(advanceFinalReview(some(previous), delta).cleanStreak).toBe(1);
    }
    expect(
      advanceFinalReview(some(previous), {
        ...clean,
        reviews: [securityReview, behaviorReview],
      }).cleanStreak,
    ).toBe(0);
    expect(
      advanceFinalReview(some({ ...previous, cleanStreak: 0 }), clean)
        .cleanStreak,
    ).toBe(1);
    expect(
      advanceFinalReview(
        some({
          ...previous,
          selectedLenses: ["behavior"],
          cleanStreak: 2,
        }),
        clean,
      ).cleanStreak,
    ).toBe(1);
  });
});
