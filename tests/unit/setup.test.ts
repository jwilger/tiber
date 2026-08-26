import { describe, expect, it } from "vitest";

import {
  digestSetupExpectedAuthority,
  digestSetupPlan,
  formatSetupPlan,
  parseSetupPlan,
  requiredSetupConfirmations,
  sameSetupAuthorityState,
  setupAuthorityStateCanReconcile,
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
    projectWorkflow: { action: "keep" },
    ...overrides,
  };
}

function workflowDefinition() {
  return {
    schemaVersion: 1,
    id: "project.workflow",
    stages: [
      "intake",
      "specification-readiness",
      "remote-claim",
      "baseline-revalidation",
      "red",
      "green",
      "lightweight-review",
      "full-verification",
      "final-review-1",
      "final-review-2",
      "final-review-3",
      "delivery",
      "exact-revision-ci",
      "claim-release",
      "cleanup",
      "done",
    ],
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
        commandCatalog: { kind: "keep" },
        projectWorkflow: { kind: "keep" },
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
    if (!result.ok || result.value.commandCatalog.kind !== "replace") return;
    expect(result.value.commandCatalog.catalog.commands).toMatchObject([
      { name: "unit", purpose: "test" },
    ]);
    expect(result.value.commandCatalog.catalog.digest).toMatch(
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

  it("accepts the bounded maximum number of secret references", () => {
    expect(
      parseSetupPlan(
        plan({
          secretReferences: Object.fromEntries(
            Array.from({ length: 64 }, (_value, index) => [
              `secret-${String(index)}`,
              { provider: "environment", name: `SECRET_${String(index)}` },
            ]),
          ),
        }),
      ).ok,
    ).toBe(true);
  });

  it.each([
    { minimumAssuranceLevel: "unsafe" },
    {
      secretReferences: {
        context7: { provider: "literal", name: "secret" },
      },
    },
    {
      secretReferences: Object.fromEntries(
        Array.from({ length: 65 }, (_value, index) => [
          `secret-${String(index)}`,
          { provider: "environment", name: `SECRET_${String(index)}` },
        ]),
      ),
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
    [
      { action: "remove", extra: true },
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
      "apply weaker global assurance current=workspace-isolated proposed=host-trusted",
      "apply weaker project assurance current=workspace-and-network-isolated proposed=host-trusted",
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

  it("formats a parsed plan for durable recovery with a stable semantic digest", () => {
    const parsed = parseSetupPlan(
      plan({
        secretReferences: {
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      }),
    );
    if (!parsed.ok) throw new Error(parsed.failure.message);

    const formatted = formatSetupPlan(parsed.value);
    const jsonRoundTrip: unknown = JSON.parse(JSON.stringify(formatted));
    const reparsed = parseSetupPlan(jsonRoundTrip);
    const reverseOrder = parseSetupPlan(
      plan({
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        },
      }),
    );
    expect(reparsed).toEqual(parsed);
    expect(digestSetupPlan(parsed.value)).toMatch(/^sha256:[0-9a-f]{64}$/u);
    expect(digestSetupExpectedAuthority(parsed.value)).toMatch(
      /^sha256:[0-9a-f]{64}$/u,
    );
    if (!reparsed.ok || !reverseOrder.ok)
      throw new Error("invalid setup round trip");
    expect(digestSetupPlan(reparsed.value)).toBe(digestSetupPlan(parsed.value));
    expect(digestSetupPlan(reverseOrder.value)).toBe(
      digestSetupPlan(parsed.value),
    );
  });

  it("parses command removal and built-in workflow selection", () => {
    const result = parseSetupPlan(
      plan({
        commandCatalog: { action: "remove" },
        projectWorkflow: { action: "built-in" },
      }),
    );

    expect(result).toMatchObject({
      ok: true,
      value: {
        commandCatalog: { kind: "remove" },
        projectWorkflow: { kind: "built-in" },
      },
    });
  });

  it("compiles a replacement project workflow against the policy floor", () => {
    const result = parseSetupPlan(
      plan({
        projectWorkflow: {
          action: "replace",
          definition: workflowDefinition(),
        },
      }),
    );

    expect(result).toMatchObject({
      ok: true,
      value: {
        projectWorkflow: {
          kind: "replace",
          workflow: { definition: { id: "project.workflow" } },
        },
      },
    });
  });

  it("accepts a project workflow replacement independently of choice key order", () => {
    expect(
      parseSetupPlan(
        plan({
          projectWorkflow: {
            definition: workflowDefinition(),
            action: "replace",
          },
        }),
      ).ok,
    ).toBe(true);
  });

  it.each([
    [undefined, "setup plan must choose how to configure project workflow"],
    [{}, "setup plan must choose how to configure project workflow"],
    [{ action: "other" }, "setup project-workflow choice is invalid"],
    [
      { action: "keep", extra: true },
      "setup project-workflow choice is invalid",
    ],
    [
      { action: "built-in", extra: true },
      "setup project-workflow choice is invalid",
    ],
    [
      { action: "other", definition: workflowDefinition() },
      "setup project-workflow choice is invalid",
    ],
    [
      { action: "replace", definition: workflowDefinition(), extra: true },
      "setup project-workflow choice is invalid",
    ],
    [
      { action: "replace", definition: {} },
      "workflow must contain only a valid id and 1 to 64 unique data-only stages",
    ],
  ] as const)(
    "rejects an invalid project workflow choice %#",
    (choice, message) => {
      expectInvalid(plan({ projectWorkflow: choice }), message);
    },
  );

  it("requires confirmation when global assurance weakens even if this project stays hermetic", () => {
    const current = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
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
          assuranceLevel: "hermetic",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
    );
    if (!current.ok || !proposed.ok) throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, proposed.value)).toEqual([
      "apply weaker global assurance current=hermetic proposed=host-trusted",
    ]);
  });

  it("distinguishes global and project assurance weakening without duplicate confirmations", () => {
    const current = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "hermetic",
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
    const inheriting = parseSetupPlan(
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
    const projectDistinct = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "host-trusted",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
        projectSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
    );
    if (!current.ok || !inheriting.ok || !projectDistinct.ok)
      throw new Error("invalid fixture");

    expect(requiredSetupConfirmations(current.value, inheriting.value)).toEqual(
      ["apply weaker global assurance current=hermetic proposed=host-trusted"],
    );
    expect(
      requiredSetupConfirmations(current.value, projectDistinct.value),
    ).toEqual([
      "apply weaker global assurance current=hermetic proposed=host-trusted",
      "apply weaker project assurance current=hermetic proposed=workspace-isolated",
    ]);
  });

  it("reconciles only atomic old-or-intended authority components", () => {
    const expected = parseSetupPlan(plan());
    const intended = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        },
        projectSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    const partial = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    const externalDrift = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 65_536,
          worktreeMode: "isolated",
        },
        minimumAssuranceLevel: "workspace-isolated",
      }),
    );
    const externalProjectDrift = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 65_536,
          worktreeMode: "current",
        },
      }),
    );
    const externalAuthorityDrift = parseSetupPlan(
      plan({ minimumAssuranceLevel: "hermetic" }),
    );
    if (
      !expected.ok ||
      !intended.ok ||
      !partial.ok ||
      !externalDrift.ok ||
      !externalProjectDrift.ok ||
      !externalAuthorityDrift.ok
    )
      throw new Error("invalid fixture");

    expect(
      setupAuthorityStateCanReconcile(
        expected.value,
        intended.value,
        partial.value,
      ),
    ).toBe(true);
    for (const drift of [
      externalDrift.value,
      externalProjectDrift.value,
      externalAuthorityDrift.value,
    ])
      expect(
        setupAuthorityStateCanReconcile(expected.value, intended.value, drift),
      ).toBe(false);
  });

  it("detects each distinct settings and secret-reference state change", () => {
    const base = parseSetupPlan(plan());
    const withoutProjectAssurance = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "inherit",
          outputPreviewBytes: 16_384,
          worktreeMode: "isolated",
        },
      }),
    );
    const withGlobalAssurance = parseSetupPlan(
      plan({
        globalSettings: {
          assuranceLevel: "host-trusted",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
    );
    const changedProjectValue = parseSetupPlan(
      plan({
        projectSettings: {
          assuranceLevel: "host-trusted",
          outputPreviewBytes: 32_768,
          worktreeMode: "isolated",
        },
      }),
    );
    const oneReference = parseSetupPlan(
      plan({
        secretReferences: {
          context7: { provider: "environment", name: "SHARED_API_KEY" },
        },
      }),
    );
    const renamedReference = parseSetupPlan(
      plan({
        secretReferences: {
          other: { provider: "environment", name: "SHARED_API_KEY" },
        },
      }),
    );
    const extraReference = parseSetupPlan(
      plan({
        secretReferences: {
          context7: { provider: "environment", name: "SHARED_API_KEY" },
          other: { provider: "environment", name: "OTHER_API_KEY" },
        },
      }),
    );
    if (
      !base.ok ||
      !withoutProjectAssurance.ok ||
      !withGlobalAssurance.ok ||
      !changedProjectValue.ok ||
      !oneReference.ok ||
      !renamedReference.ok ||
      !extraReference.ok
    )
      throw new Error("invalid fixture");

    for (const changed of [
      withoutProjectAssurance.value,
      withGlobalAssurance.value,
      changedProjectValue.value,
    ])
      expect(sameSetupAuthorityState(base.value, changed)).toBe(false);
    expect(
      sameSetupAuthorityState(oneReference.value, renamedReference.value),
    ).toBe(false);
    expect(
      sameSetupAuthorityState(oneReference.value, extraReference.value),
    ).toBe(false);
  });

  it("detects authority-state drift independently of declaration choices and reference ordering", () => {
    const current = parseSetupPlan(
      plan({
        secretReferences: {
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      }),
    );
    const equivalent = parseSetupPlan(
      plan({
        commandCatalog: { action: "remove" },
        projectWorkflow: { action: "built-in" },
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        },
      }),
    );
    const changedInputs = [
      plan({
        globalSettings: {
          assuranceLevel: "host-trusted",
          outputPreviewBytes: "inherit",
          worktreeMode: "inherit",
        },
      }),
      plan({
        projectSettings: {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        },
      }),
      plan({ minimumAssuranceLevel: "host-trusted" }),
      plan({
        secretReferences: {
          context7: { provider: "environment", name: "OTHER_API_KEY" },
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        },
      }),
      plan({
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      }),
    ];
    const changed = changedInputs.map(parseSetupPlan);
    if (
      !current.ok ||
      !equivalent.ok ||
      changed.some((candidate) => !candidate.ok)
    )
      throw new Error("invalid fixture");

    expect(sameSetupAuthorityState(current.value, equivalent.value)).toBe(true);
    for (const candidate of changed) {
      if (candidate.ok)
        expect(sameSetupAuthorityState(current.value, candidate.value)).toBe(
          false,
        );
    }
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
