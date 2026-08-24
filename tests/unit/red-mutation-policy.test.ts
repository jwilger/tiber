import { describe, expect, it } from "vitest";

import { authorizeWorkflowMutation } from "../../src/core/tools/red-mutation-policy.js";

const authority = {
  activeClaim: true,
  redAccepted: false,
  testMappings: ["tests/account-deletion.test.ts"],
};

describe("RED mutation gate", () => {
  it("allows only exact mapped test mutation before accepted RED", () => {
    expect(
      authorizeWorkflowMutation("tests/account-deletion.test.ts", authority),
    ).toEqual({
      allowed: true,
      code: "TIBER_TEST_MUTATION_ALLOWED",
      detail: "mapped test mutation is allowed before RED",
    });
    for (const path of [
      "src/account.ts",
      "tests/unrelated.test.ts",
      "tests/account-deletion.test.ts/escape",
      ".tiber/commands.json",
    ]) {
      expect(authorizeWorkflowMutation(path, authority)).toEqual({
        allowed: false,
        code: "TIBER_RED_REQUIRED",
        detail: "production mutation requires an accepted exact scenario RED",
      });
    }
  });

  it("accepts any exact mapping in a multi-test specification", () => {
    expect(
      authorizeWorkflowMutation("tests/account-deletion.test.ts", {
        ...authority,
        testMappings: ["tests/other.test.ts", "tests/account-deletion.test.ts"],
      }),
    ).toEqual({
      allowed: true,
      code: "TIBER_TEST_MUTATION_ALLOWED",
      detail: "mapped test mutation is allowed before RED",
    });
  });

  it("allows production mutation only after exact RED acceptance", () => {
    expect(
      authorizeWorkflowMutation("src/account.ts", {
        ...authority,
        redAccepted: true,
      }),
    ).toEqual({
      allowed: true,
      code: "TIBER_PRODUCTION_MUTATION_ALLOWED",
      detail:
        "accepted scenario RED authorizes a diagnostic production micro-step",
    });
    expect(
      authorizeWorkflowMutation("src/account.ts", {
        ...authority,
        activeClaim: false,
        redAccepted: true,
      }),
    ).toEqual({
      allowed: false,
      code: "TIBER_MUTATION_CLAIM_REQUIRED",
      detail: "mutation requires an exact active remote claim",
    });
  });

  it.each([
    "",
    "/absolute",
    "../escape",
    "..",
    "src/../.git/config",
    ".git/config",
    ".git",
    "src\0bad",
  ])("rejects non-canonical mutation path %j", (path) => {
    const decision = authorizeWorkflowMutation(path, {
      ...authority,
      redAccepted: true,
    });
    expect(decision).toEqual({
      allowed: false,
      code: "TIBER_MUTATION_PATH_INVALID",
      detail:
        "mutation path must be canonical, repository-relative, and outside Git metadata",
    });
  });
});
