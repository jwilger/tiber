import { describe, expect, it } from "vitest";

import { parseReadinessReviewOutput } from "../../src/adapters/models/pi-readiness-reviewer.js";

describe("isolated readiness reviewer output", () => {
  it("parses the fixed completion schema and binds the reviewed digest", () => {
    expect(
      parseReadinessReviewOutput('{"findingCount":0}', "sha256:spec"),
    ).toEqual({
      freshContext: true,
      reviewerRole: "specification-reviewer",
      findingCount: 0,
      reviewedSpecificationDigest: "sha256:spec",
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
    expect(parseReadinessReviewOutput(output, "sha256:spec")).toBeUndefined();
  });
});
