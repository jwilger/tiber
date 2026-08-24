import { describe, expect, it } from "vitest";
import {
  parseExceptionBlockerClaim,
  parseExceptionExecutionObservation,
  parseExceptionExecutionTime,
  parseHumanExceptionApproval,
} from "../../src/core/exceptions/human-exception.js";
import { parseExceptionNecessityReviewOutput } from "../../src/adapters/models/pi-exception-necessity-reviewer.js";

const claim = {
  schemaVersion: 1,
  taskId: "task",
  runId: "run",
  revision: "0".repeat(40),
  goal: "goal",
  denialCode: "DENIED",
  compliantAlternatives: [],
  operation: {
    kind: "structured-command",
    executable: "/bin/echo",
    arguments: ["hello"],
    environment: [{ name: "MODE", value: "exact" }],
    workingDirectory: "/tmp",
    timeoutMs: 1000,
    maxOutputBytes: 1000,
    paths: ["file"],
    preimages: [{ path: "file", digest: "a".repeat(64) }],
  },
  stateDigest: "b".repeat(64),
};

function expectClaimInvalid(
  value: unknown,
  field = "exception blocker claim",
): void {
  expect(parseExceptionBlockerClaim(value)).toEqual({
    ok: false,
    failure: {
      code: "TIBER_EXCEPTION_VALUE_INVALID",
      message: "Invalid human exception values are invalid",
      safeContext: { field: "human exception values are invalid" },
      causes: [],
      retryability: "retry-after-input",
      requiredRecoveryEvidence: [field],
      redaction: "public",
    },
  });
}

