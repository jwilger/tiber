import { describe, expect, it } from "vitest";

import {
  decidePermission,
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

describe("first-use permission policy", () => {
  it("offers ordinary first-use choices for an eligible unfamiliar action", () => {
    expect(decidePermission(request())).toEqual({
      status: "prompt",
      code: "TIBER_PERMISSION_REQUIRED",
      choices: ["deny-once", "deny-always", "allow-once", "allow-always"],
    });
  });

  it("honors a remembered repository-local denial before autonomy", () => {
    expect(
      decidePermission(
        request({
          autonomy: "repository",
          remembered: some("deny"),
        }),
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
        request({ workflow: "denied", remembered: some("allow") }),
      ),
    ).toEqual({ status: "denied", code: "TIBER_PERMISSION_WORKFLOW_DENIED" });
  });

  it.each(["planning", "readiness", "review", "setup", "classifier"] as const)(
    "denies process execution to the %s role",
    (role) => {
      expect(
        decidePermission(request({ role, remembered: some("allow") })),
      ).toEqual({
        status: "denied",
        code: "TIBER_PERMISSION_ROLE_DENIED",
      });
    },
  );

  it("requires exact one-use approval for arbitrary shell", () => {
    expect(
      decidePermission(
        request({
          autonomy: "repository",
          effect: "arbitrary-shell",
          risk: "destructive",
          remembered: some("allow"),
        }),
      ),
    ).toEqual({
      status: "prompt",
      code: "TIBER_PERMISSION_EXACT_APPROVAL_REQUIRED",
      choices: ["deny-once", "deny-always", "allow-once"],
    });
  });

  it("does not prompt in a headless session", () => {
    expect(decidePermission(request({ interactive: false }))).toEqual({
      status: "denied",
      code: "TIBER_PERMISSION_INTERACTION_REQUIRED",
    });
  });

  it("allows recognized routine repository work in routine mode", () => {
    expect(decidePermission(request({ risk: "routine" }))).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_ROUTINE",
    });
  });

  it("allows unfamiliar eligible repository work in repository-autonomy mode", () => {
    expect(decidePermission(request({ autonomy: "repository" }))).toEqual({
      status: "allowed",
      code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
    });
  });

  it.each(["publication", "privileged", "destructive"] as const)(
    "keeps %s work interactive in repository-autonomy mode",
    (risk) => {
      expect(
        decidePermission(request({ autonomy: "repository", risk })),
      ).toMatchObject({ status: "prompt" });
    },
  );
});
