import { describe, expect, it } from "vitest";

import { parseWorkflowRequest } from "../../src/core/workflow/workflow-request.js";

describe("Pi-native workflow requests", () => {
  it("parses closed semantic requests inferred from normal conversation", () => {
    expect(parseWorkflowRequest({ kind: "campaign-status" })).toEqual({
      ok: true,
      value: { kind: "campaign-status" },
    });
    expect(
      parseWorkflowRequest({
        kind: "campaign-goal",
        goal: "Deliver bounded campaigns",
      }),
    ).toMatchObject({ ok: true, value: { kind: "campaign-goal" } });
    expect(
      parseWorkflowRequest({
        kind: "campaign-tick",
        candidates: [
          {
            taskId: "task-1",
            initiativeId: "initiative-1",
            rank: 1,
            blockerPhase: "none",
            estimatedCostMicros: 10,
            estimatedTokens: 20,
          },
        ],
      }),
    ).toEqual({
      ok: true,
      value: {
        kind: "campaign-tick",
        candidates: [
          {
            taskId: "task-1",
            initiativeId: "initiative-1",
            rank: 1,
            blockerPhase: "none",
            estimatedCostMicros: 10,
            estimatedTokens: 20,
          },
        ],
      },
    });
    expect(
      parseWorkflowRequest({
        kind: "begin-task",
        taskId: "edea26e4-c973-4957-9b6b-30491dfbd6dd",
      }),
    ).toMatchObject({ ok: true, value: { kind: "begin-task" } });
    expect(
      parseWorkflowRequest({
        kind: "begin-task",
        taskId: "edea26e4-c973-4957-9b6b-30491dfbd6dd",
        specification: {
          outcome: "Pi follows a governed workflow from normal intent",
          scenarios: [
            {
              name: "automatic progression",
              given: ["signed task state"],
              when: ["the user asks Pi to continue"],
              then: ["Pi requests the deterministic next transition"],
            },
          ],
          acceptanceCriteria: ["No manual command choreography"],
          exclusions: ["No inferred human approval"],
          dependencies: [],
          testMappings: ["tests/acceptance/workflow-request.test.ts"],
          architectureImplications:
            "Inference requests; host authority decides.",
        },
      }),
    ).toMatchObject({
      ok: true,
      value: { kind: "begin-task", specification: { kind: "some" } },
    });
    expect(
      parseWorkflowRequest({
        kind: "campaign-start",
        bounds: {
          taskLimit: 2,
          initiativeTaskLimit: 1,
          durationLimitMs: 60_000,
          costLimitMicros: 1_000_000,
          tokenLimit: 10_000,
          concurrencyLimit: 2,
        },
      }),
    ).toMatchObject({ ok: true, value: { kind: "campaign-start" } });
  });

  it("returns complete stable failure metadata", () => {
    expect(parseWorkflowRequest(null)).toEqual({
      ok: false,
      failure: {
        code: "TIBER_WORKFLOW_REQUEST_INVALID",
        message: "Invalid workflow-request",
        safeContext: { field: "workflow-request" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-workflow-request"],
        redaction: "public",
      },
    });
  });

  it.each([
    null,
    [],
    1,
    { kind: 1 },
    { kind: "wrong", taskId: "edea26e4-c973-4957-9b6b-30491dfbd6dd" },
    {
      kind: "wrong",
      bounds: {
        taskLimit: 2,
        initiativeTaskLimit: 1,
        durationLimitMs: 60_000,
        costLimitMicros: 1_000_000,
        tokenLimit: 10_000,
        concurrencyLimit: 2,
      },
    },
    { kind: "wrong", bounds: {} },
    { kind: "wrong", candidates: [] },
    { kind: "begin-task" },
    { kind: "begin-task", taskId: "not-an-id" },
    {
      kind: "begin-task",
      taskId: "edea26e4-c973-4957-9b6b-30491dfbd6dd",
      specification: {},
    },
    {
      kind: "begin-task",
      taskId: "edea26e4-c973-4957-9b6b-30491dfbd6dd",
      extra: true,
    },
    { kind: "campaign-status", extra: true },
    { kind: "campaign-tick", candidates: "none" },
    { kind: "campaign-tick", candidates: [{}] },
    { kind: "campaign-tick", candidates: [], completedTaskIds: [] },
    {
      kind: "campaign-tick",
      candidates: [
        {
          taskId: "task-1",
          initiativeId: "initiative-1",
          rank: 1,
          blockerPhase: "none",
          estimatedCostMicros: 1,
          estimatedTokens: 1,
        },
        {},
      ],
    },
    { kind: "campaign-goal", goal: "" },
    { kind: "campaign-start", bounds: {} },
    { kind: "campaign-status", goal: "extra" },
    { kind: "unknown" },
  ])("fails closed for malformed or open request %j", (value) => {
    expect(parseWorkflowRequest(value)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_WORKFLOW_REQUEST_INVALID" },
    });
  });
});
