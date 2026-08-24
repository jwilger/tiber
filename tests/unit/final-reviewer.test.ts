import { describe, expect, it } from "vitest";

import { parseFinalReviewOutput } from "../../src/adapters/models/pi-final-reviewer.js";

describe("Pi final review boundary", () => {
  it("accepts only the exact bounded response shape", () => {
    expect(
      parseFinalReviewOutput(
        JSON.stringify({
          findingCount: 0,
          rationale: "The complete behavior is correct and tested.",
        }),
        "behavior",
      ),
    ).toMatchObject({
      ok: true,
      value: {
        lens: "behavior",
        contextFreshness: "fresh",
        findingCount: 0,
      },
    });
  });

  it.each([
    "not json",
    "null",
    "[]",
    "{}",
    '{"findingCount":0,"rationale":"too short"}',
    '{"findingCount":-1,"rationale":"A sufficiently long rationale."}',
    '{"findingCount":0,"rationale":"A sufficiently long rationale.","extra":true}',
  ])("rejects malformed output %s", (output) => {
    expect(parseFinalReviewOutput(output, "security")).toMatchObject({
      ok: false,
      failure: { code: "TIBER_FINAL_REVIEW_INVALID" },
    });
  });
});
