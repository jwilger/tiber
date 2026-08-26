import { describe, expect, it } from "vitest";

import {
  parsePermissionDecisionAt,
  parsePermissionScope,
  permissionScope,
} from "../../src/core/permissions/permission-values.js";

const SCOPE_FACTS = {
  role: "implementation",
  effect: "process",
  executable: "npm",
  argv: ["run", "test"],
  purpose: "test",
  cwd: "task-worktree",
  environment: { CI: "true" },
} as const;
const VALID_SCOPE =
  "sha256:39d939f226ce1de5ebfb6230ffff2627875c6b60cebf2e68f1fe93a29acc90a9";

function rejectsScope(values: readonly unknown[]): void {
  for (const value of values)
    expect(parsePermissionScope(value).ok, JSON.stringify(value)).toBe(false);
}

function rejectsDecisionAt(values: readonly unknown[]): void {
  for (const value of values)
    expect(parsePermissionDecisionAt(value).ok, JSON.stringify(value)).toBe(
      false,
    );
}

describe("permission semantic values", () => {
  it("derives a stable scope from every authority fact", () => {
    expect(permissionScope(SCOPE_FACTS)).toBe(VALID_SCOPE);
    const changedScopes = [
      permissionScope({ ...SCOPE_FACTS, role: "delivery" }),
      permissionScope({ ...SCOPE_FACTS, effect: "git-read" }),
      permissionScope({ ...SCOPE_FACTS, executable: "pnpm" }),
      permissionScope({ ...SCOPE_FACTS, argv: ["run", "build"] }),
      permissionScope({ ...SCOPE_FACTS, purpose: "build" }),
      permissionScope({ ...SCOPE_FACTS, cwd: "repository" }),
      permissionScope({ ...SCOPE_FACTS, environment: { CI: "false" } }),
    ];
    expect(new Set([VALID_SCOPE, ...changedScopes])).toHaveLength(8);
    expect(
      permissionScope({ ...SCOPE_FACTS, environment: { B: "2", A: "1" } }),
    ).toBe(
      "sha256:e0767a2949a2ce755a05291a2003e1b1c6db67aa03e39767fc90b97538e87d64",
    );
  });

  it("parses only exact lowercase SHA-256 permission scopes", () => {
    expect(parsePermissionScope(VALID_SCOPE)).toEqual({
      ok: true,
      value: VALID_SCOPE,
    });
    rejectsScope([
      { toString: () => VALID_SCOPE },
      1,
      VALID_SCOPE.slice(1),
      `${VALID_SCOPE}0`,
      `x${VALID_SCOPE}`,
      `${VALID_SCOPE}x`,
      `sha256:${"A".repeat(64)}`,
      `sha256:${"a".repeat(63)}`,
      `sha256:${"a".repeat(65)}`,
    ]);
  });

  it("parses only canonical ISO permission decision timestamps", () => {
    const timestamp = "2026-08-26T00:00:00.000Z";
    expect(parsePermissionDecisionAt(timestamp)).toEqual({
      ok: true,
      value: timestamp,
    });
    rejectsDecisionAt([
      { toString: () => timestamp },
      1,
      "not-a-date",
      "2026-08-26T00:00:00Z",
      "2026-08-26T00:00:00.000+00:00",
    ]);
  });

  it("returns complete purpose-specific stable failures", () => {
    expect(parsePermissionScope("bad")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_PERMISSION_VALUE_INVALID",
        message: "Invalid permissionScope",
        safeContext: { field: "permissionScope" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-value"],
        redaction: "public",
      },
    });
    const expectedDecisionFailure = {
      ok: false,
      failure: {
        code: "TIBER_PERMISSION_VALUE_INVALID",
        message: "Invalid permissionDecisionAt",
        safeContext: { field: "permissionDecisionAt" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-value"],
        redaction: "public",
      },
    } as const;
    expect(parsePermissionDecisionAt("bad")).toEqual(expectedDecisionFailure);
    expect(parsePermissionDecisionAt(Symbol("timestamp"))).toEqual(
      expectedDecisionFailure,
    );
  });
});
