import { describe, expect, it } from "vitest";

import { parseRedReviewOutput } from "../../src/adapters/models/pi-red-reviewer.js";
import { redDiagnosticDigest } from "../fixtures/workflow-values.js";

const digest = redDiagnosticDigest(`sha256:${"a".repeat(64)}`);

describe("fresh RED classifier output", () => {
  it("parses one strict closed semantic classification", () => {
    expect(
      parseRedReviewOutput(
        JSON.stringify({
          classification: "valid-red",
          missingPublicSurface: true,
          rationale:
            "The mapped compile failure names the missing account deletion API.",
        }),
        digest,
      ),
    ).toEqual({
      ok: true,
      value: {
        contextFreshness: "fresh",
        reviewerRole: "red-classifier",
        reviewedDiagnosticDigest: digest,
        classification: "valid-red",
        missingPublicSurface: true,
        rationale:
          "The mapped compile failure names the missing account deletion API.",
      },
    });
  });

  it.each([
    "not json",
    "{}",
    '{"classification":"valid-red","missingPublicSurface":false,"rationale":"short"}',
    '{"classification":"invented","missingPublicSurface":false,"rationale":"A sufficiently long rationale."}',
    '{"classification":"valid-red","missingPublicSurface":"yes","rationale":"A sufficiently long rationale."}',
    '{"classification":"valid-red","missingPublicSurface":false,"rationale":"A sufficiently long rationale.","extra":true}',
  ])("rejects malformed classifier output %j", (text) => {
    expect(parseRedReviewOutput(text, digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_RED_REVIEW_INVALID" },
    });
  });
});
