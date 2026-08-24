import { parseWorktreeAbandonedAt } from "../../src/core/worktrees/worktree-values.js";

export function worktreeAbandonedAt(value: string) {
  const parsed = parseWorktreeAbandonedAt(value);
  if (!parsed.ok) throw new Error("invalid worktree abandonment fixture");
  return parsed.value;
}
