import { describe, expect, it } from "vitest";

import {
  parseCiAdapterArgument,
  parseCiAuthorityName,
  parseCiDiagnosis,
  parseCiExecutableDigest,
  parseCiExecutablePath,
  parseCiGithubCheckName,
  parseCiGithubRepository,
  parseCiObservationDigest,
  parseCiRevision,
} from "../../src/core/ci/ci-values.js";

function accepts(
  parser: (value: unknown) => { readonly ok: boolean },
  value: unknown,
): void {
  expect(parser(value).ok).toBe(true);
}

function rejects(
  parser: (value: unknown) => { readonly ok: boolean },
  values: readonly unknown[],
): void {
  for (const value of values)
    expect(parser(value).ok, JSON.stringify(value)).toBe(false);
}

describe("CI semantic values", () => {
  it("parses bounded authority names", () => {
    accepts(parseCiAuthorityName, "a");
    accepts(parseCiAuthorityName, `a${"1".repeat(63)}`);
    rejects(parseCiAuthorityName, [
      { toString: () => "authority" },
      1,
      "",
      "A",
      "1name",
      "name_",
      `a${"1".repeat(64)}`,
    ]);
  });

  it("parses exact full commit revisions", () => {
    accepts(parseCiRevision, "a".repeat(40));
    rejects(parseCiRevision, [
      { toString: () => "a".repeat(40) },
      1,
      "a".repeat(39),
      "a".repeat(41),
      `g${"a".repeat(39)}`,
      `x${"a".repeat(40)}`,
    ]);
  });

  it("parses exact SHA-256 digests for observations and executables", () => {
    for (const parser of [parseCiObservationDigest, parseCiExecutableDigest]) {
      accepts(parser, "a".repeat(64));
      rejects(parser, [
        { toString: () => "a".repeat(64) },
        1,
        "a".repeat(63),
        "a".repeat(65),
        `g${"a".repeat(63)}`,
        `x${"a".repeat(64)}`,
      ]);
    }
  });

  it("parses trimmed bounded causal diagnoses", () => {
    accepts(parseCiDiagnosis, "a".repeat(16));
    accepts(parseCiDiagnosis, "a".repeat(2_000));
    rejects(parseCiDiagnosis, [
      { toString: () => "a".repeat(16) },
      1,
      "a".repeat(15),
      "a".repeat(2_001),
      ` ${"a".repeat(16)}`,
      `${"a".repeat(16)} `,
    ]);
  });

  it("parses absolute bounded NUL-free executable paths", () => {
    accepts(parseCiExecutablePath, "/a");
    accepts(parseCiExecutablePath, `/${"a".repeat(4_095)}`);
    rejects(parseCiExecutablePath, [
      { toString: () => "/a" },
      1,
      "a",
      `/${"a".repeat(4_096)}`,
      "/a\0b",
    ]);
  });

  it("parses exact bounded GitHub repository identities", () => {
    accepts(parseCiGithubRepository, "owner/repository");
    accepts(parseCiGithubRepository, `${"o".repeat(100)}/${"r".repeat(100)}`);
    rejects(parseCiGithubRepository, [
      { toString: () => "owner/repository" },
      1,
      "",
      "/repository",
      "owner/",
      "owner/repository/extra",
      "!owner/repository",
      "owner/repository-suffix!",
      "owner name/repository",
      `${"o".repeat(101)}/repository`,
      `owner/${"r".repeat(101)}`,
    ]);
  });

  it("parses trimmed bounded NUL-free GitHub check names", () => {
    accepts(parseCiGithubCheckName, "Q");
    accepts(parseCiGithubCheckName, "Q".repeat(256));
    rejects(parseCiGithubCheckName, [
      { trim: () => "Q", length: 1, includes: () => false },
      1,
      "",
      " Q",
      "Q ",
      "Q".repeat(257),
      "Q\0check",
    ]);
  });

  it("parses bounded NUL-free adapter arguments including empty literals", () => {
    accepts(parseCiAdapterArgument, "");
    accepts(parseCiAdapterArgument, "a".repeat(4_096));
    rejects(parseCiAdapterArgument, [
      { toString: () => "argument" },
      { length: 1, includes: () => false },
      1,
      "a".repeat(4_097),
      "a\0b",
    ]);
  });

  it("returns complete purpose-specific stable semantic failures", () => {
    expect(parseCiAuthorityName("bad_name")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CI_VALUE_INVALID",
        message: "Invalid ci",
        safeContext: { field: "ci" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["CI authority name is invalid"],
        redaction: "public",
      },
    });
    const failures = [
      parseCiRevision("bad"),
      parseCiObservationDigest("bad"),
      parseCiDiagnosis("bad"),
      parseCiExecutablePath("bad"),
      parseCiExecutableDigest("bad"),
      parseCiGithubRepository("bad"),
      parseCiGithubCheckName(""),
      parseCiAdapterArgument("a\0b"),
    ];
    expect(
      failures.map((result) =>
        result.ok ? "" : result.failure.requiredRecoveryEvidence[0],
      ),
    ).toEqual([
      "CI revision is invalid",
      "CI observation digest is invalid",
      "CI diagnosis is invalid",
      "CI executable path is invalid",
      "CI executable digest is invalid",
      "GitHub CI repository is invalid",
      "GitHub CI check name is invalid",
      "CI adapter argument is invalid",
    ]);
  });
});
