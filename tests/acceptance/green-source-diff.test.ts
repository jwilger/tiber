import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  observeSourceDiff,
  observeSourceSnapshot,
} from "../../src/adapters/git/git-source-diff.js";
import { claimBaselineRevision } from "../fixtures/task-values.js";
import { ownedWorktreePath } from "../fixtures/worktree-values.js";

function git(cwd: string, arguments_: readonly string[]): string {
  return execFileSync("git", [...arguments_], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

describe("bounded GREEN source-diff observation", () => {
  it("captures tracked and untracked source while excluding projected artifacts", () => {
    const repository = mkdtempSync(join(tmpdir(), "tiber-green-diff-"));
    git(repository, ["init"]);
    git(repository, ["config", "user.name", "Tiber Test"]);
    git(repository, ["config", "user.email", "tiber@example.test"]);
    writeFileSync(join(repository, "tracked.ts"), "export const value = 1;\n");
    git(repository, ["add", "tracked.ts"]);
    git(repository, ["commit", "-m", "test: baseline"]);
    const baseline = claimBaselineRevision(
      git(repository, ["rev-parse", "HEAD"]),
    );
    const worktree = ownedWorktreePath(repository);

    expect(observeSourceDiff(worktree, baseline)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SOURCE_OBSERVATION_INVALID" },
    });

    writeFileSync(join(repository, "tracked.ts"), "export const value = 2;\n");
    writeFileSync(join(repository, "added.ts"), "export const added = true;\n");
    const observed = observeSourceDiff(worktree, baseline);
    expect(observed.ok).toBe(true);
    if (!observed.ok) return;
    expect(observed.value).toContain("tracked.ts");
    expect(observed.value).toContain("added.ts");
    const snapshot = observeSourceSnapshot(worktree, baseline);
    expect(snapshot).toMatchObject({ ok: true });
    git(repository, ["add", "--all"]);
    git(repository, ["commit", "-m", "test: preserve snapshot"]);
    expect(observeSourceSnapshot(worktree, baseline)).toEqual(snapshot);

    writeFileSync(join(repository, "oversized.ts"), "x".repeat(65_537));
    expect(observeSourceDiff(worktree, baseline)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_SOURCE_OBSERVATION_INVALID" },
    });
  });
});
