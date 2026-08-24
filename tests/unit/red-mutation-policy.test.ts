import { describe, expect, it } from "vitest";

import { authorizeWorkflowMutation } from "../../src/core/tools/red-mutation-policy.js";
import { testMappingPath } from "../fixtures/task-values.js";

const authority = {
  claimStatus: "published" as const,
  redStatus: "required" as const,
  refactorStatus: "revoked" as const,
  testMappings: [testMappingPath("tests/account-deletion.test.ts")],
};

describe("RED mutation gate", () => {
  it("allows only exact mapped test mutation before accepted RED", () => {
    expect(
      authorizeWorkflowMutation(
        "tests/account-deletion.test.ts",
        "production",
        authority,
      ),
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
      expect(authorizeWorkflowMutation(path, "production", authority)).toEqual({
        allowed: false,
        code: "TIBER_RED_REQUIRED",
        detail: "production mutation requires an accepted exact scenario RED",
      });
    }
  });

  it("accepts any exact mapping in a multi-test specification", () => {
    expect(
      authorizeWorkflowMutation(
        "tests/account-deletion.test.ts",
        "production",
        {
          ...authority,
          testMappings: [
            testMappingPath("tests/other.test.ts"),
            testMappingPath("tests/account-deletion.test.ts"),
          ],
        },
      ),
    ).toEqual({
      allowed: true,
      code: "TIBER_TEST_MUTATION_ALLOWED",
      detail: "mapped test mutation is allowed before RED",
    });
  });

  it("allows production mutation only after exact RED acceptance", () => {
    expect(
      authorizeWorkflowMutation("src/account.ts", "production", {
        ...authority,
        redStatus: "accepted",
      }),
    ).toEqual({
      allowed: true,
      code: "TIBER_PRODUCTION_MUTATION_ALLOWED",
      detail:
        "accepted scenario RED authorizes a diagnostic production micro-step",
    });
    expect(
      authorizeWorkflowMutation("src/account.ts", "production", {
        ...authority,
        claimStatus: "absent",
        redStatus: "accepted",
      }),
    ).toEqual({
      allowed: false,
      code: "TIBER_MUTATION_CLAIM_REQUIRED",
      detail: "mutation requires an exact active remote claim",
    });
  });

  it("allows refactoring only after clean reviewed GREEN", () => {
    expect(
      authorizeWorkflowMutation("src/account.ts", "refactor", {
        ...authority,
        redStatus: "accepted",
      }),
    ).toEqual({
      allowed: false,
      code: "TIBER_REFACTOR_REQUIRES_GREEN",
      detail: "refactoring requires a clean reviewed exact GREEN increment",
    });
    expect(
      authorizeWorkflowMutation("src/account.ts", "refactor", {
        ...authority,
        redStatus: "accepted",
        refactorStatus: "allowed",
      }),
    ).toEqual({
      allowed: true,
      code: "TIBER_REFACTOR_ALLOWED",
      detail: "exact observed GREEN authorizes refactoring",
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
    const decision = authorizeWorkflowMutation(path, "production", {
      ...authority,
      redStatus: "accepted",
    });
    expect(decision).toEqual({
      allowed: false,
      code: "TIBER_MUTATION_PATH_INVALID",
      detail:
        "mutation path must be canonical, repository-relative, and outside Git metadata",
    });
  });
});
