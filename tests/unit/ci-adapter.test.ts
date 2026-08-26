import { describe, expect, it } from "vitest";

import {
  parseCiAuthorityCatalog,
  parseCiAuthorityOutput,
} from "../../src/adapters/ci/user-local-ci-authority.js";

const digest = "a".repeat(64);
const revision = "b".repeat(40);

describe("user-local CI authority boundary", () => {
  it("parses a closed digest-pinned executable catalog", () => {
    const result = parseCiAuthorityCatalog({
      schemaVersion: 1,
      authorities: [
        {
          name: "quality",
          executable: "/home/test/.pi/ci/quality",
          executableSha256: digest,
          argv: ["observe", "{revision}"],
        },
      ],
    });
    expect(result.ok).toBe(true);
  });

  it("parses a setup-generated GitHub Actions authority", () => {
    expect(
      parseCiAuthorityCatalog({
        schemaVersion: 1,
        authorities: [
          {
            kind: "github-actions",
            name: "github-actions",
            repository: "jwilger/tiber",
            requiredChecks: ["CI"],
            adapterSha256: digest,
          },
        ],
      }),
    ).toMatchObject({
      ok: true,
      value: {
        authorities: [
          {
            kind: "github-actions",
            name: "github-actions",
            repository: "jwilger/tiber",
            requiredChecks: ["CI"],
          },
        ],
      },
    });
  });

  it("rejects unknown catalog fields and relative executables", () => {
    expect(
      parseCiAuthorityCatalog({
        schemaVersion: 1,
        authorities: [
          {
            name: "quality",
            executable: "./ci",
            executableSha256: digest,
            argv: [],
            extra: true,
          },
        ],
      }).ok,
    ).toBe(false);
  });

  it("schema-validates an exact-revision observation", () => {
    const result = parseCiAuthorityOutput(
      "quality",
      revision,
      digest,
      JSON.stringify({
        schemaVersion: 1,
        authority: "quality",
        revision,
        status: "success",
      }),
    );
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value.status).toBe("success");
      expect(result.value.adapterDigest).toBe(digest);
      expect(result.value.observationDigest).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  it("rejects wrong-revision and malformed success output", () => {
    expect(
      parseCiAuthorityOutput(
        "quality",
        revision,
        digest,
        JSON.stringify({
          schemaVersion: 1,
          authority: "quality",
          revision: "c".repeat(40),
          status: "success",
        }),
      ).ok,
    ).toBe(false);
    expect(
      parseCiAuthorityOutput(
        "quality",
        revision,
        digest,
        '{"status":"success"}',
      ).ok,
    ).toBe(false);
  });
});
