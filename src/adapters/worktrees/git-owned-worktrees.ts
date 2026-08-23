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
  decideWorktreeAbandonment,
  parseOwnedWorktreeRegistry,
  type OwnedWorktree,
  type OwnedWorktreeRegistry,
} from "../../core/worktrees/worktree-recovery.js";

export type WorktreeResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: { readonly code: string; readonly message: string };
    };

function failure(code: string, message: string): WorktreeResult<never> {
  return { ok: false, failure: { code, message } };
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
    readonly taskId: string;
    readonly claimId: string;
    readonly baselineRevision: string;
    readonly occurredAt: string;
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
    const entry: OwnedWorktree = {
      schemaVersion: 1,
      taskId: input.taskId,
      claimId: input.claimId,
      branch,
      path: realpathSync(path),
      baselineRevision: input.baselineRevision,
      heartbeatAt: new Date(input.occurredAt).toISOString(),
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
    readonly taskId: string;
    readonly previousClaimId: string;
    readonly claimId: string;
    readonly occurredAt: string;
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
    const replacement: OwnedWorktree = {
      ...entry,
      claimId: input.claimId,
      heartbeatAt: new Date(input.occurredAt).toISOString(),
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
    readonly taskId: string;
    readonly claimActive: boolean;
    readonly timestamp: string;
  }): WorktreeResult<{ readonly recoveryRef?: string }> {
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
      const recoveryRef = `refs/tiber/recovery/${entry.taskId}/${stamp}`;
      const decision = decideWorktreeAbandonment(entry, {
        canonicalWithinRoot: within(canonicalRoot, canonicalPath),
        gitRegistered: registered,
        branch,
        claimActive: input.claimActive,
        dirtySource,
        recoveryRef,
      });
      if (!decision.ok)
        return failure(
          decision.code,
          "foreign, ambiguous, or claimed worktree is retained",
        );
      if (dirtySource) this.createRecoveryCommit(canonicalPath, recoveryRef);
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
      return dirtySource
        ? { ok: true, value: { recoveryRef } }
        : { ok: true, value: {} };
    } catch {
      return failure(
        "TIBER_WORKTREE_RECOVERY_FAILED",
        "source preservation or safe cleanup could not be verified",
      );
    }
  }

  private createRecoveryCommit(path: string, ref: string): void {
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
        throw new Error("recovery ref observation mismatch");
    } finally {
      rmSync(index, { force: true });
    }
  }
}
