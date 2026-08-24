import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const worktreeValuePurpose: unique symbol;
type WorktreeValue<Value, Purpose extends string> = Value & {
  readonly [worktreeValuePurpose]: Purpose;
};

export type TaskBranchName = WorktreeValue<string, "task-branch-name">;
export type OwnedWorktreePath = WorktreeValue<string, "owned-worktree-path">;
export type WorktreeHeartbeatAt = WorktreeValue<
  string,
  "worktree-heartbeat-at"
>;
export type WorktreeAbandonedAt = WorktreeValue<
  string,
  "worktree-abandoned-at"
>;
export type RecoveryReference = WorktreeValue<string, "recovery-reference">;

type WorktreeField =
  | "taskBranchName"
  | "ownedWorktreePath"
  | "worktreeHeartbeatAt"
  | "worktreeAbandonedAt"
  | "recoveryReference";
type WorktreeValueFailure = TiberFailure<
  "TIBER_WORKTREE_VALUE_INVALID",
  { readonly field: WorktreeField },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, WorktreeValueFailure>;

function invalid(field: WorktreeField): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_WORKTREE_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function valid<Purpose extends string>(
  value: string,
): WorktreeValue<string, Purpose> {
  return value as WorktreeValue<string, Purpose>;
}

export function parseTaskBranchName(value: unknown): Result<TaskBranchName> {
  return typeof value === "string" &&
    /^tiber\/task\/[A-Za-z0-9._-]{1,180}$/u.test(value)
    ? { ok: true, value: valid<"task-branch-name">(value) }
    : invalid("taskBranchName");
}

export function parseOwnedWorktreePath(
  value: unknown,
): Result<OwnedWorktreePath> {
  return typeof value === "string" &&
    value.startsWith("/") &&
    value.length > 1 &&
    !value.includes("\0")
    ? { ok: true, value: valid<"owned-worktree-path">(value) }
    : invalid("ownedWorktreePath");
}

export function parseWorktreeHeartbeatAt(
  value: unknown,
): Result<WorktreeHeartbeatAt> {
  // Stryker disable next-line ConditionalExpression: canonical ISO equality below independently rejects non-strings accepted by Date.parse; typeof establishes narrowing.
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return invalid("worktreeHeartbeatAt");
  return new Date(value).toISOString() === value
    ? { ok: true, value: valid<"worktree-heartbeat-at">(value) }
    : invalid("worktreeHeartbeatAt");
}

export function parseWorktreeAbandonedAt(
  value: unknown,
): Result<WorktreeAbandonedAt> {
  // Stryker disable next-line ConditionalExpression: canonical ISO equality below independently rejects non-strings accepted by Date.parse; typeof establishes narrowing.
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return invalid("worktreeAbandonedAt");
  return new Date(value).toISOString() === value
    ? { ok: true, value: valid<"worktree-abandoned-at">(value) }
    : invalid("worktreeAbandonedAt");
}

export function parseRecoveryReference(
  value: unknown,
): Result<RecoveryReference> {
  return typeof value === "string" &&
    /^refs\/tiber\/recovery\/[A-Za-z0-9._/-]{1,180}$/u.test(value)
    ? { ok: true, value: valid<"recovery-reference">(value) }
    : invalid("recoveryReference");
}
