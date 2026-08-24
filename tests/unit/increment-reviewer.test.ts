import { describe, expect, it } from "vitest";

import { parseIncrementReviewOutput } from "../../src/adapters/models/pi-increment-reviewer.js";
import { scenarioName } from "../fixtures/task-values.js";
import { sourceDiffDigest } from "../fixtures/workflow-values.js";

const digest = sourceDiffDigest(`sha256:${"a".repeat(64)}`);
const scenario = scenarioName("scenario");

describe("fresh lightweight increment review", () => {
  it("parses strict review findings", () => {
    expect(
      parseIncrementReviewOutput(
        JSON.stringify({
          findingCount: 0,
          overimplementation: false,
          rationale: "The change is minimal and scenario-focused.",
        }),
        scenario,
        digest,
      ),
    ).toEqual({
      ok: true,
      value: {
        contextFreshness: "fresh",
        reviewerRole: "lightweight-increment-reviewer",
        reviewedScenarioName: "scenario",
        reviewedSourceDiffDigest: digest,
        findingCount: 0,
        overimplementation: false,
        rationale: "The change is minimal and scenario-focused.",
      },
    });
  });

  it.each([
    "bad",
    "{}",
    '{"findingCount":-1,"overimplementation":false,"rationale":"A sufficiently long rationale."}',
    '{"findingCount":0,"overimplementation":"no","rationale":"A sufficiently long rationale."}',
    '{"findingCount":0,"overimplementation":false,"rationale":"short"}',
    '{"findingCount":0,"overimplementation":false,"rationale":"A sufficiently long rationale.","extra":true}',
  ])("rejects malformed review %j", (text) => {
    expect(parseIncrementReviewOutput(text, scenario, digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_INCREMENT_REVIEW_INVALID" },
    });
  });
});
