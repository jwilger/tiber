import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseOwnedWorktreePath,
  parseRecoveryReference,
  parseTaskBranchName,
  parseWorktreeAbandonedAt,
  parseWorktreeHeartbeatAt,
  type OwnedWorktreePath,
  type RecoveryReference,
  type TaskBranchName,
  type WorktreeAbandonedAt,
  type WorktreeHeartbeatAt,
} from "../../src/core/worktrees/worktree-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("worktree semantic values", () => {
  it("keeps path, branch, reference, and heartbeat purposes distinct", () => {
    expectTypeOf<OwnedWorktreePath>().not.toEqualTypeOf<TaskBranchName>();
    expectTypeOf<RecoveryReference>().not.toEqualTypeOf<OwnedWorktreePath>();
    expectTypeOf<WorktreeHeartbeatAt>().not.toEqualTypeOf<RecoveryReference>();
    expectTypeOf<WorktreeAbandonedAt>().not.toEqualTypeOf<WorktreeHeartbeatAt>();
  });

  it("parses valid worktree boundary values", () => {
    expect(parseOwnedWorktreePath("/agent/tiber/worktrees/task").ok).toBe(true);
    expect(parseTaskBranchName("tiber/task/2424c876").ok).toBe(true);
    expect(
      parseRecoveryReference("refs/tiber/recovery/2424c876/20260823T160000Z")
        .ok,
    ).toBe(true);
    expect(parseWorktreeHeartbeatAt("2026-08-23T16:00:00.000Z").ok).toBe(true);
    expect(parseWorktreeAbandonedAt("2026-08-23T17:00:00.000Z").ok).toBe(true);
  });

  it.each([
    [parseOwnedWorktreePath, "relative", "ownedWorktreePath"],
    [parseTaskBranchName, "main", "taskBranchName"],
    [parseRecoveryReference, "refs/heads/main", "recoveryReference"],
    [parseWorktreeHeartbeatAt, "2026-08-23", "worktreeHeartbeatAt"],
    [parseWorktreeAbandonedAt, "2026-08-23", "worktreeAbandonedAt"],
  ])("rejects values valid for a different purpose", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_WORKTREE_VALUE_INVALID", field),
    });
  });

  it("rejects coercible and boundary-escaping worktree values", () => {
    expect(parseOwnedWorktreePath("/").ok).toBe(false);
    expect(parseTaskBranchName({ toString: () => "tiber/task/task" }).ok).toBe(
      false,
    );
    expect(parseTaskBranchName("xtiber/task/task").ok).toBe(false);
    expect(parseTaskBranchName("tiber/task/task!").ok).toBe(false);
    expect(parseRecoveryReference("xrefs/tiber/recovery/task").ok).toBe(false);
    expect(parseRecoveryReference("refs/tiber/recovery/task!").ok).toBe(false);
    expect(parseWorktreeHeartbeatAt(1_787_507_200_000).ok).toBe(false);
    expect(parseWorktreeHeartbeatAt("invalid")).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_WORKTREE_VALUE_INVALID",
        "worktreeHeartbeatAt",
      ),
    });
    expect(parseWorktreeAbandonedAt(1_787_507_200_000).ok).toBe(false);
    expect(parseWorktreeAbandonedAt("invalid")).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_WORKTREE_VALUE_INVALID",
        "worktreeAbandonedAt",
      ),
    });
    expect(
      parseRecoveryReference({
        toString: () => "refs/tiber/recovery/task",
      }).ok,
    ).toBe(false);
  });
});
