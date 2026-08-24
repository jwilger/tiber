import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { FileHumanExceptionAuthority } from "../../src/adapters/exceptions/file-human-exception-authority.js";
import {
  parseExceptionBlockerClaim,
  parseExceptionExecutionObservation,
  parseExceptionExecutionTime,
  parseHumanExceptionApproval,
} from "../../src/core/exceptions/human-exception.js";

const roots: string[] = [];
const claimDocument = {
  schemaVersion: 1,
  taskId: "task-18",
  runId: "run-18",
  revision: "0123456789abcdef0123456789abcdef01234567",
  goal: "publish the reviewed artifact",
  denialCode: "TIBER_COMMAND_DENIED",
  compliantAlternatives: [],
  operation: {
    kind: "structured-command",
    executable: "/usr/bin/example",
    arguments: ["--exact", "value"],
    environment: [{ name: "MODE", value: "reviewed" }],
    workingDirectory: "/repo/worktree",
    timeoutMs: 30_000,
    maxOutputBytes: 65_536,
    paths: ["artifact.txt"],
    preimages: [{ path: "artifact.txt", digest: "a".repeat(64) }],
  },
  stateDigest: "b".repeat(64),
};

function executionTime(value: string) {
  const parsed = parseExceptionExecutionTime(value);
  if (!parsed.ok) throw new Error("invalid test execution time");
  return parsed.value;
}

async function root(): Promise<string> {
  const value = await mkdtemp(path.join(tmpdir(), "tiber-exceptions-"));
  roots.push(value);
  return value;
}

afterEach(async () => {
  await Promise.all(
    roots
      .splice(0)
      .map(async (value) => rm(value, { recursive: true, force: true })),
  );
});

