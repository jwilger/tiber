import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  advanceCacheEpoch,
  decideHeadroom,
  parseCacheEpochTransition,
  parseContextBudget,
  parseContextSegment,
  planContext,
} from "../../src/core/context/headroom.js";

const parsed = <T>(result: { ok: true; value: T } | { ok: false }): T => {
  if (!result.ok) throw new Error("invalid fixture");
  return result.value;
};
const budget = parsed(
  parseContextBudget({
    contextWindowTokens: 100,
    reserveTokens: 20,
    hardInputTokens: 70,
    segmentByteLimit: 200,
  }),
);
const segment = (
  id: string,
  priority: "authority" | "verification" | "goal" | "working" | "optional",
  content: string,
) =>
  parsed(
    parseContextSegment({
      id,
      priority,
      content,
      provenance: `artifact:${id}`,
    }),
  );
const stable = {
  prompt: "immutable prompt",
  initialContext: ["architecture"],
  toolSchemas: ["read:v1", "write:v1"],
};

function invalidFailure(field: string) {
  return {
    ok: false,
    failure: {
      code: "TIBER_CONTEXT_VALUE_INVALID",
      message: "Invalid context planning values",
      safeContext: { field: "context planning values" },
      causes: [],
      retryability: "retry-after-input",
      requiredRecoveryEvidence: [field],
      redaction: "public",
    },
  };
}

