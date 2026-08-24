import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseCanonicalReadTarget,
  parseClaimedWorkspaceRoot,
  parseRequestedWorkspacePath,
  type CanonicalReadTarget,
  type ClaimedWorkspaceRoot,
  type RequestedWorkspacePath,
} from "../../src/core/tools/tool-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("governed tool path values", () => {
  it("keeps root, request, and observed target purposes distinct", () => {
    expectTypeOf<ClaimedWorkspaceRoot>().not.toEqualTypeOf<RequestedWorkspacePath>();
    expectTypeOf<CanonicalReadTarget>().not.toEqualTypeOf<ClaimedWorkspaceRoot>();
  });

  it("parses each path once at the tool boundary", () => {
    expect(parseClaimedWorkspaceRoot("/repo").ok).toBe(true);
    expect(parseRequestedWorkspacePath("src/index.ts").ok).toBe(true);
    expect(parseCanonicalReadTarget("/repo/src/index.ts").ok).toBe(true);
  });

  it("rejects out-of-bound tool paths", () => {
    expect(parseClaimedWorkspaceRoot({ toString: () => "/repo" }).ok).toBe(
      false,
    );
    expect(parseClaimedWorkspaceRoot("/bad\0path").ok).toBe(false);
    expect(parseClaimedWorkspaceRoot("/" + "x".repeat(4_095)).ok).toBe(true);
    expect(parseClaimedWorkspaceRoot("/" + "x".repeat(4_096)).ok).toBe(false);
    expect(
      parseRequestedWorkspacePath({ length: 1, includes: () => false }).ok,
    ).toBe(false);
    expect(parseRequestedWorkspacePath("x".repeat(4_096)).ok).toBe(true);
    expect(parseRequestedWorkspacePath("x".repeat(4_097)).ok).toBe(false);
    expect(parseRequestedWorkspacePath("bad\0path").ok).toBe(false);
    expect(parseCanonicalReadTarget("/" + "x".repeat(4_095)).ok).toBe(true);
    expect(parseCanonicalReadTarget("/bad\0path").ok).toBe(false);
  });

  it.each([
    [parseClaimedWorkspaceRoot, "relative", "claimedWorkspaceRoot"],
    [parseRequestedWorkspacePath, "", "requestedWorkspacePath"],
    [parseCanonicalReadTarget, "relative", "canonicalReadTarget"],
  ])("rejects structurally invalid values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_TOOL_VALUE_INVALID", field),
    });
  });
});
