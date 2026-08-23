import { describe, expect, it } from "vitest";

import {
  authorizeMutation,
  authorizeReadPath,
  verifyToolInventory,
} from "../../src/core/tools/tool-policy.js";

describe("governed tool policy", () => {
  it("allows canonical reads within the repository", () => {
    expect(
      authorizeReadPath("/repo", "src/file.ts", "/repo/src/file.ts"),
    ).toEqual({
      allowed: true,
      code: "TIBER_READ_ALLOWED",
      detail: "read-only workspace inspection allowed",
    });
    expect(authorizeReadPath("/repo", ".", "/repo")).toMatchObject({
      allowed: true,
    });
  });

  it("denies lexical and symlink escapes before reading", () => {
    expect(authorizeReadPath("/repo", "../secret", "/secret")).toEqual({
      allowed: false,
      code: "TIBER_PATH_OUTSIDE_WORKSPACE",
      detail: "requested path escapes the workspace",
    });
    expect(authorizeReadPath("/repo", "linked", "/secret")).toEqual({
      allowed: false,
      code: "TIBER_PATH_SYMLINK_ESCAPE",
      detail: "canonical target escapes through a symlink",
    });
  });

  it("requires a remotely published claim for every mutation", () => {
    expect(authorizeMutation(false)).toEqual({
      allowed: false,
      code: "TIBER_MUTATION_CLAIM_REQUIRED",
      detail:
        "repository mutation requires a remotely published exclusive task claim",
    });
    expect(authorizeMutation(true)).toEqual({
      allowed: true,
      code: "TIBER_MUTATION_CLAIMED",
      detail: "published task claim authorizes governed mutation",
    });
  });

  it("accepts only the complete fixed governed inventory", () => {
    expect(
      verifyToolInventory(["write", "read", "edit", "bash", "read"]),
    ).toEqual({
      allowed: true,
      code: "TIBER_TOOL_INVENTORY_COMPLETE",
      detail: "all executable tools are governed",
    });
    expect(verifyToolInventory(["read", "shell", "remote-exec"])).toEqual({
      allowed: false,
      code: "TIBER_TOOL_INVENTORY_INCOMPLETE",
      detail: "ungoverned executable tools: remote-exec, shell",
    });
  });
});
