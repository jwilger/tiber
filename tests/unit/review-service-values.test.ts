import { describe, expect, it } from "vitest";

import {
  parseReviewAuthorLogin,
  parseReviewBaseRef,
  parseReviewBody,
  parseReviewHeadRef,
  parseReviewNodeId,
  parseReviewNumber,
  parseReviewRepositoryName,
  parseReviewRepositoryOwner,
  parseReviewRevision,
  parseReviewTitle,
  parseReviewUrl,
} from "../../src/core/reviews/review-service-values.js";

function accepts(
  parser: (input: unknown) => { readonly ok: boolean },
  input: unknown,
): void {
  expect(parser(input).ok).toBe(true);
}
function rejects(
  parser: (input: unknown) => { readonly ok: boolean },
  inputs: readonly unknown[],
): void {
  for (const input of inputs)
    expect(parser(input).ok, JSON.stringify(input)).toBe(false);
}

describe("review service semantic values", () => {
  it("parses exact revisions", () => {
    accepts(parseReviewRevision, "a".repeat(40));
    rejects(parseReviewRevision, [
      { toString: () => "a".repeat(40) },
      1,
      "a".repeat(39),
      "a".repeat(41),
      `g${"a".repeat(39)}`,
      `x${"a".repeat(40)}`,
    ]);
  });

  it("parses bounded Git head refs", () => {
    accepts(parseReviewHeadRef, "refs/heads/a");
    accepts(parseReviewHeadRef, `refs/heads/a${"b".repeat(254)}`);
    rejects(parseReviewHeadRef, [
      {
        toString: () => "refs/heads/a",
        includes: () => false,
        endsWith: () => false,
      },
      1,
      "main",
      "xrefs/heads/a",
      "refs/heads/",
      "refs/heads/a..b",
      "refs/heads/a.",
      "refs/heads/a/",
      `refs/heads/a${"b".repeat(255)}`,
    ]);
  });

  it("parses bounded base refs", () => {
    accepts(parseReviewBaseRef, "main");
    accepts(parseReviewBaseRef, `a${"b".repeat(254)}`);
    rejects(parseReviewBaseRef, [
      1,
      "",
      "/main",
      "a..b",
      "a.",
      "a/",
      `a${"b".repeat(255)}`,
    ]);
  });

  it("parses bounded Conventional Commit review titles", () => {
    accepts(parseReviewTitle, "feat: x");
    accepts(parseReviewTitle, "feat(review)!: breaking change");
    accepts(parseReviewTitle, `feat: ${"a".repeat(194)}`);
    rejects(parseReviewTitle, [
      1,
      "ab",
      " feat: x",
      "feat: x ",
      "Feature",
      "x feat: x",
      "feat: x\njunk",
      `feat: ${"a".repeat(195)}`,
    ]);
  });

  it("parses bounded nonempty review bodies", () => {
    accepts(parseReviewBody, "x");
    accepts(parseReviewBody, "a".repeat(20_000));
    rejects(parseReviewBody, [1, "", " x", "x ", "a".repeat(20_001)]);
  });

  it("parses repository owner and name identifiers", () => {
    for (const parser of [
      parseReviewRepositoryOwner,
      parseReviewRepositoryName,
    ]) {
      accepts(parser, "a");
      accepts(parser, `a${"b".repeat(98)}z`);
      rejects(parser, [1, "", "-a", "a-", "a/b", `a${"b".repeat(99)}z`]);
    }
  });

  it("parses positive safe review numbers", () => {
    accepts(parseReviewNumber, 1);
    accepts(parseReviewNumber, Number.MAX_SAFE_INTEGER);
    rejects(parseReviewNumber, ["1", 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]);
  });

  it("parses bounded node IDs and author logins", () => {
    accepts(parseReviewNodeId, "x");
    accepts(parseReviewNodeId, "a".repeat(512));
    rejects(parseReviewNodeId, [
      { length: 1, includes: () => false },
      1,
      "",
      "a".repeat(513),
      "a\0b",
    ]);
    accepts(parseReviewAuthorLogin, "x");
    accepts(parseReviewAuthorLogin, "a".repeat(100));
    rejects(parseReviewAuthorLogin, [
      { length: 1, includes: () => false },
      1,
      "",
      "a".repeat(101),
      "a\0b",
    ]);
  });

  it("parses credential-free bounded HTTPS URLs", () => {
    accepts(parseReviewUrl, "https://github.com/o/r/pull/1");
    const prefix = "https://example.test/";
    accepts(parseReviewUrl, `${prefix}${"a".repeat(2_048 - prefix.length)}`);
    rejects(parseReviewUrl, [
      { length: 1, toString: () => "https://example.test/" },
      1,
      "not-url",
      "http://github.com/o/r",
      "https://u:p@github.com/o/r",
      "https://u@github.com/o/r",
      "https://:p@github.com/o/r",
      `https://example.test/${"a".repeat(2_100)}`,
    ]);
  });

  it("returns a complete stable failure", () => {
    expect(parseReviewRevision("bad")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_REVIEW_VALUE_INVALID",
        message: "Invalid review",
        safeContext: { field: "review" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["review revision is invalid"],
        redaction: "public",
      },
    });
    for (const invalidUrl of [
      "http://example.test/path",
      `https://example.test/${"a".repeat(2_100)}`,
    ]) {
      const result = parseReviewUrl(invalidUrl);
      expect(result.ok ? "" : result.failure.requiredRecoveryEvidence[0]).toBe(
        "review URL is invalid",
      );
    }
    const failures = [
      parseReviewHeadRef("bad"),
      parseReviewBaseRef("bad..ref"),
      parseReviewTitle("bad"),
      parseReviewBody(""),
      parseReviewRepositoryOwner(""),
      parseReviewRepositoryName(""),
      parseReviewNumber(0),
      parseReviewNodeId(""),
      parseReviewUrl("bad"),
      parseReviewAuthorLogin(""),
    ];
    expect(
      failures.map((result) =>
        result.ok ? "" : result.failure.requiredRecoveryEvidence[0],
      ),
    ).toEqual([
      "review head ref is invalid",
      "review base ref is invalid",
      "review title is invalid",
      "review body is invalid",
      "review repository owner is invalid",
      "review repository name is invalid",
      "review number is invalid",
      "review node id is invalid",
      "review URL is invalid",
      "review author login is invalid",
    ]);
  });
});
