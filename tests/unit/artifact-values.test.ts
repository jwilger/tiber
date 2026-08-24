import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseArtifactByteLength,
  parseArtifactContent,
  parseArtifactDigest,
  parseArtifactLineNumber,
  parseArtifactOmittedBytes,
  parseArtifactPreviewHead,
  parseArtifactPreviewTail,
  parseArtifactRangeLimit,
  parseArtifactRangeOffset,
  parseArtifactRangeText,
  parseArtifactReapAtMilliseconds,
  parseArtifactSearchMaximumMatches,
  parseArtifactSearchMatchText,
  parseArtifactSearchQuery,
  parseArtifactsReapedCount,
  parseCommandDurationMilliseconds,
  parseCommandExitCode,
  parseCommandStandardError,
  parseCommandStandardOutput,
  parseInlineOutputMaximumBytes,
  type ArtifactDigest,
  type ArtifactRangeLimit,
  type ArtifactRangeOffset,
  type ArtifactReapAtMilliseconds,
  type ArtifactsReapedCount,
} from "../../src/core/artifacts/artifact-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("artifact semantic values", () => {
  it("keeps digest, range, lifecycle, and count purposes distinct", () => {
    expectTypeOf<ArtifactRangeOffset>().not.toEqualTypeOf<ArtifactRangeLimit>();
    expectTypeOf<ArtifactReapAtMilliseconds>().not.toEqualTypeOf<ArtifactsReapedCount>();
    expectTypeOf<ArtifactDigest>().not.toEqualTypeOf<ArtifactRangeOffset>();
  });

  it("parses bounded artifact operations", () => {
    expect(parseArtifactDigest(`sha256:${"a".repeat(64)}`).ok).toBe(true);
    expect(parseArtifactRangeOffset(0).ok).toBe(true);
    expect(parseArtifactRangeLimit(65_536).ok).toBe(true);
    expect(parseArtifactSearchMaximumMatches(100).ok).toBe(true);
    expect(parseArtifactSearchQuery("needle").ok).toBe(true);
    expect(parseArtifactReapAtMilliseconds(0).ok).toBe(true);
    expect(parseArtifactsReapedCount(0).ok).toBe(true);
  });

  it("rejects coercible and out-of-bound artifact values", () => {
    expect(parseArtifactRangeOffset("0").ok).toBe(false);
    expect(parseArtifactRangeLimit(65_537).ok).toBe(false);
    expect(parseArtifactSearchQuery({ length: 1 }).ok).toBe(false);
    expect(parseArtifactSearchQuery("x").ok).toBe(true);
    expect(parseArtifactSearchQuery("x".repeat(256)).ok).toBe(true);
    expect(parseArtifactSearchQuery("x".repeat(257)).ok).toBe(false);
    expect(
      parseArtifactDigest({ toString: () => `sha256:${"a".repeat(64)}` }).ok,
    ).toBe(false);
    expect(parseArtifactDigest(`xsha256:${"a".repeat(64)}`).ok).toBe(false);
    expect(parseArtifactDigest(`sha256:${"a".repeat(64)}x`).ok).toBe(false);
  });

  it.each([
    [parseCommandStandardOutput, 1, "commandStandardOutput"],
    [parseCommandStandardError, 1, "commandStandardError"],
    [parseCommandExitCode, 1.5, "commandExitCode"],
    [parseCommandDurationMilliseconds, -1, "commandDurationMilliseconds"],
    [parseInlineOutputMaximumBytes, 0, "inlineOutputMaximumBytes"],
    [parseArtifactDigest, "sha256:bad", "artifactDigest"],
    [parseArtifactContent, 1, "artifactContent"],
    [parseArtifactPreviewHead, 1, "artifactPreviewHead"],
    [parseArtifactPreviewTail, 1, "artifactPreviewTail"],
    [parseArtifactByteLength, -1, "artifactByteLength"],
    [parseArtifactOmittedBytes, -1, "artifactOmittedBytes"],
    [parseArtifactRangeOffset, -1, "artifactRangeOffset"],
    [parseArtifactRangeLimit, 0, "artifactRangeLimit"],
    [parseArtifactRangeText, 1, "artifactRangeText"],
    [parseArtifactSearchMaximumMatches, 101, "artifactSearchMaximumMatches"],
    [parseArtifactSearchQuery, "", "artifactSearchQuery"],
    [parseArtifactSearchMatchText, 1, "artifactSearchMatchText"],
    [parseArtifactLineNumber, 0, "artifactLineNumber"],
    [parseArtifactReapAtMilliseconds, -1, "artifactReapAtMilliseconds"],
    [parseArtifactsReapedCount, -1, "artifactsReapedCount"],
  ])("rejects malformed artifact values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_ARTIFACT_VALUE_INVALID", field),
    });
  });
});
