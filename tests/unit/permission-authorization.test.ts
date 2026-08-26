import { describe, expect, it } from "vitest";

import {
  authorizeRequestedEffect,
  type PermissionDecisionStore,
  type PermissionPrompt,
} from "../../src/extension/permission-authorization.js";
import {
  parsePermissionDecisionAt,
  permissionScope,
} from "../../src/core/permissions/permission-values.js";
import { none, some } from "../../src/core/types/option.js";

const scope = permissionScope({
  role: "implementation",
  effect: "process",
  executable: "npm",
  argv: ["run", "test"],
  purpose: "test",
  cwd: "task-worktree",
  environment: {},
});

function decisionAt() {
  const parsed = parsePermissionDecisionAt("2026-08-26T00:00:00.000Z");
  if (!parsed.ok) throw new Error("invalid timestamp fixture");
  return parsed.value;
}

function store(
  remembered?: "allow" | "deny",
): PermissionDecisionStore & { readonly saved: string[] } {
  const saved: string[] = [];
  return {
    saved,
    lookup: () => ({
      ok: true,
      value: remembered === undefined ? none : some(remembered),
    }),
    remember: (_scope, decision) => {
      saved.push(decision);
      return { ok: true, value: undefined };
    },
  };
}

function prompt(
  choice: "deny-once" | "deny-always" | "allow-once" | "allow-always",
): PermissionPrompt & { readonly calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    choose: (_description, choices) => {
      calls.push(...choices);
      return Promise.resolve(some(choice));
    },
  };
}

describe("interactive effect authorization", () => {
  it("persists an always-allow decision selected by the human", async () => {
    const permissions = store();
    const human = prompt("allow-always");

    expect(
      await authorizeRequestedEffect(
        {
          autonomy: "ask-first",
          role: "implementation",
          effect: "process",
          risk: "unfamiliar",
          boundary: "repository",
          workflow: "authorized",
          interactive: true,
          persistable: true,
        },
        scope,
        "Run npm test in this repository",
        permissions,
        human,
        decisionAt(),
      ),
    ).toEqual({ status: "allowed", remembered: true });
    expect(permissions.saved).toEqual(["allow"]);
  });

  it("does not prompt when the role ceiling denies execution", async () => {
    const permissions = store("allow");
    const human = prompt("allow-once");

    expect(
      await authorizeRequestedEffect(
        {
          autonomy: "repository",
          role: "review",
          effect: "process",
          risk: "routine",
          boundary: "repository",
          workflow: "authorized",
          interactive: true,
          persistable: true,
        },
        scope,
        "Run npm test",
        permissions,
        human,
        decisionAt(),
      ),
    ).toEqual({
      status: "denied",
      code: "TIBER_PERMISSION_ROLE_DENIED",
    });
    expect(human.calls).toEqual([]);
  });

  it("never offers persistent allow for arbitrary shell", async () => {
    const permissions = store();
    const human = prompt("allow-once");

    expect(
      await authorizeRequestedEffect(
        {
          autonomy: "repository",
          role: "implementation",
          effect: "arbitrary-shell",
          risk: "destructive",
          boundary: "repository",
          workflow: "authorized",
          interactive: true,
          persistable: false,
        },
        scope,
        "Run exact shell command",
        permissions,
        human,
        decisionAt(),
      ),
    ).toEqual({ status: "allowed", remembered: false });
    expect(human.calls).toEqual(["deny-once", "deny-always", "allow-once"]);
    expect(permissions.saved).toEqual([]);
  });
});
