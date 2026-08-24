import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

import {
  operationalFailure,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";
import type {
  ClaimBaselineRevision,
  TaskClaimId,
  TaskEventOccurredAt,
  TaskId,
} from "../../core/tasks/task-values.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  decideWorktreeAbandonment,
  parseOwnedWorktreeRegistry,
  type OwnedWorktree,
  type OwnedWorktreeRegistry,
} from "../../core/worktrees/worktree-recovery.js";
import {
  parseOwnedWorktreePath,
  parseRecoveryReference,
  parseTaskBranchName,
  parseWorktreeHeartbeatAt,
  type RecoveryReference,
  type WorktreeAbandonedAt,
} from "../../core/worktrees/worktree-values.js";

type WorktreeFailureCode =
  | "TIBER_WORKTREE_CLEANUP_DENIED"
  | "TIBER_WORKTREE_CREATE_FAILED"
  | "TIBER_WORKTREE_NOT_OWNED"
  | "TIBER_WORKTREE_OWNERSHIP_CONFLICT"
  | "TIBER_WORKTREE_PATH_UNSAFE"
  | "TIBER_WORKTREE_QUOTA"
  | "TIBER_WORKTREE_RECOVERY_FAILED"
  | "TIBER_WORKTREE_REGISTRY_INVALID"
  | "TIBER_WORKTREE_REGISTRY_IO"
  | "TIBER_WORKTREE_TAKEOVER_DENIED";
type WorktreeFailure = TiberFailure<
  WorktreeFailureCode,
  { readonly domain: "owned-worktrees" },
  "corrected-input" | "state-change" | "retry-operation"
>;

export type WorktreeResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: WorktreeFailure;
    };

function failure(
  code: WorktreeFailureCode,
  message: string,
): WorktreeResult<never> {
  const retryability =
    code === "TIBER_WORKTREE_CREATE_FAILED" ||
    code === "TIBER_WORKTREE_REGISTRY_IO" ||
    code === "TIBER_WORKTREE_RECOVERY_FAILED"
      ? "transient"
      : code === "TIBER_WORKTREE_PATH_UNSAFE"
        ? "retry-after-input"
        : "retry-after-state-change";
  return {
    ok: false,
    failure: operationalFailure(code, "owned-worktrees", message, retryability),
  };
}

function git(
  cwd: string,
  args: readonly string[],
  environment?: NodeJS.ProcessEnv,
): string {
  return execFileSync("git", [...args], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: environment,
  }).trim();
}

function within(root: string, candidate: string): boolean {
  const path = relative(root, candidate);
  return path !== "" && !path.startsWith("..") && !isAbsolute(path);
}

export class GitOwnedWorktrees {
  private readonly registryPath: string;
  private readonly worktreeRoot: string;

  public constructor(
    private readonly repository: string,
    agentDirectory: string,
  ) {
    const common = git(repository, [
      "rev-parse",
      "--path-format=absolute",
      "--git-common-dir",
    ]);
    const identity = createHash("sha256")
      .update(realpathSync(common))
      .digest("hex");
    this.registryPath = join(common, "tiber", "owned-worktrees.v1.json");
    this.worktreeRoot = join(agentDirectory, "tiber", "worktrees", identity);
  }

  public read(): WorktreeResult<OwnedWorktreeRegistry> {
    if (!existsSync(this.registryPath))
      return { ok: true, value: { schemaVersion: 1, worktrees: [] } };
    try {
      const value: unknown = JSON.parse(
        readFileSync(this.registryPath, "utf8"),
      );
      const parsed = parseOwnedWorktreeRegistry(value);
      return parsed.ok
        ? parsed
        : failure(parsed.failure.code, parsed.failure.message);
    } catch {
      return failure(
        "TIBER_WORKTREE_REGISTRY_INVALID",
        "owned worktree registry is unreadable",
      );
    }
  }

