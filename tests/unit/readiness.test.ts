import { describe, expect, it } from "vitest";

import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
} from "../../src/core/tasks/readiness.js";
import {
  parseSpecificationDigest,
  parseSpecificationReviewFindingCount,
} from "../../src/core/tasks/task-values.js";
import { expectedSpecificationParseFailure } from "../fixtures/failures.js";
import {
  requireTaskSpecification,
  validTaskSpecificationDocument,
} from "../fixtures/task-specification.js";

const specification = requireTaskSpecification(validTaskSpecificationDocument);
const digest = digestTaskSpecification(specification);

function findingCount(value: number) {
  const parsed = parseSpecificationReviewFindingCount(value);
  if (!parsed.ok) throw new Error("invalid finding count fixture");
  return parsed.value;
}

function specificationDigest(value: string) {
  const parsed = parseSpecificationDigest(value);
  if (!parsed.ok) throw new Error("invalid digest fixture");
  return parsed.value;
}

const review: ReadinessReview = {
  contextFreshness: "fresh",
  reviewerRole: "specification-reviewer",
  findingCount: findingCount(0),
  reviewedSpecificationDigest: digest,
};

describe("specification parsing", () => {
  it("computes a byte-stable canonical digest", () => {
    expect(digest).toBe(
      "sha256:6643ae23a72c68821650a8b0e91f4296b17fe86741bb6aa34cac4186085ff558",
    );
  });

  it("parses a complete structured specification", () => {
    expect(parseTaskSpecification(validTaskSpecificationDocument)).toEqual({
      ok: true,
      value: validTaskSpecificationDocument,
    });
  });

  it("rejects each malformed scenario component independently", () => {
    for (const scenario of [
      null,
      { ...validTaskSpecificationDocument.scenarios[0], given: "not-an-array" },
      { ...validTaskSpecificationDocument.scenarios[0], when: "not-an-array" },
      { ...validTaskSpecificationDocument.scenarios[0], then: "not-an-array" },
    ]) {
      expect(
        parseTaskSpecification({
          ...validTaskSpecificationDocument,
          scenarios: [scenario],
        }),
      ).toEqual({
        ok: false,
        failure: expectedSpecificationParseFailure(),
      });
    }
  });

  it.each([
    null,
    [],
    "specification",
    {},
    { ...validTaskSpecificationDocument, outcome: 1 },
    { ...validTaskSpecificationDocument, outcome: "" },
    { ...validTaskSpecificationDocument, scenarios: null },
    { ...validTaskSpecificationDocument, scenarios: [] },
    { ...validTaskSpecificationDocument, acceptanceCriteria: null },
    { ...validTaskSpecificationDocument, acceptanceCriteria: [1] },
    { ...validTaskSpecificationDocument, acceptanceCriteria: [] },
    { ...validTaskSpecificationDocument, exclusions: null },
    { ...validTaskSpecificationDocument, exclusions: [1] },
    { ...validTaskSpecificationDocument, exclusions: [] },
    { ...validTaskSpecificationDocument, dependencies: null },
    { ...validTaskSpecificationDocument, dependencies: [1] },
    { ...validTaskSpecificationDocument, testMappings: null },
    { ...validTaskSpecificationDocument, testMappings: [1] },
    { ...validTaskSpecificationDocument, testMappings: [] },
    { ...validTaskSpecificationDocument, architectureImplications: null },
    { ...validTaskSpecificationDocument, architectureImplications: "" },
    {
      ...validTaskSpecificationDocument,
      scenarios: [
        { name: "", given: ["given"], when: ["when"], then: ["then"] },
      ],
    },
    {
      ...validTaskSpecificationDocument,
      scenarios: [
        { name: "scenario", given: [], when: ["when"], then: ["then"] },
      ],
    },
    {
      ...validTaskSpecificationDocument,
      scenarios: [
        { name: "scenario", given: ["given"], when: [], then: ["then"] },
      ],
    },
    {
      ...validTaskSpecificationDocument,
      scenarios: [
        { name: "scenario", given: ["given"], when: ["when"], then: [] },
      ],
    },
  ])("rejects malformed or incomplete specification", (candidate) => {
    expect(parseTaskSpecification(candidate)).toEqual({
      ok: false,
      failure: expectedSpecificationParseFailure(),
    });
  });
});

describe("specification readiness", () => {
  it("accepts a complete specification with a clean fresh exact review", () => {
    expect(decideReadiness(digest, review)).toEqual({
      status: "ready",
      code: "TIBER_SPECIFICATION_READY",
      reasons: [],
    });
  });

  it.each([
    [
      { ...review, contextFreshness: "stale" },
      "review did not use fresh context",
    ],
    [
      { ...review, findingCount: findingCount(1) },
      "review has unresolved findings",
    ],
    [
      {
        ...review,
        reviewedSpecificationDigest: specificationDigest(
          `sha256:${"f".repeat(64)}`,
        ),
      },
      "review is stale",
    ],
  ] as const)("denies an adverse review", (candidateReview, reason) => {
    const decision = decideReadiness(digest, candidateReview);
    expect(decision).toEqual({
      status: "not-ready",
      code: "TIBER_SPECIFICATION_NOT_READY",
      reasons: [reason],
    });
  });
});