describe("exact human exception authority", () => {
  it("executes one frozen necessary operation once and rejects replay, near matches, drift, and expiry", async () => {
    const directory = await root();
    const authority = new FileHumanExceptionAuthority(directory);
    const claim = parseExceptionBlockerClaim(claimDocument);
    expect(claim.ok).toBe(true);
    if (!claim.ok) return;

    const escalated = await authority.escalate(claim.value, {
      disposition: "necessary",
      rationale: "No compliant route can satisfy the stated goal.",
      reviewerIdentity: "independent-reviewer",
    });
    expect(escalated.ok).toBe(true);
    if (!escalated.ok) return;
    const duplicate = await authority.escalate(claim.value, {
      disposition: "necessary",
      rationale: "Still necessary.",
      reviewerIdentity: "other-independent-reviewer",
    });
    expect(duplicate).toEqual(escalated);

    const approval = parseHumanExceptionApproval({
      attentionId: escalated.value.attentionId,
      approvedAt: "2026-08-24T14:00:00.000Z",
      expiresAt: "2026-08-24T14:05:00.000Z",
      humanIdentity: "human:owner",
    });
    expect(approval.ok).toBe(true);
    if (!approval.ok) return;
    expect((await authority.approve(approval.value)).ok).toBe(true);

    const nearMatch = parseExceptionBlockerClaim({
      ...claimDocument,
      operation: {
        ...claimDocument.operation,
        arguments: ["--exact", "other"],
      },
    });
    expect(nearMatch.ok).toBe(true);
    if (!nearMatch.ok) return;
    expect(
      (
        await authority.consume(
          nearMatch.value,
          executionTime("2026-08-24T14:01:00.000Z"),
        )
      ).ok,
    ).toBe(false);
    expect(
      (
        await authority.consume(
          { ...claim.value, stateDigest: "c".repeat(64) },
          executionTime("2026-08-24T14:01:00.000Z"),
        )
      ).ok,
    ).toBe(false);

    const consumed = await authority.consume(
      claim.value,
      executionTime("2026-08-24T14:01:00.000Z"),
    );
    expect(consumed.ok).toBe(true);
    if (!consumed.ok) return;
    const observation = parseExceptionExecutionObservation({
      attemptId: consumed.value.attemptId,
      exitCode: 0,
      stdoutDigest: "d".repeat(64),
      stderrDigest: "e".repeat(64),
      observedAt: "2026-08-24T14:01:01.000Z",
    });
    expect(observation.ok).toBe(true);
    if (!observation.ok) return;
    expect((await authority.recordObservation(observation.value)).ok).toBe(
      true,
    );
    expect(
      (
        await authority.consume(
          claim.value,
          executionTime("2026-08-24T14:02:00.000Z"),
        )
      ).ok,
    ).toBe(false);

    const audit = JSON.parse(
      await readFile(path.join(directory, "audit.json"), "utf8"),
    ) as { events: unknown[] };
    expect(audit.events).toHaveLength(4);
    expect(audit.events.at(-1)).toMatchObject({
      kind: "exception-observed",
      exitCode: 0,
      stdoutDigest: "d".repeat(64),
      stderrDigest: "e".repeat(64),
    });

    const expiringClaim = parseExceptionBlockerClaim({
      ...claimDocument,
      runId: "run-expired",
    });
    expect(expiringClaim.ok).toBe(true);
    if (!expiringClaim.ok) return;
    const expiredAttention = await authority.escalate(expiringClaim.value, {
      disposition: "necessary",
      rationale: "Necessary but short-lived.",
      reviewerIdentity: "independent-reviewer",
    });
    expect(expiredAttention.ok).toBe(true);
    if (!expiredAttention.ok) return;
    const expiredApproval = parseHumanExceptionApproval({
      attentionId: expiredAttention.value.attentionId,
      approvedAt: "2026-08-24T14:00:00.000Z",
      expiresAt: "2026-08-24T14:01:00.000Z",
      humanIdentity: "human:owner",
    });
    expect(expiredApproval.ok).toBe(true);
    if (!expiredApproval.ok) return;
    await authority.approve(expiredApproval.value);
    expect(
      (
        await authority.consume(
          expiringClaim.value,
          executionTime("2026-08-24T14:01:00.000Z"),
        )
      ).ok,
    ).toBe(false);
  });

  it("rejects compliant routes, corrupt state, future approvals, and concurrent double consumption", async () => {
    const directory = await root();
    const authority = new FileHumanExceptionAuthority(directory);
    const claim = parseExceptionBlockerClaim(claimDocument);
    expect(claim.ok).toBe(true);
    if (!claim.ok) return;
    const route = await authority.escalate(
      { ...claim.value, compliantAlternatives: ["use the governed command"] },
      {
        disposition: "necessary",
        rationale: "claimed necessary",
        reviewerIdentity: "reviewer",
      },
    );
    expect(route.ok).toBe(false);

    const escalated = await authority.escalate(claim.value, {
      disposition: "necessary",
      rationale: "No route remains.",
      reviewerIdentity: "reviewer",
    });
    expect(escalated.ok).toBe(true);
    if (!escalated.ok) return;
    const approval = parseHumanExceptionApproval({
      attentionId: escalated.value.attentionId,
      approvedAt: "2026-08-24T14:02:00.000Z",
      expiresAt: "2026-08-24T14:03:00.000Z",
      humanIdentity: "human",
    });
    expect(approval.ok).toBe(true);
    if (!approval.ok) return;
    await authority.approve(approval.value);
    expect(
      (
        await authority.consume(
          claim.value,
          executionTime("2026-08-24T14:01:59.999Z"),
        )
      ).ok,
    ).toBe(false);
    const concurrent = await Promise.all([
      authority.consume(claim.value, executionTime("2026-08-24T14:02:00.000Z")),
      authority.consume(claim.value, executionTime("2026-08-24T14:02:00.000Z")),
    ]);
    expect(concurrent.filter((result) => result.ok)).toHaveLength(1);

    await writeFile(
      path.join(directory, "audit.json"),
      '{"attentions":[{"claim":"forged"}],"events":[]}\n',
    );
    const corrupt = await authority.pending();
    expect(corrupt.ok).toBe(false);
    if (!corrupt.ok)
      expect(corrupt.failure.code).toBe("TIBER_EXCEPTION_STORE_INVALID");
    await writeFile(path.join(directory, "audit.json"), "{not-json");
    const malformed = await authority.pending();
    expect(malformed.ok).toBe(false);
    if (!malformed.ok)
      expect(malformed.failure.code).toBe("TIBER_EXCEPTION_STORE_INVALID");
  });
});
