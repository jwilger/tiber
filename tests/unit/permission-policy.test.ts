import { describe, expect, it } from "vitest";

import {
  decidePermission,
  type AgentRole,
  type PermissionEffect,
  type PermissionRequest,
} from "../../src/core/permissions/permission-policy.js";
import { none, some } from "../../src/core/types/option.js";

function request(
  overrides: Partial<PermissionRequest> = {},
): PermissionRequest {
  return {
    autonomy: "routine",
    role: "implementation",
    effect: "process",
    risk: "unfamiliar",
    boundary: "repository",
    workflow: "authorized",
    remembered: none,
    interactive: true,
    persistable: true,
    ...overrides,
  };
}

const ORDINARY_PROMPT = {
  status: "prompt",
  code: "TIBER_PERMISSION_REQUIRED",
  choices: ["deny-once", "deny-always", "allow-once", "allow-always"],
} as const;
const EXACT_PROMPT = {
  status: "prompt",
  code: "TIBER_PERMISSION_EXACT_APPROVAL_REQUIRED",
  choices: ["deny-once", "deny-always", "allow-once"],
} as const;

function expectedForRole(
  role: "delivery" | "ci",
  effect: PermissionEffect,
): ReturnType<typeof decidePermission> {
  if (effect === "repository-read")
    return { status: "allowed", code: "TIBER_PERMISSION_READ_ONLY" };
  const roleAllows =
    role === "delivery"
      ? ["git-read", "git-mutate", "github-read", "github-mutate"].includes(
          effect,
        )
      : effect === "github-read";
  return roleAllows
    ? {
        status: "allowed",
        code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
      }
    : { status: "denied", code: "TIBER_PERMISSION_ROLE_DENIED" };
}

const EFFECTS: readonly PermissionEffect[] = [
  "repository-read",
  "process",
  "arbitrary-shell",
  "git-read",
  "git-mutate",
  "github-read",
  "github-mutate",
];

const RESTRICTED_ROLES: readonly AgentRole[] = [
  "coordinator",
  "planning",
  "readiness",
  "review",
  "setup",
  "classifier",
];

describe("first-use permission policy", () => {
  it("offers ordinary first-use choices for an eligible unfamiliar action", () => {
    expect(decidePermission(request())).toEqual(ORDINARY_PROMPT);
  });

  it("honors a remembered repository-local denial before autonomy", () => {
    expect(
      decidePermission(
        request({ autonomy: "repository", remembered: some("deny") }),
      ),
    ).toEqual({ status: "denied", code: "TIBER_PERMISSION_ALWAYS_DENIED" });
  });

  it("allows a remembered eligible action", () => {
    expect(decidePermission(request({ remembered: some("allow") }))).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_REMEMBERED",
    });
  });

  it("never lets permission bypass workflow policy", () => {
    expect(
      decidePermission(
        request({
          workflow: "denied",
          effect: "repository-read",
          remembered: some("allow"),
        }),
      ),
    ).toEqual({ status: "denied", code: "TIBER_PERMISSION_WORKFLOW_DENIED" });
  });

  it.each(RESTRICTED_ROLES)(
    "denies process and arbitrary-shell execution to the %s role",
    (role) => {
      for (const effect of ["process", "arbitrary-shell"] as const) {
        expect(
          decidePermission(
            request({ role, effect, remembered: some("allow") }),
          ),
        ).toEqual({
          status: "denied",
          code: "TIBER_PERMISSION_ROLE_DENIED",
        });
      }
    },
  );

  it("does not turn a restricted process role into a blanket role denial", () => {
    expect(
      decidePermission(
        request({
          role: "planning",
          effect: "git-read",
          autonomy: "repository",
        }),
      ),
    ).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
    });
  });

  it.each(["delivery", "ci"] as const)(
    "enforces the complete %s role ceiling",
    (role) => {
      for (const effect of EFFECTS) {
        expect(
          decidePermission(
            request({
              role,
              effect,
              autonomy: "repository",
              risk: "unfamiliar",
            }),
          ),
          effect,
        ).toEqual(expectedForRole(role, effect));
      }
    },
  );

  it("allows repository reads without prompting or consulting remembered denial", () => {
    expect(
      decidePermission(
        request({
          role: "review",
          effect: "repository-read",
          remembered: some("deny"),
          interactive: false,
        }),
      ),
    ).toEqual({ status: "allowed", code: "TIBER_PERMISSION_READ_ONLY" });
  });

  it.each([
    ["arbitrary shell", { effect: "arbitrary-shell" }],
    ["an external boundary", { boundary: "external" }],
    ["a non-persistable action", { persistable: false }],
    ["destructive work", { risk: "destructive" }],
    ["publication work", { risk: "publication" }],
    ["privileged work", { risk: "privileged" }],
  ] satisfies readonly (readonly [string, Partial<PermissionRequest>])[])(
    "requires exact one-use approval for %s",
    (_label, overrides) => {
      expect(
        decidePermission(
          request({
            autonomy: "repository",
            remembered: some("allow"),
            ...overrides,
          }),
        ),
      ).toEqual(EXACT_PROMPT);
    },
  );

  it("denies an exact-approval request in a headless session", () => {
    expect(
      decidePermission(
        request({
          autonomy: "repository",
          boundary: "external",
          remembered: some("allow"),
          interactive: false,
        }),
      ),
    ).toEqual({
      status: "denied",
      code: "TIBER_PERMISSION_INTERACTION_REQUIRED",
    });
  });

  it("does not prompt for an ordinary request in a headless session", () => {
    expect(decidePermission(request({ interactive: false }))).toEqual({
      status: "denied",
      code: "TIBER_PERMISSION_INTERACTION_REQUIRED",
    });
  });

  it("allows recognized routine repository work only in routine or repository autonomy", () => {
    expect(decidePermission(request({ risk: "routine" }))).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_ROUTINE",
    });
    expect(
      decidePermission(request({ autonomy: "repository", risk: "routine" })),
    ).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
    });
    expect(
      decidePermission(request({ autonomy: "ask-first", risk: "routine" })),
    ).toEqual(ORDINARY_PROMPT);
  });

  it("allows unfamiliar eligible repository work in repository-autonomy mode", () => {
    expect(decidePermission(request({ autonomy: "repository" }))).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
    });
  });
});
