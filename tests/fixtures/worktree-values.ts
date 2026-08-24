import {
  parseOwnedWorktreePath,
  parseWorktreeAbandonedAt,
} from "../../src/core/worktrees/worktree-values.js";

export function ownedWorktreePath(value: string) {
  const parsed = parseOwnedWorktreePath(value);
  if (!parsed.ok) throw new Error("invalid owned worktree path fixture");
  return parsed.value;
}

export function worktreeAbandonedAt(value: string) {
  const parsed = parseWorktreeAbandonedAt(value);
  if (!parsed.ok) throw new Error("invalid worktree abandonment fixture");
  return parsed.value;
}
