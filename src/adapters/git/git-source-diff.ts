import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  lstatSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative } from "node:path";

import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import type { ClaimBaselineRevision } from "../../core/tasks/task-values.js";
import {
  parseSourceDiffText,
  parseSourceSnapshotDigest,
  type SourceDiffText,
  type SourceSnapshotDigest,
} from "../../core/workflow/workflow-values.js";
import type { OwnedWorktreePath } from "../../core/worktrees/worktree-values.js";

type SourceDiffFailure = TiberFailure<
  "TIBER_SOURCE_OBSERVATION_INVALID",
  { readonly domain: "source-observation" },
  "corrected-input" | "state-change" | "retry-operation"
>;

function git(cwd: OwnedWorktreePath, arguments_: readonly string[]): string {
  return execFileSync("git", [...arguments_], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 70_000,
    timeout: 30_000,
  });
}

function failure(
  message: string,
  retryability: "retry-after-input" | "transient",
): TiberResult<never, SourceDiffFailure> {
  return {
    ok: false,
    failure: operationalFailure(
      "TIBER_SOURCE_OBSERVATION_INVALID",
      "source-observation",
      message,
      retryability,
    ),
  };
}

export function observeSourceSnapshot(
  worktree: OwnedWorktreePath,
  baseline: ClaimBaselineRevision,
): TiberResult<SourceSnapshotDigest, SourceDiffFailure> {
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "tiber-snapshot-"));
  const index = join(temporaryDirectory, "index");
  try {
    const environment = { ...process.env, GIT_INDEX_FILE: index };
    execFileSync("git", ["read-tree", baseline], {
      cwd: worktree,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 30_000,
    });
    execFileSync("git", ["add", "--all"], {
      cwd: worktree,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 30_000,
    });
    const tree = execFileSync("git", ["write-tree"], {
      cwd: worktree,
      env: environment,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 30_000,
    }).trim();
    const parsed = parseSourceSnapshotDigest(
      `sha256:${createHash("sha256").update(tree).digest("hex")}`,
    );
    return parsed.ok
      ? parsed
      : failure("source snapshot identity was malformed", "transient");
  } catch {
    return failure("source snapshot could not be observed", "transient");
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}

export function observeSourceDiff(
  worktree: OwnedWorktreePath,
  baseline: ClaimBaselineRevision,
): TiberResult<SourceDiffText, SourceDiffFailure> {
  try {
    const tracked = git(worktree, [
      "diff",
      "--binary",
      "--no-ext-diff",
      baseline,
      "--",
    ]);
    const additions: string[] = [];
    let byteLength = Buffer.byteLength(tracked);
    for (const path of git(worktree, [
      "ls-files",
      "--others",
      "--exclude-standard",
    ])
      .split("\n")
      .filter((path) => path.length > 0 && !path.startsWith(".tiber/"))
      .sort()) {
      const canonical = realpathSync(join(worktree, path));
      const fromRoot = relative(worktree, canonical);
      const status = lstatSync(canonical);
      if (
        fromRoot.startsWith("..") ||
        isAbsolute(fromRoot) ||
        !status.isFile() ||
        status.size > 65_536 - byteLength
      )
        return failure(
          "source diff contains an unsafe or oversized untracked file",
          "retry-after-input",
        );
      const addition = `--- /dev/null\n+++ b/${path}\n${readFileSync(canonical, "utf8")}`;
      byteLength += Buffer.byteLength(addition);
      additions.push(addition);
    }
    const parsed = parseSourceDiffText(`${tracked}${additions.join("\n")}`);
    return parsed.ok
      ? parsed
      : failure(
          "source diff is empty or exceeds its bound",
          "retry-after-input",
        );
  } catch {
    return failure("source diff could not be observed", "transient");
  }
}
