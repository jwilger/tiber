import { describe, expect, it } from "vitest";

import {
  BUILT_IN_WORKFLOW,
  compileWorkflow,
  POLICY_FLOOR_STAGES,
} from "../../src/core/workflow/workflow.js";

const valid = {
  schemaVersion: 1,
  id: "project.restrictive",
  stages: ["project-check", ...POLICY_FLOOR_STAGES],
};

describe("data-only workflow compilation", () => {
  it("compiles the immutable built-in workflow", () => {
    expect(compileWorkflow(BUILT_IN_WORKFLOW)).toMatchObject({
      ok: true,
      value: { definition: BUILT_IN_WORKFLOW },
    });
  });

  it("canonicalizes and digests a policy-floor-preserving workflow", () => {
    expect(compileWorkflow(valid)).toEqual({
      ok: true,
      value: {
        definition: valid,
        canonicalJson: JSON.stringify(valid),
        digest:
          "sha256:f40098f7e2fe19dfbccb4e63e1809426107cbbdaf350a293cba7593476f1d6ff",
      },
    });
  });

  it.each([
    null,
    [],
    {},
    { ...valid, schemaVersion: 2 },
    { ...valid, id: "Invalid" },
    { ...valid, id: 1 },
    { ...valid, id: "valid!suffix" },
    { ...valid, stages: [] },
    { ...valid, stages: [...valid.stages, valid.stages[0]] },
    { ...valid, stages: [1, ...POLICY_FLOOR_STAGES] },
    { ...valid, stages: ["valid!suffix", ...POLICY_FLOOR_STAGES] },
    { ...valid, callback: "code" },
    {
      ...valid,
      stages: Array.from(
        { length: 65 },
        (_, index) => `stage-${String(index)}`,
      ),
    },
  ])("rejects malformed executable workflow data %j", (input) => {
    expect(compileWorkflow(input)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_WORKFLOW_INVALID" },
    });
  });

  it("accepts the exact maximum bounded stage count", () => {
    const extras = Array.from(
      { length: 49 },
      (_, index) => `extra-${String(index)}`,
    );
    expect(
      compileWorkflow({
        ...valid,
        stages: [...extras, ...POLICY_FLOOR_STAGES],
      }),
    ).toMatchObject({ ok: true });
  });

  it("reports malformed workflow details exactly", () => {
    expect(compileWorkflow(null)).toEqual({
      ok: false,
      failure: {
        code: "TIBER_WORKFLOW_INVALID",
        message: "workflow must be an object",
      },
    });
    expect(compileWorkflow({})).toEqual({
      ok: false,
      failure: {
        code: "TIBER_WORKFLOW_INVALID",
        message:
          "workflow must contain only a valid id and 1 to 64 unique data-only stages",
      },
    });
  });

  it("rejects a missing or reordered immutable floor stage", () => {
    expect(
      compileWorkflow({
        ...valid,
        stages: POLICY_FLOOR_STAGES.filter((stage) => stage !== "red"),
      }),
    ).toEqual({
      ok: false,
      failure: {
        code: "TIBER_WORKFLOW_POLICY_FLOOR",
        message: "workflow must preserve required stage order: red",
      },
    });
    expect(
      compileWorkflow({
        ...valid,
        stages: [
          ...POLICY_FLOOR_STAGES.slice(0, 3),
          "green",
          "red",
          ...POLICY_FLOOR_STAGES.slice(5),
        ],
      }),
    ).toMatchObject({ ok: false });
  });
});
