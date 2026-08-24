import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  parseTaskEvent,
} from "../../src/core/tasks/task-board.js";
import {
  createAdHocCampaignTask,
  decideCampaignSchedule,
  mergeCampaignAttention,
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignConsumption,
  parseCampaignGoal,
  parseCampaignId,
} from "../../src/core/campaigns/campaign.js";

function parsed<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid fixture");
  return result.value;
}

const campaignGoalEventDocument = {
  schemaVersion: 1,
  eventId: "11111111-1111-4111-8111-111111111111",
  kind: "task-campaign-goal-created",
  occurredAt: "2026-01-01T00:00:00.000Z",
  campaignId: "campaign-17",
  task: {
    id: "22222222-2222-4222-8222-222222222222",
    title: "Ship deterministic campaigns",
    description: "Ad-hoc campaign goal",
  },
} as const;

const bounds = parsed(
  parseCampaignBounds({
    taskLimit: 3,
    initiativeTaskLimit: 1,
    durationLimitMs: 60_000,
    costLimitMicros: 1_000_000,
    tokenLimit: 10_000,
    concurrencyLimit: 2,
  }),
);
const empty = parsed(
  parseCampaignConsumption({
    startedTasks: 0,
    elapsedMs: 0,
    costMicros: 0,
    tokens: 0,
    activeTasks: 0,
    startedTaskIds: [],
    activeTaskIds: [],
    initiativeStarts: {},
  }),
);

function candidate(value: {
  taskId: string;
  initiativeId: string;
  rank: number;
  blockerPhase?: "none" | "pre-mutation" | "post-mutation";
}) {
  return parsed(
    parseCampaignCandidate({
      taskId: value.taskId,
      initiativeId: value.initiativeId,
      rank: value.rank,
      blockerPhase: value.blockerPhase ?? "none",
      estimatedCostMicros: 100_000,
      estimatedTokens: 1_000,
    }),
  );
}

