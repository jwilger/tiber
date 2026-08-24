import { describe, expect, it } from "vitest";

import { parseReadinessReviewOutput } from "../../src/adapters/models/pi-readiness-reviewer.js";
import { parseSpecificationDigest } from "../../src/core/tasks/task-values.js";

const parsedDigest = parseSpecificationDigest(`sha256:${"a".repeat(64)}`);
if (!parsedDigest.ok) throw new Error("invalid digest fixture");
const digest = parsedDigest.value;

describe("isolated readiness reviewer output", () => {
  it("parses the fixed completion schema and binds the reviewed digest", () => {
    expect(parseReadinessReviewOutput('{"findingCount":0}', digest)).toEqual({
      ok: true,
      value: {
        contextFreshness: "fresh",
        reviewerRole: "specification-reviewer",
        findingCount: 0,
        reviewedSpecificationDigest: digest,
      },
    });
  });

  it.each([
    "not json",
    "null",
    "[]",
    "{}",
    '{"findingCount":"0"}',
    '{"findingCount":-1}',
    '{"findingCount":1.5}',
    '{"findingCount":0,"extra":true}',
  ])("rejects malformed completion %j", (output) => {
    expect(parseReadinessReviewOutput(output, digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_READINESS_REVIEW_INVALID" },
    });
  });
});
