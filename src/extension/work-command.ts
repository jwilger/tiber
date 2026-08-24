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
  parseClaimBaselineRevision,
  parseClaimOwnerIdentity,
  parseTaskClaimId,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";
import { none, some } from "../core/types/option.js";
import { parseCompiledWorkflowDigest } from "../core/workflow/workflow-values.js";
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

function newTaskEventCoordinates():
  | {
      readonly eventId: ReturnType<typeof parseTaskEventId> & {
        readonly ok: true;
      };
      readonly occurredAt: ReturnType<typeof parseTaskEventOccurredAt> & {
        readonly ok: true;
      };
    }
  | undefined {
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  return eventId.ok && occurredAt.ok ? { eventId, occurredAt } : undefined;
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
    const taskId = parseTaskId(takeoverMatch[1]);
    const board = remote.read();
    const task = taskId.ok
      ? board.tasks.find((candidate) => candidate.id === taskId.value)
      : undefined;
    if (
      board.mode !== "writable" ||
      task?.state !== "In Progress" ||
      task.claim.kind !== "some" ||
      task.specificationDigest.kind !== "some"
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
    const activeClaim = task.claim.value;
    const specificationDigest = task.specificationDigest.value;
    const phrase = `takeover ${task.id} ${activeClaim.claimId}`;
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
    const claimId = parseTaskClaimId(randomUUID());
    const claimOwner = parseClaimOwnerIdentity(owner);
    const coordinates = newTaskEventCoordinates();
    if (!claimId.ok || !claimOwner.ok || coordinates === undefined) {
      context.ui.notify(
        "TIBER_TASK_VALUE_INVALID: takeover values are invalid",
        "error",
      );
      return;
    }
    const event: TaskClaimTakenOverEvent = {
      schemaVersion: 1,
      eventId: coordinates.eventId.value,
      kind: "task-claim-taken-over",
      occurredAt: coordinates.occurredAt.value,
      taskId: task.id,
      specificationDigest,
      previousClaimId: activeClaim.claimId,
      claim: {
        ...activeClaim,
        claimId: claimId.value,
        owner: claimOwner.value,
      },
    };
    const published = remote.publish(event);
    const observedClaim = published.tasks.find(
      (candidate) => candidate.id === task.id,
    )?.claim;
    if (
      published.mode !== "writable" ||
      observedClaim?.kind !== "some" ||
      observedClaim.value.claimId !== claimId.value
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
      previousClaimId: activeClaim.claimId,
      claimId: claimId.value,
      occurredAt: coordinates.occurredAt.value,
    });
    if (!transfer.ok) {
      context.ui.notify(
        `${transfer.failure.code}: ${transfer.failure.message}`,
        "error",
      );
      return;
    }
    context.ui.notify(
      `Claim takeover published\nTask: ${task.id}\nClaim: ${claimId.value}`,
      "info",
    );
    return;
  }
  const taskId = parseTaskId(argumentsText.trim());
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    board.mode !== "writable" ||
    task?.state !== "Ready" ||
    task.specificationDigest.kind !== "some"
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
  const claimId = parseTaskClaimId(randomUUID());
  const claimBaseline = parseClaimBaselineRevision(baselineRevision);
  const claimOwner = parseClaimOwnerIdentity(owner);
  const workflowDigest = parseCompiledWorkflowDigest(workflow.value.digest);
  const claimCoordinates = newTaskEventCoordinates();
  if (
    !claimId.ok ||
    !claimBaseline.ok ||
    !claimOwner.ok ||
    !workflowDigest.ok ||
    claimCoordinates === undefined
  ) {
    context.ui.notify(
      "TIBER_TASK_VALUE_INVALID: claim values are invalid",
      "error",
    );
    return Promise.resolve();
  }
  const journal = new FileRunJournal(getAgentDir());
  const baseRecord = {
    schemaVersion: 1,
    taskId: task.id,
    claimId: claimId.value,
    baselineRevision: claimBaseline.value,
    workflowDigest: workflowDigest.value,
    worktreePath: none,
    redReceipt: none,
  } as const;
  if (!journal.write({ ...baseRecord, state: "claim-intent" }).ok) {
    context.ui.notify(
      "TIBER_RUN_JOURNAL_FAILED: claim intent was not durable",
      "error",
    );
    return Promise.resolve();
  }
  const event: TaskClaimedEvent = {
    schemaVersion: 1,
    eventId: claimCoordinates.eventId.value,
    kind: "task-claimed",
    occurredAt: claimCoordinates.occurredAt.value,
    taskId: task.id,
    specificationDigest: task.specificationDigest.value,
    claim: {
      claimId: claimId.value,
      owner: claimOwner.value,
      baselineRevision: claimBaseline.value,
      workflowDigest: workflowDigest.value,
    },
  };
  const claimed = remote.publish(event);
  const published = claimed.tasks.find(
    (candidate) => candidate.id === task.id,
  )?.claim;
  if (
    claimed.mode !== "writable" ||
    published?.kind !== "some" ||
    published.value.claimId !== claimId.value
  ) {
    context.ui.notify(
      "TIBER_CLAIM_PUBLICATION_FAILED: exclusive claim was not observed",
      "error",
    );
    return Promise.resolve();
  }
  const currentRevision = git(context.cwd, ["rev-parse", "HEAD"]);
  if (currentRevision !== baselineRevision) {
    const releaseCoordinates = newTaskEventCoordinates();
    if (releaseCoordinates === undefined) {
      context.ui.notify(
        "TIBER_TASK_VALUE_INVALID: release event values are invalid",
        "error",
      );
      return Promise.resolve();
    }
    const release: TaskClaimReleasedEvent = {
      schemaVersion: 1,
      eventId: releaseCoordinates.eventId.value,
      kind: "task-claim-released",
      occurredAt: releaseCoordinates.occurredAt.value,
      taskId: task.id,
      specificationDigest: task.specificationDigest.value,
      claimId: claimId.value,
      reason: "baseline-drift",
    };
    remote.publish(release);
    if (!journal.write({ ...baseRecord, state: "blocked-baseline-drift" }).ok) {
      context.ui.notify("TIBER_RUN_JOURNAL_FAILED", "error");
      return Promise.resolve();
    }
    context.ui.notify(
      "TIBER_BASELINE_DRIFT: claim released and Ready rank preserved",
      "error",
    );
    return Promise.resolve();
  }
  const worktree = new GitOwnedWorktrees(context.cwd, getAgentDir()).create({
    taskId: task.id,
    claimId: claimId.value,
    baselineRevision: claimBaseline.value,
    occurredAt: claimCoordinates.occurredAt.value,
  });
  if (!worktree.ok) {
    const releaseCoordinates = newTaskEventCoordinates();
    if (releaseCoordinates === undefined) {
      context.ui.notify(
        "TIBER_TASK_VALUE_INVALID: release event values are invalid",
        "error",
      );
      return Promise.resolve();
    }
    const release: TaskClaimReleasedEvent = {
      schemaVersion: 1,
      eventId: releaseCoordinates.eventId.value,
      kind: "task-claim-released",
      occurredAt: releaseCoordinates.occurredAt.value,
      taskId: task.id,
      specificationDigest: task.specificationDigest.value,
      claimId: claimId.value,
      reason: "released",
    };
    remote.publish(release);
    if (!journal.write({ ...baseRecord, state: "blocked-worktree" }).ok) {
      context.ui.notify("TIBER_RUN_JOURNAL_FAILED", "error");
      return Promise.resolve();
    }
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
      worktreePath: some(worktree.value.path),
    }).ok
  ) {
    context.ui.notify(
      "TIBER_RUN_JOURNAL_FAILED: active worktree receipt was not durable",
      "error",
    );
    return Promise.resolve();
  }
  context.ui.notify(
    `Tiber work started\nTask: ${task.id}\nClaim: ${claimId.value}\nBaseline: ${claimBaseline.value}\nWorkflow: ${workflowDigest.value}\nWorktree: ${worktree.value.path}`,
    "info",
  );
  return Promise.resolve();
}