describe("human exception semantic boundaries", () => {
  it("parses the complete exact blocker claim and stable failure metadata", () => {
    expect(parseExceptionBlockerClaim(claim)).toMatchObject({
      ok: true,
      value: claim,
    });
    for (const value of [null, [], "claim"]) expectClaimInvalid(value);
    expect(
      parseExceptionBlockerClaim({ ...claim, taskId: { length: 1 } }).ok,
    ).toBe(false);
    expect(
      parseExceptionBlockerClaim({
        ...claim,
        revision: { toString: () => "0".repeat(40) },
      }).ok,
    ).toBe(false);
    expect(
      parseExceptionBlockerClaim({
        ...claim,
        stateDigest: { toString: () => "b".repeat(64) },
      }).ok,
    ).toBe(false);
  });

  it.each([
    ["schemaVersion", 2],
    ["taskId", ""],
    ["runId", ""],
    ["revision", "bad"],
    ["revision", `${"0".repeat(40)}suffix`],
    ["revision", `prefix${"0".repeat(40)}`],
    ["goal", ""],
    ["denialCode", ""],
    ["compliantAlternatives", [1]],
    ["stateDigest", "bad"],
    ["stateDigest", `${"b".repeat(64)}suffix`],
    ["stateDigest", `prefix${"b".repeat(64)}`],
  ])("rejects invalid blocker field %s", (field, value) => {
    expect(parseExceptionBlockerClaim({ ...claim, [field]: value }).ok).toBe(
      false,
    );
  });

  it.each([
    ["kind", "shell"],
    ["executable", "echo"],
    ["arguments", [1]],
    ["workingDirectory", "relative"],
    ["timeoutMs", 0],
    ["timeoutMs", 1.5],
    ["timeoutMs", 60_001],
    ["maxOutputBytes", 0],
    ["maxOutputBytes", 1.5],
    ["maxOutputBytes", 1_048_577],
    ["paths", [1]],
    ["environment", [null]],
    ["environment", [{ name: "", value: "x" }]],
    ["environment", [{ name: "MODE", value: 1 }]],
    ["preimages", [null]],
    ["preimages", [{ path: "", digest: "a".repeat(64) }]],
    ["preimages", [{ path: "file", digest: "bad" }]],
  ])("rejects invalid frozen operation field %s=%j", (field, value) => {
    expect(
      parseExceptionBlockerClaim({
        ...claim,
        operation: { ...claim.operation, [field]: value },
      }).ok,
    ).toBe(false);
  });

  it("keeps distinct recovery evidence for each invalid boundary", () => {
    const claimFailure = parseExceptionBlockerClaim({
      ...claim,
      schemaVersion: 2,
    });
    const approvalFailure = parseHumanExceptionApproval(null);
    const lifetimeFailure = parseHumanExceptionApproval({
      attentionId: "a",
      approvedAt: "2026-08-24T14:00:00.000Z",
      expiresAt: "2026-08-24T14:00:00.000Z",
      humanIdentity: "human",
    });
    const timeFailure = parseExceptionExecutionTime("bad");
    const observationFailure = parseExceptionExecutionObservation(null);
    expect(
      [
        claimFailure,
        approvalFailure,
        lifetimeFailure,
        timeFailure,
        observationFailure,
      ].map((result) =>
        result.ok ? [] : result.failure.requiredRecoveryEvidence,
      ),
    ).toEqual([
      ["exception blocker claim"],
      ["human exception approval"],
      ["human exception approval lifetime"],
      ["exception execution time"],
      ["exception execution observation"],
    ]);
  });

  it("accepts exact operation bounds", () => {
    for (const [timeoutMs, maxOutputBytes] of [
      [1, 1],
      [60_000, 1_048_576],
    ])
      expect(
        parseExceptionBlockerClaim({
          ...claim,
          operation: { ...claim.operation, timeoutMs, maxOutputBytes },
        }).ok,
      ).toBe(true);
  });

  it("accepts only canonical execution times and short forward approvals", () => {
    expect(parseExceptionExecutionTime("2026-08-24T14:00:00.000Z").ok).toBe(
      true,
    );
    for (const value of ["not-time", null, "2026-08-24T14:00:00Z"])
      expect(parseExceptionExecutionTime(value).ok).toBe(false);
    const approval = {
      attentionId: "a",
      approvedAt: "2026-08-24T14:00:00.000Z",
      expiresAt: "2026-08-24T14:15:00.000Z",
      humanIdentity: "human",
    };
    expect(parseHumanExceptionApproval(approval).ok).toBe(true);
    for (const value of [
      null,
      [],
      { ...approval, attentionId: "" },
      { ...approval, humanIdentity: "" },
      { ...approval, approvedAt: "invalid" },
      { ...approval, expiresAt: "invalid" },
      { ...approval, expiresAt: "2026-08-24T14:15:00.001Z" },
      { ...approval, expiresAt: approval.approvedAt },
      { ...approval, expiresAt: "2026-08-24T13:59:59.999Z" },
    ])
      expect(parseHumanExceptionApproval(value).ok).toBe(false);
  });

  it("parses complete observations and rejects malformed receipts", () => {
    const value = {
      attemptId: "attempt",
      exitCode: 0,
      stdoutDigest: "a".repeat(64),
      stderrDigest: "b".repeat(64),
      observedAt: "2026-08-24T14:00:00.000Z",
    };
    expect(parseExceptionExecutionObservation(value).ok).toBe(true);
    for (const invalid of [
      null,
      [],
      { ...value, attemptId: "" },
      { ...value, exitCode: 1.5 },
      { ...value, stdoutDigest: "bad" },
      { ...value, stderrDigest: "bad" },
      { ...value, observedAt: "bad" },
    ])
      expect(parseExceptionExecutionObservation(invalid).ok).toBe(false);
  });

  it("parses only closed independent necessity review output", () => {
    expect(
      parseExceptionNecessityReviewOutput(
        '{"disposition":"necessary","rationale":"No route remains."}',
      ),
    ).toMatchObject({ ok: true, value: { disposition: "necessary" } });
    expect(parseExceptionNecessityReviewOutput("not-json").ok).toBe(false);
    expect(
      parseExceptionNecessityReviewOutput(
        '{"disposition":"necessary","rationale":"","extra":true}',
      ).ok,
    ).toBe(false);
  });
});
