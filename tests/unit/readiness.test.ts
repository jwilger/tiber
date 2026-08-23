import { describe, expect, it } from "vitest";

import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
  type TaskSpecification,
} from "../../src/core/tasks/readiness.js";

const baseScenario = {
  name: "clean review",
  given: ["a complete canonical specification"],
  when: ["a fresh reviewer finds no issues"],
  then: ["the task enters Ready"],
} as const;

const specification = {
  outcome: "A shared task can enter Ready only after independent review",
  scenarios: [baseScenario],
  acceptanceCriteria: ["Ready is shared"],
  exclusions: ["No automatic priority changes"],
  dependencies: [],
  testMappings: ["tests/acceptance/readiness.test.ts"],
  architectureImplications:
    "The review is advisory input to deterministic authority.",
} as const satisfies TaskSpecification;

const review: ReadinessReview = {
  freshContext: true,
  reviewerRole: "specification-reviewer",
  findingCount: 0,
  reviewedSpecificationDigest: "sha256:spec",
};

describe("specification parsing", () => {
  it("computes a byte-stable canonical digest", () => {
    expect(digestTaskSpecification(specification)).toBe(
      "sha256:6643ae23a72c68821650a8b0e91f4296b17fe86741bb6aa34cac4186085ff558",
    );
  });

  it("parses a complete structured specification", () => {
    expect(parseTaskSpecification(specification)).toEqual(specification);
  });

  it.each([
    null,
    [],
    "specification",
    {},
    { ...specification, outcome: 1 },
    { ...specification, scenarios: null },
    { ...specification, acceptanceCriteria: null },
    { ...specification, acceptanceCriteria: [1] },
    { ...specification, exclusions: null },
    { ...specification, exclusions: [1] },
    { ...specification, dependencies: null },
    { ...specification, dependencies: [1] },
    { ...specification, testMappings: null },
    { ...specification, testMappings: [1] },
    { ...specification, architectureImplications: 1 },
    { ...specification, scenarios: [null] },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], name: 1 }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], given: null }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], given: [1] }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], when: null }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], when: [1] }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], then: null }],
    },
    {
      ...specification,
      scenarios: [{ ...specification.scenarios[0], then: [1] }],
    },
  ])("rejects malformed specification %j", (candidate) => {
    expect(parseTaskSpecification(candidate)).toBeUndefined();
  });
});

describe("specification readiness", () => {
  it("accepts a complete specification with a clean fresh exact review", () => {
    expect(decideReadiness(specification, "sha256:spec", review)).toEqual({
      ready: true,
      code: "TIBER_SPECIFICATION_READY",
      reasons: [],
    });
  });

  it.each([
    [{ ...specification, outcome: "" }, review, "outcome is missing"],
    [{ ...specification, outcome: "   " }, review, "outcome is missing"],
    [{ ...specification, scenarios: [] }, review, "scenarios are missing"],
    [
      {
        ...specification,
        scenarios: [{ ...specification.scenarios[0], name: "" }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      {
        ...specification,
        scenarios: [{ ...specification.scenarios[0], name: "   " }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      {
        ...specification,
        scenarios: [baseScenario, { ...baseScenario, then: [] }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      {
        ...specification,
        scenarios: [{ ...specification.scenarios[0], given: [] }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      {
        ...specification,
        scenarios: [{ ...specification.scenarios[0], when: [] }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      {
        ...specification,
        scenarios: [{ ...specification.scenarios[0], then: [] }],
      },
      review,
      "a scenario is structurally incomplete",
    ],
    [
      { ...specification, acceptanceCriteria: [] },
      review,
      "acceptance criteria are missing",
    ],
    [{ ...specification, exclusions: [] }, review, "exclusions are missing"],
    [
      { ...specification, testMappings: [] },
      review,
      "test mappings are missing",
    ],
    [
      { ...specification, architectureImplications: "" },
      review,
      "architecture implications are missing",
    ],
    [
      { ...specification, architectureImplications: "   " },
      review,
      "architecture implications are missing",
    ],
    [
      specification,
      { ...review, freshContext: false },
      "review did not use fresh context",
    ],
    [
      specification,
      { ...review, findingCount: 1 },
      "review has unresolved findings",
    ],
    [
      specification,
      { ...review, reviewedSpecificationDigest: "old" },
      "review is stale",
    ],
  ] as const)(
    "denies incomplete or adverse readiness",
    (candidate, candidateReview, reason) => {
      const decision = decideReadiness(
        candidate,
        "sha256:spec",
        candidateReview,
      );
      expect(decision.ready).toBe(false);
      expect(decision.code).toBe("TIBER_SPECIFICATION_NOT_READY");
      expect(decision.reasons).toContain(reason);
    },
  );

  it("rejects structurally incomplete Gherkin", () => {
    expect(
      decideReadiness(
        {
          ...specification,
          scenarios: [
            { name: "bad", given: [], when: ["when"], then: ["then"] },
          ],
        },
        "sha256:spec",
        review,
      ).reasons,
    ).toContain("a scenario is structurally incomplete");
  });
});
