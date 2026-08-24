import { describe, expect, it } from "vitest";
import {
  parseCacheEpochTransition,
  parseContextBudget,
  parseContextSegment,
} from "../../src/core/context/headroom.js";

const validBudget = {
  contextWindowTokens: 100,
  reserveTokens: 20,
  hardInputTokens: 70,
  segmentByteLimit: 200,
};
const validSegment = {
  id: "id",
  priority: "authority",
  content: "content",
  provenance: "artifact:digest",
};

describe("headroom semantic values", () => {
  it("parses the exact complete budget inside the test boundary", () => {
    expect(parseContextBudget(validBudget)).toEqual({
      ok: true,
      value: validBudget,
    });
    const zeroReserve = { ...validBudget, reserveTokens: 0 };
    expect(parseContextBudget(zeroReserve)).toEqual({
      ok: true,
      value: zeroReserve,
    });
  });

  it.each([
    null,
    [],
    "value",
    {},
    { ...validBudget, contextWindowTokens: 0 },
    { ...validBudget, contextWindowTokens: 1.5 },
    { ...validBudget, reserveTokens: -1 },
    { ...validBudget, reserveTokens: 1.5 },
    { ...validBudget, hardInputTokens: 0 },
    { ...validBudget, hardInputTokens: 1.5 },
    { ...validBudget, segmentByteLimit: 0 },
    { ...validBudget, segmentByteLimit: 1.5 },
    { ...validBudget, hardInputTokens: 81 },
    { reserveTokens: 20, hardInputTokens: 70, segmentByteLimit: 200 },
    { contextWindowTokens: 100, hardInputTokens: 70, segmentByteLimit: 200 },
    { contextWindowTokens: 100, reserveTokens: 20, segmentByteLimit: 200 },
    { contextWindowTokens: 100, reserveTokens: 20, hardInputTokens: 70 },
  ])("rejects malformed budget %#", (value) => {
    expect(parseContextBudget(value).ok).toBe(false);
  });

  it("parses the exact complete segment inside the test boundary", () => {
    expect(parseContextSegment(validSegment)).toEqual({
      ok: true,
      value: validSegment,
    });
  });

  it.each([
    null,
    [],
    "value",
    {},
    { ...validSegment, id: "" },
    { ...validSegment, priority: "unknown" },
    { ...validSegment, content: "" },
    { ...validSegment, provenance: "" },
    {
      priority: "authority",
      content: "content",
      provenance: "artifact:digest",
    },
    { id: "id", content: "content", provenance: "artifact:digest" },
    { id: "id", priority: "authority", provenance: "artifact:digest" },
    { id: "id", priority: "authority", content: "content" },
  ])("rejects malformed segment %#", (value) => {
    expect(parseContextSegment(value).ok).toBe(false);
  });

  it("parses only complete semantic cache epoch transitions", () => {
    const valid = {
      previousEpochId: "a".repeat(64),
      sourceArtifactDigest: "b".repeat(64),
      summaryDigest: "c".repeat(64),
      firstKeptEntryId: "entry",
    };
    expect(parseCacheEpochTransition(valid)).toMatchObject({
      ok: true,
      value: valid,
    });
    for (const value of [
      null,
      {},
      { ...valid, previousEpochId: "bad" },
      { ...valid, previousEpochId: `${"a".repeat(64)}suffix` },
      { ...valid, previousEpochId: `prefix${"a".repeat(64)}` },
      { ...valid, previousEpochId: { toString: () => "a".repeat(64) } },
      { ...valid, sourceArtifactDigest: "bad" },
      { ...valid, sourceArtifactDigest: `${"b".repeat(64)}suffix` },
      { ...valid, sourceArtifactDigest: `prefix${"b".repeat(64)}` },
      { ...valid, sourceArtifactDigest: { toString: () => "b".repeat(64) } },
      { ...valid, summaryDigest: "bad" },
      { ...valid, summaryDigest: `${"c".repeat(64)}suffix` },
      { ...valid, summaryDigest: `prefix${"c".repeat(64)}` },
      { ...valid, summaryDigest: { toString: () => "c".repeat(64) } },
      { ...valid, firstKeptEntryId: "" },
    ])
      expect(parseCacheEpochTransition(value).ok).toBe(false);
    const failure = parseCacheEpochTransition({});
    expect(failure).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT_VALUE_INVALID",
        message: "Invalid context planning values",
        safeContext: { field: "context planning values" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["cache epoch transition"],
        redaction: "public",
      },
    });
  });
});
