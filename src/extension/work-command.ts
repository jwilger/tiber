import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import type {
  TaskClaimedEvent,
  TaskClaimReleasedEvent,
  TaskClaimTakenOverEvent,
} from "../core/tasks/task-board.js";
import {
  BUILT_IN_WORKFLOW,
  compileWorkflow,
} from "../core/workflow/workflow.js";

function git(cwd: string, arguments_: readonly string[]): string | undefined {
  try {
    return execFileSync("git", [...arguments_], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return undefined;
  }
}

function projectWorkflow(cwd: string): unknown {
  const path = join(cwd, ".tiber", "workflow.json");
  if (!existsSync(path)) return BUILT_IN_WORKFLOW;
  try {
    const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
    return parsed;
  } catch {
    return undefined;
  }
}

export async function handleWorkCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const remote = new GitTaskRemote(context.cwd);
  const takeoverMatch = /^takeover\s+(\S+)$/u.exec(argumentsText.trim());
  if (takeoverMatch !== null) {
    const taskId = takeoverMatch[1] ?? "";
    const board = remote.read();
    const task = board.tasks.find((candidate) => candidate.id === taskId);
    if (
      board.mode !== "writable" ||
      task?.state !== "In Progress" ||
      task.claim === undefined ||
      task.specificationDigest === undefined
    ) {
      context.ui.notify(
        "TIBER_TAKEOVER_DENIED: no exact active claim",
        "error",
      );
      return;
    }
    if (!context.hasUI) {
      context.ui.notify(
        "TIBER_TAKEOVER_HUMAN_REQUIRED: interactive confirmation required",
        "error",
      );
      return;
    }
    const phrase = `takeover ${task.id} ${task.claim.claimId}`;
    const confirmation = await context.ui.input(
      "Exact claim takeover",
      `Type: ${phrase}`,
    );
    if (confirmation !== phrase) {
      context.ui.notify(
        "TIBER_TAKEOVER_DENIED: exact confirmation did not match",
        "error",
      );
      return;
    }
    const owner = git(context.cwd, ["config", "user.email"]);
    if (owner === undefined) {
      context.ui.notify(
        "TIBER_WORK_GIT_IDENTITY_REQUIRED: owner is unavailable",
        "error",
      );
      return;
    }
    const claimId = randomUUID();
    const occurredAt = new Date().toISOString();
    const event: TaskClaimTakenOverEvent = {
      schemaVersion: 1,
      eventId: randomUUID(),
      kind: "task-claim-taken-over",
      occurredAt,
      taskId: task.id,
      specificationDigest: task.specificationDigest,
      previousClaimId: task.claim.claimId,
      claim: { ...task.claim, claimId, owner },
    };
    const published = remote.publish(event);
    if (
      published.mode !== "writable" ||
      published.tasks.find((candidate) => candidate.id === task.id)?.claim
        ?.claimId !== claimId
    ) {
      context.ui.notify(
        "TIBER_TAKEOVER_PUBLICATION_FAILED: takeover was not observed",
        "error",
      );
      return;
    }
    const transfer = new GitOwnedWorktrees(
      context.cwd,
      getAgentDir(),
    ).transferClaim({
      taskId: task.id,
      previousClaimId: task.claim.claimId,
      claimId,
      occurredAt,
    });
    if (!transfer.ok) {
      context.ui.notify(
        `${transfer.failure.code}: ${transfer.failure.message}`,
        "error",
      );
      return;
    }
    context.ui.notify(
      `Claim takeover published\nTask: ${task.id}\nClaim: ${claimId}`,
      "info",
    );
    return;
  }
  const taskId = argumentsText.trim();
  const board = remote.read();
  const task = board.tasks.find((candidate) => candidate.id === taskId);
  if (
    board.mode !== "writable" ||
    task?.state !== "Ready" ||
    task.specificationDigest === undefined
  ) {
    context.ui.notify(
      "TIBER_WORK_NOT_READY: task must be unclaimed, verified, and Ready",
      "error",
    );
    return Promise.resolve();
  }
  const workflow = compileWorkflow(projectWorkflow(context.cwd));
  if (!workflow.ok) {
    context.ui.notify(
      `${workflow.failure.code}: ${workflow.failure.message}`,
      "error",
    );
    return Promise.resolve();
  }
  const baselineRevision = git(context.cwd, ["rev-parse", "HEAD"]);
  const owner = git(context.cwd, ["config", "user.email"]);
  if (baselineRevision === undefined || owner === undefined) {
    context.ui.notify(
      "TIBER_WORK_GIT_IDENTITY_REQUIRED: baseline and owner are unavailable",
      "error",
    );
    return Promise.resolve();
  }
  const claimId = randomUUID();
  const journal = new FileRunJournal(getAgentDir());
  const baseRecord = {
    schemaVersion: 1,
    taskId: task.id,
    claimId,
    baselineRevision,
    workflowDigest: workflow.value.digest,
  } as const;
  if (!journal.write({ ...baseRecord, state: "claim-intent" })) {
    context.ui.notify(
      "TIBER_RUN_JOURNAL_FAILED: claim intent was not durable",
      "error",
    );
    return Promise.resolve();
  }
  const event: TaskClaimedEvent = {
    schemaVersion: 1,
    eventId: randomUUID(),
    kind: "task-claimed",
    occurredAt: new Date().toISOString(),
    taskId: task.id,
    specificationDigest: task.specificationDigest,
    claim: {
      claimId,
      owner,
      baselineRevision,
      workflowDigest: workflow.value.digest,
    },
  };
  const claimed = remote.publish(event);
  const published = claimed.tasks.find(
    (candidate) => candidate.id === task.id,
  )?.claim;
  if (claimed.mode !== "writable" || published?.claimId !== claimId) {
    context.ui.notify(
      "TIBER_CLAIM_PUBLICATION_FAILED: exclusive claim was not observed",
      "error",
    );
    return Promise.resolve();
  }
  const currentRevision = git(context.cwd, ["rev-parse", "HEAD"]);
  if (currentRevision !== baselineRevision) {
    const release: TaskClaimReleasedEvent = {
      schemaVersion: 1,
      eventId: randomUUID(),
      kind: "task-claim-released",
      occurredAt: new Date().toISOString(),
      taskId: task.id,
      specificationDigest: task.specificationDigest,
      claimId,
      reason: "baseline-drift",
    };
    remote.publish(release);
    journal.write({ ...baseRecord, state: "blocked-baseline-drift" });
    context.ui.notify(
      "TIBER_BASELINE_DRIFT: claim released and Ready rank preserved",
      "error",
    );
    return Promise.resolve();
  }
  const worktree = new GitOwnedWorktrees(context.cwd, getAgentDir()).create({
    taskId: task.id,
    claimId,
    baselineRevision,
    occurredAt: new Date().toISOString(),
  });
  if (!worktree.ok) {
    const release: TaskClaimReleasedEvent = {
      schemaVersion: 1,
      eventId: randomUUID(),
      kind: "task-claim-released",
      occurredAt: new Date().toISOString(),
      taskId: task.id,
      specificationDigest: task.specificationDigest,
      claimId,
      reason: "released",
    };
    remote.publish(release);
    journal.write({ ...baseRecord, state: "blocked-worktree" });
    context.ui.notify(
      `${worktree.failure.code}: ${worktree.failure.message}; claim released`,
      "error",
    );
    return Promise.resolve();
  }
  if (
    !journal.write({
      ...baseRecord,
      state: "active",
      worktreePath: worktree.value.path,
    })
  ) {
    context.ui.notify(
      "TIBER_RUN_JOURNAL_FAILED: active worktree receipt was not durable",
      "error",
    );
    return Promise.resolve();
  }
  context.ui.notify(
    `Tiber work started\nTask: ${task.id}\nClaim: ${claimId}\nBaseline: ${baselineRevision}\nWorkflow: ${workflow.value.digest}\nWorktree: ${worktree.value.path}`,
    "info",
  );
  return Promise.resolve();
}
