import { describe, expect, it } from "vitest";

import {
  parseSetupAgentDirectoryPath,
  parseSetupExpectedAuthorityDigest,
  parseSetupPlanDigest,
  parseSetupRepositoryPath,
} from "../../src/core/configuration/setup-values.js";

describe("guided setup paths", () => {
  it("distinguishes absolute repository and agent-directory paths", () => {
    expect(parseSetupRepositoryPath("/workspace/repository")).toEqual({
      ok: true,
      value: "/workspace/repository",
    });
    expect(parseSetupAgentDirectoryPath("/home/user/.pi/agent")).toEqual({
      ok: true,
      value: "/home/user/.pi/agent",
    });
    const maximumPath = `/${"x".repeat(4_095)}`;
    expect(parseSetupRepositoryPath(maximumPath)).toEqual({
      ok: true,
      value: maximumPath,
    });
  });

  it("accepts only canonical setup plan digests", () => {
    expect(parseSetupPlanDigest(`sha256:${"a".repeat(64)}`)).toEqual({
      ok: true,
      value: `sha256:${"a".repeat(64)}`,
    });
    expect(
      parseSetupExpectedAuthorityDigest(`sha256:${"b".repeat(64)}`),
    ).toEqual({
      ok: true,
      value: `sha256:${"b".repeat(64)}`,
    });
    for (const invalid of [
      1,
      "sha256:not-a-digest",
      `xsha256:${"a".repeat(64)}`,
      `sha256:${"a".repeat(64)}x`,
    ])
      expect(parseSetupPlanDigest(invalid)).toMatchObject({
        ok: false,
        failure: { safeContext: { field: "setupPlanDigest" } },
      });
    expect(parseSetupExpectedAuthorityDigest("invalid")).toMatchObject({
      ok: false,
      failure: {
        safeContext: { field: "setupExpectedAuthorityDigest" },
      },
    });
  });

  it.each([
    ["repository", parseSetupRepositoryPath, "setupRepositoryPath"],
    ["agent", parseSetupAgentDirectoryPath, "setupAgentDirectoryPath"],
  ] as const)("rejects invalid %s paths", (_name, parse, field) => {
    for (const value of [
      undefined,
      "relative/path",
      "/invalid\0path",
      `/${"x".repeat(4_096)}`,
    ]) {
      expect(parse(value)).toMatchObject({
        ok: false,
        failure: {
          code: "TIBER_SETUP_VALUE_INVALID",
          safeContext: { field },
          retryability: "retry-after-input",
          requiredRecoveryEvidence: ["corrected-value"],
        },
      });
    }
  });
});