  private write(registry: OwnedWorktreeRegistry): boolean {
    const temporary = `${this.registryPath}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.registryPath), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(registry, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, this.registryPath);
      return true;
    } catch {
      rmSync(temporary, { force: true });
      return false;
    }
  }

  public create(input: {
    readonly taskId: TaskId;
    readonly claimId: TaskClaimId;
    readonly baselineRevision: ClaimBaselineRevision;
    readonly occurredAt: TaskEventOccurredAt;
  }): WorktreeResult<OwnedWorktree> {
    const registry = this.read();
    if (!registry.ok) return registry;
    if (registry.value.worktrees.length >= 8)
      return failure(
        "TIBER_WORKTREE_QUOTA",
        "owned worktree quota is exhausted",
      );
    const existing = registry.value.worktrees.find(
      (entry) => entry.taskId === input.taskId,
    );
    if (existing !== undefined)
      return existing.claimId === input.claimId
        ? { ok: true, value: existing }
        : failure(
            "TIBER_WORKTREE_OWNERSHIP_CONFLICT",
            "task worktree has another owner",
          );

    mkdirSync(this.worktreeRoot, { recursive: true, mode: 0o700 });
    const canonicalRoot = realpathSync(this.worktreeRoot);
    const path = resolve(canonicalRoot, input.taskId);
    if (!within(canonicalRoot, path) || existsSync(path))
      return failure(
        "TIBER_WORKTREE_PATH_UNSAFE",
        "owned worktree path is ambiguous",
      );
    const branch = `tiber/task/${input.taskId}`;
    try {
      git(this.repository, [
        "worktree",
        "add",
        "--no-checkout",
        "-b",
        branch,
        path,
        input.baselineRevision,
      ]);
      git(path, ["checkout", "--force", branch]);
    } catch {
      try {
        git(this.repository, ["worktree", "remove", "--force", path]);
      } catch {
        // Git may have failed before registering the path.
      }
      return failure(
        "TIBER_WORKTREE_CREATE_FAILED",
        "Git could not create the owned worktree",
      );
    }
    const parsedBranch = parseTaskBranchName(branch);
    const parsedPath = parseOwnedWorktreePath(realpathSync(path));
    const heartbeatAt = parseWorktreeHeartbeatAt(
      new Date(input.occurredAt).toISOString(),
    );
    if (!parsedBranch.ok || !parsedPath.ok || !heartbeatAt.ok)
      return failure(
        "TIBER_WORKTREE_REGISTRY_INVALID",
        "created worktree values violated their semantic invariants",
      );
    const entry: OwnedWorktree = {
      schemaVersion: 1,
      taskId: input.taskId,
      claimId: input.claimId,
      branch: parsedBranch.value,
      path: parsedPath.value,
      baselineRevision: input.baselineRevision,
      heartbeatAt: heartbeatAt.value,
    };
    if (
      !this.write({
        schemaVersion: 1,
        worktrees: [...registry.value.worktrees, entry],
      })
    ) {
      try {
        git(this.repository, ["worktree", "remove", "--force", path]);
        git(this.repository, ["branch", "-D", branch]);
      } catch {
        // The unregistered path is retained if safe rollback cannot be established.
      }
      return failure(
        "TIBER_WORKTREE_REGISTRY_IO",
        "worktree ownership was not durable",
      );
    }
    return { ok: true, value: entry };
  }

  public transferClaim(input: {
    readonly taskId: TaskId;
    readonly previousClaimId: TaskClaimId;
    readonly claimId: TaskClaimId;
    readonly occurredAt: TaskEventOccurredAt;
  }): WorktreeResult<OwnedWorktree> {
    const registry = this.read();
    if (!registry.ok) return registry;
    const entry = registry.value.worktrees.find(
      (item) => item.taskId === input.taskId,
    );
    if (entry?.claimId !== input.previousClaimId)
      return failure(
        "TIBER_WORKTREE_TAKEOVER_DENIED",
        "worktree ownership does not match the previous claim",
      );
    const heartbeatAt = parseWorktreeHeartbeatAt(
      new Date(input.occurredAt).toISOString(),
    );
    if (!heartbeatAt.ok)
      return failure(
        "TIBER_WORKTREE_REGISTRY_INVALID",
        "takeover heartbeat violated its semantic invariant",
      );
    const replacement: OwnedWorktree = {
      ...entry,
      claimId: input.claimId,
      heartbeatAt: heartbeatAt.value,
    };
    if (
      !this.write({
        schemaVersion: 1,
        worktrees: registry.value.worktrees.map((item) =>
          item.taskId === input.taskId ? replacement : item,
        ),
      })
    )
      return failure("TIBER_WORKTREE_REGISTRY_IO", "takeover was not durable");
    return { ok: true, value: replacement };
  }

  public abandon(input: {
    readonly taskId: TaskId;
    readonly claimStatus: "active" | "released";
    readonly timestamp: WorktreeAbandonedAt;
  }): WorktreeResult<{ readonly recoveryRef: Option<RecoveryReference> }> {
    const registry = this.read();
    if (!registry.ok) return registry;
    const entry = registry.value.worktrees.find(
      (item) => item.taskId === input.taskId,
    );
    if (entry === undefined)
      return failure(
        "TIBER_WORKTREE_NOT_OWNED",
        "no owned worktree exists for the task",
      );
    let canonicalPath: string;
    let registered: boolean;
    let branch: string | undefined;
    let dirtySource: boolean;
    try {
      canonicalPath = realpathSync(entry.path);
      const canonicalRoot = realpathSync(this.worktreeRoot);
      registered = git(this.repository, [
        "worktree",
        "list",
        "--porcelain",
      ]).includes(`worktree ${canonicalPath}\n`);
      branch = git(canonicalPath, ["branch", "--show-current"]);
      dirtySource =
        git(canonicalPath, ["rev-parse", "HEAD"]) !== entry.baselineRevision ||
        git(canonicalPath, [
          "status",
          "--porcelain",
          "--untracked-files=all",
        ]) !== "";
      const stamp = new Date(input.timestamp)
        .toISOString()
        .replaceAll(/[-:.]/gu, "");
      const parsedBranch = parseTaskBranchName(branch);
      const recoveryRef = parseRecoveryReference(
        `refs/tiber/recovery/${entry.taskId}/${stamp}`,
      );
      if (!recoveryRef.ok)
        return failure(
          "TIBER_WORKTREE_RECOVERY_FAILED",
          "recovery reference violated its semantic invariant",
        );
      const decision = decideWorktreeAbandonment(entry, {
        canonicalWithinRoot: within(canonicalRoot, canonicalPath),
        gitRegistered: registered,
        branch: parsedBranch.ok ? some(parsedBranch.value) : none,
        claimStatus: input.claimStatus,
        dirtySource,
        recoveryRef: some(recoveryRef.value),
      });
      if (!decision.ok)
        return failure(
          decision.code,
          "foreign, ambiguous, or claimed worktree is retained",
        );
      if (dirtySource) {
        const recovery = this.createRecoveryCommit(
          canonicalPath,
          recoveryRef.value,
        );
        if (!recovery.ok) return recovery;
      }
      git(this.repository, ["worktree", "remove", "--force", canonicalPath]);
      git(this.repository, ["branch", "-D", entry.branch]);
      if (
        !this.write({
          schemaVersion: 1,
          worktrees: registry.value.worktrees.filter(
            (item) => item.taskId !== entry.taskId,
          ),
        })
      )
        return failure(
          "TIBER_WORKTREE_REGISTRY_IO",
          "cleanup receipt was not durable",
        );
      return {
        ok: true,
        value: { recoveryRef: dirtySource ? some(recoveryRef.value) : none },
      };
    } catch {
      return failure(
        "TIBER_WORKTREE_RECOVERY_FAILED",
        "source preservation or safe cleanup could not be verified",
      );
    }
  }

  private createRecoveryCommit(
    path: string,
    ref: string,
  ): WorktreeResult<void> {
    const index = join(dirname(this.registryPath), `${randomUUID()}.index`);
    const environment = { ...process.env, GIT_INDEX_FILE: index };
    try {
      git(path, ["read-tree", "HEAD"], environment);
      git(path, ["add", "-A"], environment);
      const tree = git(path, ["write-tree"], environment);
      const parent = git(path, ["rev-parse", "HEAD"]);
      const commit = git(
        path,
        ["commit-tree", tree, "-p", parent, "-m", `tiber recovery for ${ref}`],
        environment,
      );
      git(path, ["update-ref", "-m", "tiber recovery", ref, commit]);
      if (git(path, ["rev-parse", ref]) !== commit)
        return failure(
          "TIBER_WORKTREE_RECOVERY_FAILED",
          "recovery ref observation did not match its intent",
        );
      return { ok: true, value: undefined };
    } finally {
      rmSync(index, { force: true });
    }
  }
}