describe("bounded autonomous campaign scheduling", () => {
  it("returns no eligible work when no candidate can start", () => {
    expect(
      decideCampaignSchedule({ bounds, consumption: empty, candidates: [] }),
    ).toEqual({
      effects: [],
      attention: [],
      checkpoint: { reason: "no-eligible-work", consumption: empty },
    });
    const saturated = parsed(
      parseCampaignConsumption({
        ...empty,
        initiativeStarts: { "initiative-a": 1 },
      }),
    );
    expect(
      decideCampaignSchedule({
        bounds,
        consumption: saturated,
        candidates: [
          candidate({
            taskId: "task-a",
            initiativeId: "initiative-a",
            rank: 1,
          }),
        ],
      }).checkpoint.reason,
    ).toBe("no-eligible-work");
  });

  it("honors total, initiative, and concurrency bounds deterministically", () => {
    const decision = decideCampaignSchedule({
      bounds,
      consumption: empty,
      candidates: [
        candidate({ taskId: "task-b", initiativeId: "initiative-a", rank: 2 }),
        candidate({ taskId: "task-a", initiativeId: "initiative-a", rank: 1 }),
        candidate({ taskId: "task-c", initiativeId: "initiative-b", rank: 3 }),
      ],
    });

    expect(decision.effects).toEqual([
      { kind: "start-task", taskId: "task-a", initiativeId: "initiative-a" },
      { kind: "start-task", taskId: "task-c", initiativeId: "initiative-b" },
    ]);
    expect(decision.checkpoint.reason).toBe("concurrency-bound");
  });

  it("releases and defers pre-mutation blockers while retaining post-mutation work and continuing independently", () => {
    const decision = decideCampaignSchedule({
      bounds: parsed(
        parseCampaignBounds({ ...bounds, concurrencyLimit: 3, taskLimit: 3 }),
      ),
      consumption: empty,
      candidates: [
        candidate({
          taskId: "task-pre",
          initiativeId: "initiative-a",
          rank: 1,
          blockerPhase: "pre-mutation",
        }),
        candidate({
          taskId: "task-post",
          initiativeId: "initiative-b",
          rank: 2,
          blockerPhase: "post-mutation",
        }),
        candidate({
          taskId: "task-independent",
          initiativeId: "initiative-c",
          rank: 3,
        }),
      ],
    });

    expect(decision.effects).toEqual([
      {
        kind: "release-and-defer",
        taskId: "task-pre",
        initiativeId: "initiative-a",
      },
      {
        kind: "retain-blocked-work",
        taskId: "task-post",
        initiativeId: "initiative-b",
      },
      {
        kind: "start-task",
        taskId: "task-independent",
        initiativeId: "initiative-c",
      },
    ]);
    expect(decision.attention).toEqual([
      {
        taskId: "task-pre",
        initiativeId: "initiative-a",
        kind: "pre-mutation-blocker",
      },
      {
        taskId: "task-post",
        initiativeId: "initiative-b",
        kind: "post-mutation-blocker",
      },
    ]);
  });

  it("frees concurrency when a pre-mutation blocker is deferred", () => {
    const active = parsed(
      parseCampaignConsumption({
        ...empty,
        startedTasks: 2,
        activeTasks: 2,
        startedTaskIds: ["task-pre", "task-other"],
        activeTaskIds: ["task-pre", "task-other"],
        initiativeStarts: { "initiative-a": 1, "initiative-other": 1 },
      }),
    );
    const decision = decideCampaignSchedule({
      bounds,
      consumption: active,
      candidates: [
        candidate({
          taskId: "task-pre",
          initiativeId: "initiative-a",
          rank: 1,
          blockerPhase: "pre-mutation",
        }),
        candidate({
          taskId: "task-next",
          initiativeId: "initiative-b",
          rank: 2,
        }),
      ],
    });
    expect(decision.effects.map((effect) => effect.kind)).toEqual([
      "release-and-defer",
      "start-task",
    ]);
    expect(decision.checkpoint.consumption.activeTasks).toBe(2);
    expect(decision.checkpoint.consumption.activeTaskIds).toEqual([
      "task-other",
      "task-next",
    ]);
  });

  it("reports no eligible work when observations contain blockers only", () => {
    const decision = decideCampaignSchedule({
      bounds,
      consumption: empty,
      candidates: [
        candidate({
          taskId: "task-pre",
          initiativeId: "initiative-a",
          rank: 1,
          blockerPhase: "pre-mutation",
        }),
      ],
    });
    expect(decision.checkpoint.reason).toBe("no-eligible-work");
    expect(decision.checkpoint.consumption.activeTasks).toBe(0);
  });

  it("uses task identity to break equal-rank scheduling ties", () => {
    const decision = decideCampaignSchedule({
      bounds: parsed(parseCampaignBounds({ ...bounds, taskLimit: 1 })),
      consumption: empty,
      candidates: [
        candidate({ taskId: "task-z", initiativeId: "initiative-z", rank: 1 }),
        candidate({ taskId: "task-a", initiativeId: "initiative-a", rank: 1 }),
      ],
    });
    expect(decision.effects[0]).toMatchObject({ taskId: "task-a" });
  });

  it("stops before starting work that would exceed time, cost, token, or task bounds", () => {
    for (const [consumption, reason] of [
      [
        {
          ...empty,
          startedTasks: 3,
          startedTaskIds: ["done-1", "done-2", "done-3"],
        },
        "task-bound",
      ],
      [{ ...empty, elapsedMs: 60_000 }, "time-bound"],
      [{ ...empty, costMicros: 950_001 }, "cost-bound"],
      [{ ...empty, tokens: 9_001 }, "token-bound"],
      [
        {
          ...empty,
          startedTasks: 2,
          activeTasks: 2,
          startedTaskIds: ["active-1", "active-2"],
          activeTaskIds: ["active-1", "active-2"],
        },
        "concurrency-bound",
      ],
    ] as const) {
      const decision = decideCampaignSchedule({
        bounds,
        consumption: parsed(parseCampaignConsumption(consumption)),
        candidates: [
          candidate({
            taskId: "task-a",
            initiativeId: "initiative-a",
            rank: 1,
          }),
        ],
      });
      expect(decision.effects).toEqual([]);
      expect(decision.checkpoint.reason).toBe(reason);
    }
  });

  it("reserves exact estimated cost and tokens in the durable consumption", () => {
    const decision = decideCampaignSchedule({
      bounds: parsed(
        parseCampaignBounds({
          ...bounds,
          taskLimit: 5,
          concurrencyLimit: 5,
          costLimitMicros: 100_000,
          tokenLimit: 1_000,
        }),
      ),
      consumption: empty,
      candidates: [
        candidate({ taskId: "task-a", initiativeId: "initiative-a", rank: 1 }),
      ],
    });
    expect(decision.checkpoint).toEqual({
      reason: "cost-bound",
      consumption: {
        ...empty,
        startedTasks: 1,
        activeTasks: 1,
        startedTaskIds: ["task-a"],
        activeTaskIds: ["task-a"],
        costMicros: 100_000,
        tokens: 1_000,
        initiativeStarts: { "initiative-a": 1 },
      },
    });
  });

  it("recognizes an exact token boundary independently of cost", () => {
    const decision = decideCampaignSchedule({
      bounds: parsed(
        parseCampaignBounds({
          ...bounds,
          taskLimit: 5,
          concurrencyLimit: 5,
          tokenLimit: 1_000,
        }),
      ),
      consumption: empty,
      candidates: [
        candidate({ taskId: "task-a", initiativeId: "initiative-a", rank: 1 }),
      ],
    });
    expect(decision.checkpoint.reason).toBe("token-bound");
  });

  it("returns work-scheduled when capacity remains after a start", () => {
    const decision = decideCampaignSchedule({
      bounds: parsed(
        parseCampaignBounds({
          ...bounds,
          taskLimit: 5,
          concurrencyLimit: 5,
        }),
      ),
      consumption: empty,
      candidates: [
        candidate({ taskId: "task-a", initiativeId: "initiative-a", rank: 1 }),
      ],
    });
    expect(decision.checkpoint.reason).toBe("work-scheduled");
  });

  it("retains blocker attention even when new starts are already bounded", () => {
    const decision = decideCampaignSchedule({
      bounds,
      consumption: parsed(
        parseCampaignConsumption({
          ...empty,
          startedTasks: 3,
          startedTaskIds: ["done-1", "done-2", "done-3"],
        }),
      ),
      candidates: [
        candidate({
          taskId: "task-blocked",
          initiativeId: "initiative-a",
          rank: 1,
          blockerPhase: "post-mutation",
        }),
      ],
    });
    expect(decision.effects).toEqual([
      {
        kind: "retain-blocked-work",
        taskId: "task-blocked",
        initiativeId: "initiative-a",
      },
    ]);
    expect(decision.attention).toHaveLength(1);
    expect(decision.checkpoint.reason).toBe("task-bound");
  });

  it("keeps non-modal blocker attention durable and deduplicated", () => {
    const item = {
      kind: "pre-mutation-blocker" as const,
      taskId: candidate({
        taskId: "task-blocked",
        initiativeId: "initiative-a",
        rank: 1,
      }).taskId,
      initiativeId: candidate({
        taskId: "task-blocked",
        initiativeId: "initiative-a",
        rank: 1,
      }).initiativeId,
    };
    const later = {
      ...item,
      kind: "post-mutation-blocker" as const,
      initiativeId: candidate({
        taskId: "task-blocked",
        initiativeId: "initiative-b",
        rank: 1,
      }).initiativeId,
    };
    const earlier = {
      ...item,
      taskId: candidate({
        taskId: "task-a",
        initiativeId: "initiative-a",
        rank: 1,
      }).taskId,
    };
    expect(mergeCampaignAttention([item, later], [item, earlier])).toEqual([
      earlier,
      later,
      item,
    ]);
  });

  it("folds a campaign goal into a provenance-bearing shared Backlog task", () => {
    const event = parseTaskEvent(campaignGoalEventDocument);
    if (!event.ok) throw new Error("invalid fixture");
    expect(foldTaskEvents([event.value])).toMatchObject({
      mode: "writable",
      tasks: [
        {
          state: "Backlog",
          campaignProvenance: { kind: "some", value: "campaign-17" },
        },
      ],
    });
  });

  it("rejects malformed campaign-goal task provenance", () => {
    expect(parseTaskEvent(campaignGoalEventDocument)).toMatchObject({
      ok: true,
      value: campaignGoalEventDocument,
    });
    for (const document of [
      { ...campaignGoalEventDocument, schemaVersion: 2 },
      { ...campaignGoalEventDocument, kind: "unknown" },
      { ...campaignGoalEventDocument, eventId: "bad" },
      { ...campaignGoalEventDocument, occurredAt: "bad" },
      { ...campaignGoalEventDocument, campaignId: " bad" },
      {
        ...campaignGoalEventDocument,
        task: { ...campaignGoalEventDocument.task, id: "bad" },
      },
      {
        ...campaignGoalEventDocument,
        task: { ...campaignGoalEventDocument.task, title: "" },
      },
      {
        ...campaignGoalEventDocument,
        task: { ...campaignGoalEventDocument.task, description: 1 },
      },
    ])
      expect(parseTaskEvent(document)).toMatchObject({ ok: false });
  });

  it("creates a provenance-bearing Backlog task proposal for an ad-hoc goal", () => {
    const goal = parsed(parseCampaignGoal("Ship deterministic campaigns"));
    expect(
      createAdHocCampaignTask(goal, parsed(parseCampaignId("campaign-17"))),
    ).toEqual({
      title: "Ship deterministic campaigns",
      description: "Ad-hoc campaign goal: Ship deterministic campaigns",
      provenance: { kind: "campaign-goal", campaignId: "campaign-17" },
      initialState: "Backlog",
    });
  });
});
