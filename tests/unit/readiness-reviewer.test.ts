import { describe, expect, it } from "vitest";

import {
  parseReadinessReviewOutput,
  reviewSpecification,
} from "../../src/adapters/models/pi-readiness-reviewer.js";
import { parseTaskSpecification } from "../../src/core/tasks/readiness.js";
import { parseSpecificationDigest } from "../../src/core/tasks/task-values.js";

const parsedDigest = parseSpecificationDigest(`sha256:${"a".repeat(64)}`);
if (!parsedDigest.ok) throw new Error("invalid digest fixture");
const digest = parsedDigest.value;
const parsedSpecification = parseTaskSpecification({
  outcome: "Keep the visible Pi session responsive during readiness review.",
  scenarios: [
    {
      name: "cancel review",
      given: ["A readiness review is pending"],
      when: ["The outer tool request is cancelled"],
      then: ["The nested review is cancelled without provider dispatch"],
    },
  ],
  acceptanceCriteria: ["Cancellation is observed"],
  exclusions: ["Live provider testing"],
  dependencies: ["Pi SDK cancellation"],
  testMappings: ["cancel review maps to this deterministic adapter test"],
  architectureImplications: "Cancellation remains in the imperative shell.",
});
if (!parsedSpecification.ok) throw new Error("invalid specification fixture");
const specification = parsedSpecification.value;

describe("isolated readiness reviewer output", () => {
  it("parses clean completion and binds the reviewed digest", () => {
    expect(
      parseReadinessReviewOutput('{"findingCount":0,"findings":[]}', digest),
    ).toEqual({
      ok: true,
      value: {
        review: {
          contextFreshness: "fresh",
          reviewerRole: "specification-reviewer",
          findingCount: 0,
          reviewedSpecificationDigest: digest,
        },
        findings: [],
      },
    });
  });

  it("retains bounded actionable findings for recovery feedback", () => {
    expect(
      parseReadinessReviewOutput(
        JSON.stringify({
          findingCount: 2,
          findings: [
            "Name the expected behavior when the task is already Ready.",
            "Map the cancellation scenario to a public-boundary test.",
          ],
        }),
        digest,
      ),
    ).toMatchObject({
      ok: true,
      value: {
        review: { findingCount: 2 },
        findings: [
          "Name the expected behavior when the task is already Ready.",
          "Map the cancellation scenario to a public-boundary test.",
        ],
      },
    });
  });

  it("honors cancellation before creating a nested model session", async () => {
    const controller = new AbortController();
    controller.abort();

    await expect(
      reviewSpecification("/unused", specification, digest, {
        signal: controller.signal,
      }),
    ).resolves.toMatchObject({
      ok: false,
      failure: { code: "TIBER_REVIEW_CANCELLED" },
    });
  });

  it.each([
    "not json",
    "null",
    "[]",
    "{}",
    '{"findingCount":0}',
    '{"findingCount":"0","findings":[]}',
    '{"findingCount":-1}',
    '{"findingCount":1.5}',
    '{"findingCount":0,"findings":[],"extra":true}',
    '{"findingCount":1,"findings":[]}',
    '{"findingCount":0,"findings":["unexpected"]}',
    '{"findingCount":1,"findings":[""]}',
    JSON.stringify({ findingCount: 1, findings: ["line one\nline two"] }),
    JSON.stringify({ findingCount: 1, findings: ["unsafe\u001b[31m"] }),
    JSON.stringify({ findingCount: 1, findings: ["line\u2028separator"] }),
    JSON.stringify({ findingCount: 1, findings: ["spoof\u202etext"] }),
    JSON.stringify({ findingCount: 1, findings: ["x".repeat(501)] }),
    JSON.stringify({
      findingCount: 6,
      findings: Array.from(
        { length: 6 },
        (_, index) => `Blocking finding ${String(index + 1)}`,
      ),
    }),
  ])("rejects malformed completion %j", (output) => {
    expect(parseReadinessReviewOutput(output, digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_READINESS_REVIEW_INVALID" },
    });
  });
});
