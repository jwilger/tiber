import { describe, expect, it } from "vitest";

import {
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignCheckpointTime,
  parseCampaignConsumption,
  parseCampaignGoal,
  parseCampaignId,
  parseCampaignTaskId,
} from "../../src/core/campaigns/campaign.js";

const validBounds = {
  taskLimit: 2,
  initiativeTaskLimit: 1,
  durationLimitMs: 60_000,
  costLimitMicros: 1_000_000,
  tokenLimit: 10_000,
  concurrencyLimit: 2,
};
const validConsumption = {
  startedTasks: 0,
  elapsedMs: 0,
  costMicros: 0,
  tokens: 0,
  activeTasks: 0,
  startedTaskIds: [],
  activeTaskIds: [],
  initiativeStarts: { initiative: 0 },
};
const validCandidate = {
  taskId: "task-1",
  initiativeId: "initiative-1",
  rank: 0,
  blockerPhase: "none",
  estimatedCostMicros: 0,
  estimatedTokens: 0,
};

function expectInvalid(result: { readonly ok: boolean }): void {
  expect(result).toMatchObject({
    ok: false,
    failure: {
      code: "TIBER_CAMPAIGN_VALUE_INVALID",
      safeContext: { field: "campaign" },
      requiredRecoveryEvidence: ["corrected-input"],
    },
  });
}

describe("campaign semantic values", () => {
  it("parses every positive campaign bound", () => {
    expect(parseCampaignBounds(validBounds)).toEqual({
      ok: true,
      value: validBounds,
    });
  });

  it.each([null, [], "bounds"])("rejects non-record bounds %j", (value) => {
    expectInvalid(parseCampaignBounds(value));
  });

  for (const field of Object.keys(
    validBounds,
  ) as (keyof typeof validBounds)[]) {
    it.each([0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, "1"])(
      `rejects invalid ${field} %j`,
      (value) => {
        expectInvalid(parseCampaignBounds({ ...validBounds, [field]: value }));
      },
    );
  }

  it("parses non-negative consumption and initiative counts", () => {
    expect(parseCampaignConsumption(validConsumption)).toEqual({
      ok: true,
      value: validConsumption,
    });
  });

  it.each([null, [], "consumption"])(
    "rejects non-record consumption %j",
    (value) => {
      expectInvalid(parseCampaignConsumption(value));
    },
  );

  for (const field of [
    "startedTasks",
    "elapsedMs",
    "costMicros",
    "tokens",
    "activeTasks",
  ] as const) {
    it.each([-1, 0.5, Number.MAX_SAFE_INTEGER + 1, "0"])(
      `rejects invalid consumption ${field} %j`,
      (value) => {
        expectInvalid(
          parseCampaignConsumption({
            ...validConsumption,
            [field]: value,
          }),
        );
      },
    );
  }

  it.each([
    [],
    null,
    { " bad": 0 },
    { initiative: -1 },
    { initiative: 0.5 },
    { initiative: "0" },
  ])("rejects invalid initiative consumption %j", (initiativeStarts) => {
    expectInvalid(
      parseCampaignConsumption({ ...validConsumption, initiativeStarts }),
    );
  });

  it.each([
    { startedTasks: 1, startedTaskIds: [] },
    { startedTasks: 2, startedTaskIds: ["task-1", "task-1"] },
    { activeTasks: 1, activeTaskIds: [] },
    {
      startedTasks: 2,
      activeTasks: 2,
      startedTaskIds: ["task-1", "task-2"],
      activeTaskIds: ["task-1", "task-1"],
    },
    { activeTasks: 1, activeTaskIds: ["task-2"] },
    { startedTaskIds: [" bad"] },
    { activeTaskIds: "task-1" },
  ])("rejects inconsistent task consumption %j", (change) => {
    expectInvalid(parseCampaignConsumption({ ...validConsumption, ...change }));
  });

  it("parses a complete campaign candidate", () => {
    expect(parseCampaignCandidate(validCandidate)).toEqual({
      ok: true,
      value: validCandidate,
    });
  });

  it.each([null, [], "candidate"])(
    "rejects non-record candidate %j",
    (value) => {
      expectInvalid(parseCampaignCandidate(value));
    },
  );

  it.each([
    ["taskId", " bad"],
    ["taskId", "a".repeat(129)],
    ["initiativeId", ""],
    ["initiativeId", "bad/value"],
    ["rank", -1],
    ["rank", 0.5],
    ["blockerPhase", "blocked"],
    ["estimatedCostMicros", -1],
    ["estimatedCostMicros", 0.5],
    ["estimatedTokens", -1],
    ["estimatedTokens", "0"],
  ] as const)("rejects invalid candidate %s=%j", (field, value) => {
    expectInvalid(
      parseCampaignCandidate({ ...validCandidate, [field]: value }),
    );
  });

  it.each(["none", "pre-mutation", "post-mutation"] as const)(
    "accepts blocker phase %s",
    (blockerPhase) => {
      expect(
        parseCampaignCandidate({ ...validCandidate, blockerPhase }),
      ).toMatchObject({ ok: true, value: { blockerPhase } });
    },
  );

  it("parses campaign identities, times, and trimmed goals", () => {
    expect(parseCampaignTaskId("task-17")).toEqual({
      ok: true,
      value: "task-17",
    });
    expect(parseCampaignId("campaign:17")).toEqual({
      ok: true,
      value: "campaign:17",
    });
    expect(parseCampaignCheckpointTime("2026-01-01T00:00:00.000Z")).toEqual({
      ok: true,
      value: "2026-01-01T00:00:00.000Z",
    });
    expect(parseCampaignGoal("  deliver campaigns  ")).toEqual({
      ok: true,
      value: "deliver campaigns",
    });
  });

  it.each([null, "", " bad", "a".repeat(129)])(
    "rejects campaign identity %j",
    (value) => {
      expectInvalid(parseCampaignId(value));
      expectInvalid(parseCampaignTaskId(value));
    },
  );
  it.each([null, 0, "", "not-a-time"])("rejects campaign time %j", (value) => {
    expectInvalid(parseCampaignCheckpointTime(value));
  });
  it("accepts the maximum-length campaign goal", () => {
    expect(parseCampaignGoal("a".repeat(200))).toMatchObject({ ok: true });
  });
  it.each([null, "", "   ", "a".repeat(201)])(
    "rejects campaign goal %j",
    (value) => {
      expectInvalid(parseCampaignGoal(value));
    },
  );
});
