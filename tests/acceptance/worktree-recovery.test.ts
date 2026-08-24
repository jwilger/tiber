import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { describe, expect, it } from "vitest";

import { GitOwnedWorktrees } from "../../src/adapters/worktrees/git-owned-worktrees.js";
import {
  claimBaselineRevision,
  taskClaimId,
  taskEventOccurredAt,
  taskId,
} from "../fixtures/task-values.js";
import { worktreeAbandonedAt } from "../fixtures/worktree-values.js";

function git(cwd: string, args: readonly string[]): string {
  return execFileSync("git", [...args], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

describe("owned Git worktree recovery", () => {
  it("resumes interrupted ownership and preserves abandoned source privately", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-worktree-"));
    const repository = join(root, "repository");
    const agent = join(root, "agent");
    git(root, ["init", repository]);
    git(repository, ["config", "user.name", "Tiber Test"]);
    git(repository, ["config", "user.email", "tiber@example.test"]);
    writeFileSync(join(repository, "tracked.ts"), "export const value = 1;\n");
    git(repository, ["add", "tracked.ts"]);
    git(repository, ["commit", "-m", "test: baseline"]);
    const baseline = git(repository, ["rev-parse", "HEAD"]);
    const input = {
      taskId: taskId("2424c876-6180-4c64-976e-9ea4bd540744"),
      claimId: taskClaimId("00000000-0000-4000-8000-000000000001"),
      baselineRevision: claimBaselineRevision(baseline),
      occurredAt: taskEventOccurredAt("2026-08-23T16:00:00.000Z"),
    };

    const created = new GitOwnedWorktrees(repository, agent).create(input);
    expect(created.ok).toBe(true);
    if (!created.ok) return;
    writeFileSync(
      join(created.value.path, "tracked.ts"),
      "export const value = 2;\n",
    );
    writeFileSync(
      join(created.value.path, "new-source.ts"),
      "export const added = true;\n",
    );

    const resumed = new GitOwnedWorktrees(repository, agent).read();
    expect(resumed).toMatchObject({
      ok: true,
      value: { worktrees: [{ taskId: input.taskId, claimId: input.claimId }] },
    });

    const denied = new GitOwnedWorktrees(repository, agent).abandon({
      taskId: input.taskId,
      claimStatus: "active",
      timestamp: worktreeAbandonedAt("2026-08-23T17:00:00.000Z"),
    });
    expect(denied).toMatchObject({
      ok: false,
      failure: { code: "TIBER_WORKTREE_CLEANUP_DENIED" },
    });

    const abandoned = new GitOwnedWorktrees(repository, agent).abandon({
      taskId: input.taskId,
      claimStatus: "released",
      timestamp: worktreeAbandonedAt("2026-08-23T17:00:00.000Z"),
    });
    expect(abandoned, JSON.stringify(abandoned)).toMatchObject({ ok: true });
    if (!abandoned.ok || abandoned.value.recoveryRef.kind === "none") return;
    const recoveryRef = abandoned.value.recoveryRef.value;
    expect(git(repository, ["show", `${recoveryRef}:tracked.ts`])).toBe(
      "export const value = 2;",
    );
    expect(
      readFileSync(
        join(repository, ".git", "tiber", "owned-worktrees.v1.json"),
        "utf8",
      ),
    ).toContain('"worktrees": []');
    expect(git(repository, ["show", `${recoveryRef}:new-source.ts`])).toBe(
      "export const added = true;",
    );
    expect(git(repository, ["remote"])).toBe("");
  });
});
