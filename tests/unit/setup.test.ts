import { describe, expect, it } from "vitest";

import {
  parseSetupPlan,
  requiredSetupConfirmations,
} from "../../src/core/configuration/setup.js";
import { none, some } from "../../src/core/types/option.js";

function plan(
  overrides: Readonly<Record<string, unknown>> = {},
): Readonly<Record<string, unknown>> {
  return {
    schemaVersion: 1,
    globalSettings: {
      assuranceLevel: "inherit",
      outputPreviewBytes: "inherit",
      worktreeMode: "inherit",
    },
    projectSettings: {
      assuranceLevel: "host-trusted",
      outputPreviewBytes: 16_384,
      worktreeMode: "isolated",
    },
    minimumAssuranceLevel: "unlocked",
    secretReferences: {},
    commandCatalog: { action: "keep" },
    ...overrides,
  };
}

function expectInvalid(input: unknown, message: string): void {
  expect(parseSetupPlan(input)).toMatchObject({
    ok: false,
    failure: {
      code: "TIBER_SETUP_PLAN_INVALID",
      message,
      safeContext: { domain: "setup" },
      retryability: "retry-after-input",
      requiredRecoveryEvidence: ["corrected-input"],
    },
  });
}

describe("guided setup plans", () => {
  it("parses an explicit answer for every layered setting and declaration choice", () => {
    const result = parseSetupPlan(plan());

    expect(result).toEqual({
      ok: true,
      value: {
        globalSettings: {
          assuranceLevel: none,
          outputPreviewBytes: none,
          worktreeMode: none,
        },
        projectSettings: {
          assuranceLevel: some("host-trusted"),
          outputPreviewBytes: some(16_384),
          worktreeMode: some("isolated"),
        },
        authority: {
          schemaVersion: 1,
          ceilings: { minimumAssuranceLevel: none },
          secretReferences: {},
        },
        commandCatalog: none,
      },
    });
  });

  it("accepts complete setting answers independently of object key order", () => {
    const result = parseSetupPlan(
      plan({
        globalSettings: {
          worktreeMode: "inherit",
          assuranceLevel: "inherit",
          outputPreviewBytes: "inherit",
        },
        projectSettings: {
          worktreeMode: "isolated",
          outputPreviewBytes: 16_384,
          assuranceLevel: "host-trusted",
        },
      }),
    );

    expect(result.ok).toBe(true);
  });

  it("parses a validated replacement command catalog", () => {
    const result = parseSetupPlan(
      plan({
        commandCatalog: {
          action: "replace",
          definition: {
            schemaVersion: 1,
            commands: [
              {
                name: "unit",
                executable: "/usr/bin/node",
                purpose: "test",
                argv: ["--test"],
                cwd: "worktree",
                environment: {},
                timeoutMs: 60_000,
                maxOutputBytes: 1_048_576,
              },
            ],
          },
        },
      }),
    );

    expect(result.ok).toBe(true);
    if (!result.ok || result.value.commandCatalog.kind === "none") return;
    expect(result.value.commandCatalog.value.commands).toMatchObject([
      { name: "unit", purpose: "test" },
    ]);
    expect(result.value.commandCatalog.value.digest).toMatch(
      /^sha256:[0-9a-f]{64}$/u,
    );
  });

  it.each([
    null,
    [],
    {},
    plan({ schemaVersion: 2 }),
    { ...plan(), unexpected: true },
  ])("rejects an incomplete or open top-level plan %#", (input) => {
    expectInvalid(
      input,
      "setup plan must use the complete schema version 1 shape",
    );
  });

  it("rejects a plan that omits a global setting", () => {
    expectInvalid(
      plan({
        globalSettings: {
          assuranceLevel: "inherit",
          worktreeMode: "inherit",
        },
      }),
      "setup plan must answer every global setting",
    );
  });

  it("rejects a plan that omits a project setting", () => {
    expectInvalid(
      plan({
        projectSettings: {
          assuranceLevel: "host-trusted",
          worktreeMode: "isolated",
        },
      }),
      "setup plan must answer every project setting",
    );
  });

  it.each([
    ["globalSettings", "setup plan contains invalid global settings"],
    ["projectSettings", "setup plan contains invalid project settings"],
  ] as const)("rejects invalid %s values", (scope, message) => {
    expectInvalid(
      plan({
        [scope]: {
          assuranceLevel: "unsafe",
          outputPreviewBytes: 16_384,
          worktreeMode: "isolated",
        },
      }),
      message,
    );
  });

  it.each([
    { minimumAssuranceLevel: "unsafe" },
    {
      secretReferences: {
        context7: { provider: "literal", name: "secret" },
      },
    },
  ])("rejects invalid authority values %#", (override) => {
    expectInvalid(
      plan(override),
      "setup plan contains invalid authority settings",
    );
  });

  it.each([
    [undefined, "setup plan must choose how to configure project commands"],
    [{}, "setup plan must choose how to configure project commands"],
    [{ action: 1 }, "setup plan must choose how to configure project commands"],
    [
      { action: "keep", extra: true },
      "setup command-catalog choice is invalid",
    ],
    [{ action: "replace" }, "setup command-catalog choice is invalid"],
    [{ action: "other" }, "setup command-catalog choice is invalid"],
    [
      {
        action: "other",
        definition: {
          schemaVersion: 1,
          commands: [
            {
              name: "unit",
              executable: "/usr/bin/node",
              purpose: "test",
              argv: ["--test"],
              cwd: "worktree",
              environment: {},
              timeoutMs: 60_000,
              maxOutputBytes: 1_048_576,
            },
          ],
        },
      },
      "setup command-catalog choice is invalid",
    ],
  ] as const)(
    "rejects invalid command choice %#",
    (commandCatalog, message) => {
      expectInvalid(plan({ commandCatalog }), message);
    },
  );

  it("accepts a replacement choice independently of object key order", () => {
    const result = parseSetupPlan(
      plan({
        commandCatalog: {
          definition: {
            schemaVersion: 1,
            commands: [
              {
                name: "unit",
                executable: "/usr/bin/node",
                purpose: "test",
                argv: ["--test"],
                cwd: "worktree",
                environment: {},
                timeoutMs: 60_000,
                maxOutputBytes: 1_048_576,
              },
            ],
          },
          action: "replace",
        },
      }),
    );

    expect(result.ok).toBe(true);
  });

  it("returns command compiler feedback for an invalid replacement", () => {
    expectInvalid(
      plan({
        commandCatalog: {
          action: "replace",
          definition: { schemaVersion: 1, commands: [] },
        },
      }),
      "command catalog must contain 1 to 64 closed command definitions",
    );
  });

  it("requires exact confirmations when a plan removes a floor and weakens effective assurance", () => {
    const current = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        projectSettings: {
          assuranceLevel: "inherit",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        minimumAssuranceLevel: "workspace-and-network-isolated",
      }),
    );
    const proposed = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "host-trusted",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        projectSettings: {
          assuranceLevel: "inherit",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
    );
    if (!current.ok || !proposed.ok) throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, proposed.value)).toEqual([
      "unlock minimumAssuranceLevel=workspace-and-network-isolated",
      "apply weaker assurance current=workspace-and-network-isolated proposed=host-trusted",
    ]);
  });

  it("requires only floor confirmation when effective assurance does not weaken", () => {
    const current = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        minimumAssuranceLevel: "workspace-and-network-isolated",
      }),
    );
    const proposed = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    if (!current.ok || !proposed.ok) throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, proposed.value)).toEqual([
      "unlock minimumAssuranceLevel=workspace-and-network-isolated",
    ]);
  });

  it("requires floor confirmation when an equal effective plan removes the floor", () => {
    const current = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    const proposed = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
    );
    if (!current.ok || !proposed.ok) throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, proposed.value)).toEqual([
      "unlock minimumAssuranceLevel=workspace-isolated",
    ]);
  });

  it("does not treat an equal or stronger floor as authority loosening", () => {
    const current = parseSetupPlan(
      plan({ minimumAssuranceLevel: "workspace-isolated" }),
    );
    const equal = parseSetupPlan(
      plan({ minimumAssuranceLevel: "workspace-isolated" }),
    );
    const stronger = parseSetupPlan(
      plan({
        minimumAssuranceLevel: "workspace-and-network-isolated",
      }),
    );
    if (!current.ok || !equal.ok || !stronger.ok)
      throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, equal.value)).toEqual([]);
    expect(requiredSetupConfirmations(current.value, stronger.value)).toEqual(
      [],
    );
  });

  it("does not require confirmation when floors and assurance stay equal or strengthen", () => {
    const current = parseSetupPlan(plan());
    const equal = parseSetupPlan(plan());
    const stronger = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 16_384,
          worktreeMode: "isolated",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    if (!current.ok || !equal.ok || !stronger.ok)
      throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, equal.value)).toEqual([]);
    expect(requiredSetupConfirmations(current.value, stronger.value)).toEqual(
      [],
    );
  });
});
