import { describe, expect, it } from "vitest";

import {
  decideWorktreeAbandonment,
  parseOwnedWorktreeRegistry,
  reconcileOwnedWorktrees,
} from "../../src/core/worktrees/worktree-recovery.js";

const owned = {
  schemaVersion: 1 as const,
  taskId: "2424c876-6180-4c64-976e-9ea4bd540744",
  claimId: "00000000-0000-4000-8000-000000000001",
  branch: "tiber/task/2424c876",
  path: "/repo/.tiber/worktrees/2424c876",
  baselineRevision: "a".repeat(40),
  heartbeatAt: "2026-08-23T16:00:00.000Z",
};

function registry(worktrees: readonly unknown[]): unknown {
  return { schemaVersion: 1, worktrees };
}

describe("owned worktree recovery", () => {
  it("parses a bounded durable registry and rejects malformed or excess ownership", () => {
    expect(parseOwnedWorktreeRegistry(registry([owned]))).toEqual({
      ok: true,
      value: { schemaVersion: 1, worktrees: [owned] },
    });
    const eight = Array.from({ length: 8 }, (_, index) => ({
      ...owned,
      taskId: `2424c87${String(index)}-6180-4c64-976e-9ea4bd540744`,
      path: `${owned.path}-${String(index)}`,
    }));
    expect(parseOwnedWorktreeRegistry(registry(eight))).toMatchObject({
      ok: true,
    });
    expect(
      parseOwnedWorktreeRegistry({ worktrees: [], schemaVersion: 1 }),
    ).toEqual({
      ok: true,
      value: { schemaVersion: 1, worktrees: [] },
    });
    for (const input of [
      null,
      {},
      { schemaVersion: 1, worktrees: [], extra: true },
      { schemaVersion: 2, worktrees: [] },
      { schemaVersion: 1, worktrees: "bad" },
      registry([{ ...owned, extra: true }]),
      registry([
        owned,
        { ...owned, claimId: "bad", path: `${owned.path}-bad` },
      ]),
      registry([{ ...owned, schemaVersion: 2 }]),
      registry([{ ...owned, taskId: `x${owned.taskId}` }]),
      registry([{ ...owned, taskId: `${owned.taskId}x` }]),
      registry([{ ...owned, taskId: 1 }]),
      registry([{ ...owned, claimId: "bad" }]),
      registry([{ ...owned, claimId: 1 }]),
      registry([{ ...owned, branch: "!bad" }]),
      registry([{ ...owned, branch: "valid!bad" }]),
      registry([{ ...owned, branch: 1 }]),
      registry([{ ...owned, path: "relative" }]),
      registry([{ ...owned, path: "/bad\0path" }]),
      registry([{ ...owned, path: 1 }]),
      registry([{ ...owned, baselineRevision: `x${owned.baselineRevision}` }]),
      registry([{ ...owned, baselineRevision: `${owned.baselineRevision}x` }]),
      registry([{ ...owned, baselineRevision: 1 }]),
      registry([{ ...owned, heartbeatAt: "bad" }]),
      registry([{ ...owned, heartbeatAt: 1 }]),
      registry([owned, owned]),
      registry([owned, { ...owned, path: `${owned.path}-duplicate-task` }]),
      registry([
        owned,
        { ...owned, taskId: "33333333-3333-4333-8333-333333333333" },
      ]),
      registry(Array.from({ length: 9 }, () => owned)),
      registry(
        Array.from({ length: 9 }, (_, index) => ({
          ...owned,
          taskId: `2424c8${String(index).padStart(2, "0")}-6180-4c64-976e-9ea4bd540744`,
          path: `${owned.path}-quota-${String(index)}`,
        })),
      ),
    ]) {
      const result = parseOwnedWorktreeRegistry(input);
      expect(result).toMatchObject({
        ok: false,
        failure: { code: "TIBER_WORKTREE_REGISTRY_INVALID" },
      });
      if (!result.ok) expect(result.failure.message.length).toBeGreaterThan(0);
    }
  });

  it("retains exact interrupted ownership and classifies every ambiguity", () => {
    const valid = {
      path: owned.path,
      canonicalWithinRoot: true,
      gitRegistered: true,
      branch: owned.branch,
      claimId: owned.claimId,
      processGroupAlive: false,
    };
    expect(reconcileOwnedWorktrees([owned], [valid])).toEqual({
      resumable: [owned],
      blocked: [],
      staleProcessGroups: [],
    });
    expect(
      reconcileOwnedWorktrees(
        [owned],
        [
          { ...valid, path: "/foreign" },
          { ...valid, processGroupAlive: true },
        ],
      ),
    ).toEqual({
      resumable: [owned],
      blocked: [],
      staleProcessGroups: [owned.taskId],
    });
    for (const observation of [
      undefined,
      { ...valid, canonicalWithinRoot: false },
      { ...valid, gitRegistered: false },
      { ...valid, branch: "foreign" },
      { ...valid, claimId: "foreign" },
    ]) {
      expect(
        reconcileOwnedWorktrees(
          [owned],
          observation === undefined ? [] : [observation],
        ),
      ).toEqual({ resumable: [], blocked: [owned], staleProcessGroups: [] });
    }
  });

  it("refuses foreign, ambiguous, actively claimed, or unrecoverable cleanup", () => {
    for (const observation of [
      {
        canonicalWithinRoot: false,
        gitRegistered: true,
        branch: owned.branch,
        claimActive: false,
      },
      {
        canonicalWithinRoot: true,
        gitRegistered: false,
        branch: owned.branch,
        claimActive: false,
      },
      {
        canonicalWithinRoot: true,
        gitRegistered: true,
        branch: "foreign",
        claimActive: false,
      },
      {
        canonicalWithinRoot: true,
        gitRegistered: true,
        branch: owned.branch,
        claimActive: true,
      },
    ]) {
      expect(
        decideWorktreeAbandonment(owned, {
          ...observation,
          dirtySource: true,
          recoveryRef: "refs/tiber/recovery/example",
        }),
      ).toEqual({ ok: false, code: "TIBER_WORKTREE_CLEANUP_DENIED" });
    }
    for (const recoveryRef of [
      "bad",
      "xrefs/tiber/recovery/example",
      "refs/tiber/recovery/example!",
    ]) {
      expect(
        decideWorktreeAbandonment(owned, {
          canonicalWithinRoot: true,
          gitRegistered: true,
          branch: owned.branch,
          claimActive: false,
          dirtySource: true,
          recoveryRef,
        }),
      ).toEqual({ ok: false, code: "TIBER_WORKTREE_CLEANUP_DENIED" });
    }
  });

  it("preserves dirty source under a private recovery ref before removal", () => {
    expect(
      decideWorktreeAbandonment(owned, {
        canonicalWithinRoot: true,
        gitRegistered: true,
        branch: owned.branch,
        claimActive: false,
        dirtySource: true,
        recoveryRef: "refs/tiber/recovery/2424c876/20260823T160000Z",
      }),
    ).toEqual({
      ok: true,
      effects: [
        {
          kind: "create-recovery-ref",
          path: owned.path,
          ref: "refs/tiber/recovery/2424c876/20260823T160000Z",
        },
        { kind: "remove-owned-worktree", path: owned.path },
        { kind: "remove-registry-entry", taskId: owned.taskId },
      ],
    });
  });

  it("removes clean owned work without manufacturing a recovery ref", () => {
    expect(
      decideWorktreeAbandonment(owned, {
        canonicalWithinRoot: true,
        gitRegistered: true,
        branch: owned.branch,
        claimActive: false,
        dirtySource: false,
        recoveryRef: "not-needed-for-clean-work",
      }),
    ).toEqual({
      ok: true,
      effects: [
        { kind: "remove-owned-worktree", path: owned.path },
        { kind: "remove-registry-entry", taskId: owned.taskId },
      ],
    });
  });
});
