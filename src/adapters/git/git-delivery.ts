import { execFileSync } from "node:child_process";

import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import {
  validateGitDeliveryReceipt,
  type GitDeliveryMode,
  type GitDeliveryReceipt,
} from "../../core/delivery/git-delivery.js";
import {
  parseDeliveryCommitRevision,
  parseDeliveryTreeDigest,
  type DeliveryCommitBody,
  type DeliveryCommitSubject,
  type DeliveryDestinationRef,
} from "../../core/delivery/git-delivery-values.js";
import type { ClaimBaselineRevision } from "../../core/tasks/task-values.js";
import { observeSourceSnapshot } from "./git-source-diff.js";
import type { OwnedWorktreePath } from "../../core/worktrees/worktree-values.js";
import { none, some, type Option } from "../../core/types/option.js";
import type { SourceSnapshotDigest } from "../../core/workflow/workflow-values.js";

export type GitDeliveryFailure = TiberFailure<
  | "TIBER_DELIVERY_COMMIT_FAILED"
  | "TIBER_DELIVERY_NON_FAST_FORWARD"
  | "TIBER_DELIVERY_OBSERVATION_INVALID"
  | "TIBER_DELIVERY_PUSH_FAILED"
  | "TIBER_DELIVERY_SIGNATURE_INVALID",
  { readonly domain: "git-delivery" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type Result<Value> = TiberResult<Value, GitDeliveryFailure>;

function failure(
  code: GitDeliveryFailure["code"],
  message: string,
): Result<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "git-delivery",
      message,
      "retry-after-state-change",
    ),
  };
}

function git(cwd: string, args: readonly string[]): Result<string> {
  try {
    return {
      ok: true,
      value: execFileSync("git", args, {
        cwd,
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
        timeout: 120_000,
        maxBuffer: 1_048_576,
      }).trim(),
    };
  } catch {
    return failure(
      "TIBER_DELIVERY_COMMIT_FAILED",
      "Git delivery command failed",
    );
  }
}

function observeRemote(
  cwd: string,
  destination: DeliveryDestinationRef,
): Result<Option<GitDeliveryReceipt["commit"]>> {
  const observed = git(cwd, ["ls-remote", "--heads", "origin", destination]);
  if (!observed.ok) return observed;
  if (observed.value.length === 0) return { ok: true, value: none };
  const revision = parseDeliveryCommitRevision(observed.value.split(/\s/u)[0]);
  return revision.ok
    ? { ok: true, value: some(revision.value) }
    : failure(
        "TIBER_DELIVERY_OBSERVATION_INVALID",
        "remote delivery revision was malformed",
      );
}

export function deliverGit(input: {
  readonly worktree: OwnedWorktreePath;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly mode: GitDeliveryMode;
  readonly destination: Option<DeliveryDestinationRef>;
  readonly subject: DeliveryCommitSubject;
  readonly body: DeliveryCommitBody;
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
}): Result<GitDeliveryReceipt> {
  const before = observeSourceSnapshot(input.worktree, input.baselineRevision);
  if (!before.ok || before.value !== input.sourceSnapshotDigest)
    return failure(
      "TIBER_DELIVERY_OBSERVATION_INVALID",
      "source changed after final review",
    );
  const staged = git(input.worktree, ["add", "--all"]);
  if (!staged.ok) return staged;
  const afterStaging = observeSourceSnapshot(
    input.worktree,
    input.baselineRevision,
  );
  if (!afterStaging.ok || afterStaging.value !== input.sourceSnapshotDigest)
    return failure(
      "TIBER_DELIVERY_OBSERVATION_INVALID",
      "source changed while preparing delivery",
    );
  const treeBefore = git(input.worktree, ["write-tree"]);
  if (!treeBefore.ok) return treeBefore;
  const committed = git(input.worktree, [
    "commit",
    "-S",
    "-m",
    input.subject,
    "-m",
    input.body,
  ]);
  if (!committed.ok) return committed;
  const head = git(input.worktree, ["rev-parse", "HEAD"]);
  const tree = git(input.worktree, ["rev-parse", "HEAD^{tree}"]);
  const parent = git(input.worktree, ["rev-parse", "HEAD^"]);
  if (!head.ok || !tree.ok || !parent.ok)
    return failure(
      "TIBER_DELIVERY_COMMIT_FAILED",
      "commit identity was unavailable",
    );
  const commit = parseDeliveryCommitRevision(head.value);
  const treeDigest = parseDeliveryTreeDigest(tree.value);
  if (
    !commit.ok ||
    !treeDigest.ok ||
    tree.value !== treeBefore.value ||
    parent.value !== input.baselineRevision
  )
    return failure(
      "TIBER_DELIVERY_OBSERVATION_INVALID",
      "commit tree did not preserve the staged snapshot",
    );
  const signature = git(input.worktree, ["verify-commit", commit.value]);
  if (!signature.ok)
    return failure(
      "TIBER_DELIVERY_SIGNATURE_INVALID",
      "delivery commit signature was not valid",
    );
  let remoteCommit: Option<GitDeliveryReceipt["commit"]> = none;
  if (input.destination.kind === "some") {
    const before = observeRemote(input.worktree, input.destination.value);
    if (!before.ok) return before;
    if (before.value.kind === "some") {
      const checkRef = "refs/tiber/delivery-check";
      const fetched = git(input.worktree, [
        "fetch",
        "--no-tags",
        "origin",
        `+${input.destination.value}:${checkRef}`,
      ]);
      const fetchedRevision = fetched.ok
        ? git(input.worktree, ["rev-parse", checkRef])
        : fetched;
      if (!fetchedRevision.ok || fetchedRevision.value !== before.value.value)
        return failure(
          "TIBER_DELIVERY_OBSERVATION_INVALID",
          "remote destination changed during revalidation",
        );
      const ancestry = git(input.worktree, [
        "merge-base",
        "--is-ancestor",
        fetchedRevision.value,
        commit.value,
      ]);
      const cleaned = git(input.worktree, ["update-ref", "-d", checkRef]);
      if (!cleaned.ok)
        return failure(
          "TIBER_DELIVERY_OBSERVATION_INVALID",
          "delivery revalidation reference was not cleaned",
        );
      if (!ancestry.ok)
        return failure(
          "TIBER_DELIVERY_NON_FAST_FORWARD",
          "remote destination requires revalidation",
        );
    }
    const pushed = git(input.worktree, [
      "push",
      "origin",
      `${commit.value}:${input.destination.value}`,
    ]);
    if (!pushed.ok)
      return failure(
        "TIBER_DELIVERY_PUSH_FAILED",
        "fast-forward delivery push failed",
      );
    const after = observeRemote(input.worktree, input.destination.value);
    if (!after.ok) return after;
    remoteCommit = after.value;
  }
  const receipt: GitDeliveryReceipt = {
    mode: input.mode,
    baselineRevision: input.baselineRevision,
    commit: commit.value,
    tree: treeDigest.value,
    sourceSnapshotDigest: input.sourceSnapshotDigest,
    destination: input.destination,
    observedRemoteCommit: remoteCommit,
  };
  return validateGitDeliveryReceipt(receipt).status === "authorized"
    ? { ok: true, value: receipt }
    : failure(
        "TIBER_DELIVERY_OBSERVATION_INVALID",
        "delivery receipt was not exact",
      );
}