describe("headroom and cache epochs", () => {
  it("parses complete budgets and rejects every malformed or contradictory bound", () => {
    expect(
      parseContextBudget({
        contextWindowTokens: 100,
        reserveTokens: 20,
        hardInputTokens: 70,
        segmentByteLimit: 200,
      }),
    ).toMatchObject({ ok: true });
    for (const value of [
      null,
      [],
      "budget",
      {},
      {
        contextWindowTokens: 0,
        reserveTokens: 0,
        hardInputTokens: 1,
        segmentByteLimit: 1,
      },
      {
        contextWindowTokens: 1.5,
        reserveTokens: 0,
        hardInputTokens: 1,
        segmentByteLimit: 1,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: -1,
        hardInputTokens: 70,
        segmentByteLimit: 200,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: 1.5,
        hardInputTokens: 70,
        segmentByteLimit: 200,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: 100,
        hardInputTokens: 1,
        segmentByteLimit: 200,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: 20,
        hardInputTokens: 0,
        segmentByteLimit: 200,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: 20,
        hardInputTokens: 81,
        segmentByteLimit: 200,
      },
      {
        contextWindowTokens: 100,
        reserveTokens: 20,
        hardInputTokens: 70,
        segmentByteLimit: 0,
      },
    ])
      expect(parseContextBudget(value)).toEqual(
        invalidFailure("context budget"),
      );
  });

  it("parses each typed priority and rejects malformed segments", () => {
    for (const priority of [
      "authority",
      "verification",
      "goal",
      "working",
      "optional",
    ] as const)
      expect(
        parseContextSegment({
          id: priority,
          priority,
          content: "content",
          provenance: "artifact:digest",
        }),
      ).toMatchObject({ ok: true, value: { priority } });
    for (const value of [
      null,
      [],
      "segment",
      {},
      { id: "", priority: "goal", content: "c", provenance: "p" },
      { id: { length: 1 }, priority: "goal", content: "c", provenance: "p" },
      { id: "id", priority: "unknown", content: "c", provenance: "p" },
      { id: "id", priority: "goal", content: "", provenance: "p" },
      { id: "id", priority: "goal", content: "c", provenance: "" },
    ])
      expect(parseContextSegment(value)).toEqual(
        invalidFailure("context segment"),
      );
  });

  it("keeps the exact cache prefix byte-stable and appends changing state only as a suffix", () => {
    const first = planContext({
      budget,
      stable,
      dynamic: [segment("authority", "authority", "signed state A")],
    });
    const second = planContext({
      budget,
      stable,
      dynamic: [segment("authority", "authority", "signed state B")],
    });
    expect(first.ok && second.ok).toBe(true);
    if (!first.ok || !second.ok) return;
    const expectedPrefix =
      "<tiber-cache-prefix-v1>\nprompt:immutable prompt\ncontext:architecture\ntool:read:v1\ntool:write:v1\n</tiber-cache-prefix-v1>\n";
    expect(first.value.stablePrefix).toBe(expectedPrefix);
    expect(first.value.stablePrefix).toBe(second.value.stablePrefix);
    expect(first.value.epochId).toBe(
      createHash("sha256").update(expectedPrefix).digest("hex"),
    );
    expect(first.value.dynamicSuffix).toBe(
      '<tiber-context priority="authority" id="authority" provenance="artifact:authority">\nsigned state A\n</tiber-context>\n',
    );
    expect(first.value.context).toBe(
      `${expectedPrefix}${first.value.dynamicSuffix}`,
    );
    expect(second.value.dynamicSuffix).not.toBe(first.value.dynamicSuffix);
    expect(first.value.includedSegmentIds).toEqual(["authority"]);
    expect(first.value.omittedSegmentIds).toEqual([]);
  });

  it("orders priorities deterministically while preserving order within one priority", () => {
    const roomy = parsed(
      parseContextBudget({
        contextWindowTokens: 1000,
        reserveTokens: 100,
        hardInputTokens: 800,
        segmentByteLimit: 500,
      }),
    );
    const result = planContext({
      budget: roomy,
      stable: { prompt: "p", initialContext: [], toolSchemas: [] },
      dynamic: [
        segment("optional", "optional", "o"),
        segment("work-1", "working", "w1"),
        segment("authority", "authority", "a"),
        segment("goal", "goal", "g"),
        segment("verification", "verification", "v"),
        segment("work-2", "working", "w2"),
      ],
    });
    expect(result).toMatchObject({
      ok: true,
      value: {
        includedSegmentIds: [
          "authority",
          "verification",
          "goal",
          "work-1",
          "work-2",
          "optional",
        ],
        omittedSegmentIds: [],
      },
    });
    if (result.ok) {
      expect(result.value.dynamicSuffix.indexOf('id="work-1"')).toBeLessThan(
        result.value.dynamicSuffix.indexOf('id="work-2"'),
      );
      expect(result.value.dynamicSuffix).not.toContain("Stryker was here");
    }
  });

  it("drops only lower priorities and blocks before authority or verification can be weakened", () => {
    const roomy = parsed(
      parseContextBudget({
        contextWindowTokens: 200,
        reserveTokens: 20,
        hardInputTokens: 150,
        segmentByteLimit: 200,
      }),
    );
    const planned = planContext({
      budget: roomy,
      stable: { prompt: "p", initialContext: [], toolSchemas: [] },
      dynamic: [
        segment("authority", "authority", "A"),
        segment("verification", "verification", "V"),
        segment("goal", "goal", "G".repeat(500)),
        segment("working", "working", "W".repeat(500)),
        segment("optional", "optional", "O".repeat(500)),
      ],
    });
    expect(planned).toMatchObject({
      ok: true,
      value: {
        includedSegmentIds: ["authority", "verification"],
        omittedSegmentIds: ["goal", "working", "optional"],
      },
    });
    for (const priority of ["authority", "verification"] as const)
      expect(
        planContext({
          budget: roomy,
          stable: { prompt: "p", initialContext: [], toolSchemas: [] },
          dynamic: [segment(priority, priority, "X".repeat(201))],
        }),
      ).toMatchObject({
        ok: false,
        failure: {
          code: "TIBER_CONTEXT_BUDGET_EXHAUSTED",
          message: "Mandatory context exceeds the hard input budget",
          safeContext: { domain: "context-headroom" },
          causes: [],
          retryability: "retry-after-state-change",
          requiredRecoveryEvidence: [
            "smaller-authoritative-context-or-larger-budget",
          ],
          redaction: "public",
        },
      });
  });

  it("honors exact byte and token boundaries", () => {
    const bytes = parsed(
      parseContextBudget({
        contextWindowTokens: 500,
        reserveTokens: 10,
        hardInputTokens: 400,
        segmentByteLimit: 3,
      }),
    );
    expect(
      planContext({
        budget: bytes,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("goal", "goal", "abc")],
      }),
    ).toMatchObject({ ok: true, value: { includedSegmentIds: ["goal"] } });
    expect(
      planContext({
        budget: bytes,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("goal", "goal", "abcd")],
      }),
    ).toMatchObject({ ok: true, value: { omittedSegmentIds: ["goal"] } });
    const seed = planContext({
      budget: bytes,
      stable: { prompt: "p", initialContext: [], toolSchemas: [] },
      dynamic: [segment("authority", "authority", "a")],
    });
    expect(seed.ok).toBe(true);
    if (!seed.ok) return;
    const exact = parsed(
      parseContextBudget({
        contextWindowTokens: seed.value.estimatedInputTokens + 10,
        reserveTokens: 10,
        hardInputTokens: seed.value.estimatedInputTokens,
        segmentByteLimit: 3,
      }),
    );
    expect(
      planContext({
        budget: exact,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("authority", "authority", "a")],
      }).ok,
    ).toBe(true);
    const tooSmall = parsed(
      parseContextBudget({
        contextWindowTokens: seed.value.estimatedInputTokens + 10,
        reserveTokens: 10,
        hardInputTokens: seed.value.estimatedInputTokens - 1,
        segmentByteLimit: 3,
      }),
    );
    expect(
      planContext({
        budget: tooSmall,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("authority", "authority", "a")],
      }).ok,
    ).toBe(false);
    expect(
      planContext({
        budget: tooSmall,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("verification", "verification", "a")],
      }).ok,
    ).toBe(false);
    const goalSeed = planContext({
      budget: bytes,
      stable: { prompt: "p", initialContext: [], toolSchemas: [] },
      dynamic: [segment("goal", "goal", "a")],
    });
    expect(goalSeed.ok).toBe(true);
    if (!goalSeed.ok) return;
    const goalTooSmall = parsed(
      parseContextBudget({
        contextWindowTokens: goalSeed.value.estimatedInputTokens + 10,
        reserveTokens: 10,
        hardInputTokens: goalSeed.value.estimatedInputTokens - 1,
        segmentByteLimit: 3,
      }),
    );
    expect(
      planContext({
        budget: goalTooSmall,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [segment("goal", "goal", "a")],
      }),
    ).toMatchObject({ ok: true, value: { omittedSegmentIds: ["goal"] } });
  });

  it("accepts a stable prefix at the exact token boundary", () => {
    const roomy = parsed(
      parseContextBudget({
        contextWindowTokens: 100,
        reserveTokens: 10,
        hardInputTokens: 90,
        segmentByteLimit: 10,
      }),
    );
    const seed = planContext({
      budget: roomy,
      stable: { prompt: "p", initialContext: [], toolSchemas: [] },
      dynamic: [],
    });
    expect(seed.ok).toBe(true);
    if (!seed.ok) return;
    const exact = parsed(
      parseContextBudget({
        contextWindowTokens: seed.value.estimatedInputTokens + 1,
        reserveTokens: 1,
        hardInputTokens: seed.value.estimatedInputTokens,
        segmentByteLimit: 10,
      }),
    );
    expect(
      planContext({
        budget: exact,
        stable: { prompt: "p", initialContext: [], toolSchemas: [] },
        dynamic: [],
      }).ok,
    ).toBe(true);
  });

  it("blocks when the stable prefix alone exceeds the hard budget", () => {
    const tiny = parsed(
      parseContextBudget({
        contextWindowTokens: 10,
        reserveTokens: 1,
        hardInputTokens: 1,
        segmentByteLimit: 10,
      }),
    );
    expect(
      planContext({
        budget: tiny,
        stable: { prompt: "large", initialContext: [], toolSchemas: [] },
        dynamic: [],
      }).ok,
    ).toBe(false);
  });

  it("reserves completion headroom for observed and estimated contexts", () => {
    expect(
      decideHeadroom({
        budget,
        plannedInputTokens: 70,
        observedContextTokens: 79,
      }),
    ).toEqual({ kind: "proceed", remainingTokens: 21 });
    expect(
      decideHeadroom({
        budget,
        plannedInputTokens: 70,
        observedContextTokens: 80,
      }),
    ).toEqual({ kind: "compact", reason: "reserve-bound" });
    expect(
      decideHeadroom({
        budget,
        plannedInputTokens: 70,
        observedContextTokens: null,
      }),
    ).toEqual({ kind: "proceed", remainingTokens: 30 });
    expect(
      decideHeadroom({
        budget,
        plannedInputTokens: 71,
        observedContextTokens: null,
      }),
    ).toEqual({ kind: "block", code: "TIBER_CONTEXT_BUDGET_EXHAUSTED" });
  });

  it("starts a deterministic provenance-bound epoch only through compaction", () => {
    const input = {
      previousEpochId: "a".repeat(64),
      sourceArtifactDigest: "b".repeat(64),
      summaryDigest: "c".repeat(64),
      firstKeptEntryId: "entry-1",
    };
    const transition = parsed(parseCacheEpochTransition(input));
    const epoch = advanceCacheEpoch(transition);
    expect(epoch).toEqual({
      ...input,
      epochId: createHash("sha256").update(JSON.stringify(input)).digest("hex"),
    });
    expect(advanceCacheEpoch(transition)).toEqual(epoch);
    const changed = parsed(
      parseCacheEpochTransition({ ...input, summaryDigest: "d".repeat(64) }),
    );
    expect(advanceCacheEpoch(changed).epochId).not.toBe(epoch.epochId);
  });
});
