import { randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { observeSourceSnapshot } from "../adapters/git/git-source-diff.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import type {
  TaskClaimReleasedEvent,
  TaskCompletedEvent,
} from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";
import { none, some, type Option } from "../core/types/option.js";
import { decideReviewedCompletion } from "../core/workflow/final-review.js";
import { parseWorktreeAbandonedAt } from "../core/worktrees/worktree-values.js";

function coordinates(): Option<{
  readonly eventId: TaskClaimReleasedEvent["eventId"];
  readonly occurredAt: TaskClaimReleasedEvent["occurredAt"];
}> {
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  return eventId.ok && occurredAt.ok
    ? some({ eventId: eventId.value, occurredAt: occurredAt.value })
    : none;
}

function runDoneCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): void {
  const taskId = parseTaskId(argumentsText.trim());
  if (!taskId.ok) {
    context.ui.notify("Usage: /tiber:done <task-id>", "info");
    return;
  }
  const remote = new GitTaskRemote(context.cwd);
  let board = remote.read();
  let task = board.tasks.find((candidate) => candidate.id === taskId.value);
  if (
    board.mode !== "writable" ||
    task === undefined ||
    task.specificationDigest.kind === "none" ||
    task.finalReviewProgress.kind === "none" ||
    task.finalReviewProgress.value.cleanStreak !== 3 ||
    (task.claim.kind === "none" && task.completionRelease.kind === "none")
  ) {
    context.ui.notify("TIBER_DONE_AUTHORITY_INVALID", "error");
    return;
  }
  const specificationDigest = task.specificationDigest.value;
  const sourceSnapshotDigest =
    task.finalReviewProgress.value.sourceSnapshotDigest;
  const claimId =
    task.claim.kind === "some"
      ? task.claim.value.claimId
      : task.completionRelease.kind === "some"
        ? task.completionRelease.value
        : undefined;
  if (claimId === undefined) {
    context.ui.notify("TIBER_DONE_AUTHORITY_INVALID", "error");
    return;
  }
  const worktrees = new GitOwnedWorktrees(context.cwd, getAgentDir());
  const registry = worktrees.read();
  if (!registry.ok) {
    context.ui.notify(registry.failure.code, "error");
    return;
  }
  const ownedEntry = registry.value.worktrees.find(
    (entry) => entry.taskId === taskId.value && entry.claimId === claimId,
  );
  if (ownedEntry !== undefined) {
    const observedSnapshot = observeSourceSnapshot(
      ownedEntry.path,
      ownedEntry.baselineRevision,
    );
    if (!observedSnapshot.ok) {
      context.ui.notify(observedSnapshot.failure.code, "error");
      return;
    }
    if (
      decideReviewedCompletion(
        task.finalReviewProgress.value,
        observedSnapshot.value,
      ).status !== "authorized"
    ) {
      context.ui.notify("TIBER_DONE_SOURCE_DELTA_REVIEW_REQUIRED", "error");
      return;
    }
  } else if (task.claim.kind === "some") {
    context.ui.notify("TIBER_DONE_WORKTREE_REQUIRED", "error");
    return;
  }
  const processes = new FileProcessGroupRegistry(getAgentDir()).terminateTask(
    task.id,
    claimId,
  );
  if (!processes.ok) {
    context.ui.notify(processes.failure.code, "error");
    return;
  }
  if (task.claim.kind === "some") {
    const releaseCoordinates = coordinates();
    if (releaseCoordinates.kind === "none") {
      context.ui.notify("TIBER_DONE_RECEIPT_INVALID", "error");
      return;
    }
    const release: TaskClaimReleasedEvent = {
      schemaVersion: 1,
      eventId: releaseCoordinates.value.eventId,
      kind: "task-claim-released",
      occurredAt: releaseCoordinates.value.occurredAt,
      taskId: task.id,
      specificationDigest,
      claimId,
      reason: "completed",
    };
    board = remote.publish(release);
    task = board.tasks.find((candidate) => candidate.id === taskId.value);
    if (
      board.mode !== "writable" ||
      task?.completionRelease.kind !== "some" ||
      task.completionRelease.value !== claimId
    ) {
      context.ui.notify("TIBER_DONE_RELEASE_NOT_PUBLISHED", "error");
      return;
    }
  }
  const owned = registry.value.worktrees.some(
    (entry) => entry.taskId === taskId.value && entry.claimId === claimId,
  );
  if (owned) {
    const abandonedAt = parseWorktreeAbandonedAt(new Date().toISOString());
    if (!abandonedAt.ok) {
      context.ui.notify(abandonedAt.failure.code, "error");
      return;
    }
    const cleaned = worktrees.abandon({
      taskId: taskId.value,
      claimStatus: "released",
      timestamp: abandonedAt.value,
    });
    if (!cleaned.ok) {
      context.ui.notify(cleaned.failure.code, "error");
      return;
    }
  }
  const completionCoordinates = coordinates();
  if (completionCoordinates.kind === "none") {
    context.ui.notify("TIBER_DONE_RECEIPT_INVALID", "error");
    return;
  }
  const completed: TaskCompletedEvent = {
    schemaVersion: 1,
    eventId: completionCoordinates.value.eventId,
    kind: "task-completed",
    occurredAt: completionCoordinates.value.occurredAt,
    taskId: taskId.value,
    specificationDigest,
    claimId,
    sourceSnapshotDigest,
    cleanup: {
      processCleanupStatus: "clean",
      worktreeCleanupStatus: "clean",
    },
  };
  board = remote.publish(completed);
  if (
    board.mode !== "writable" ||
    board.tasks.find((candidate) => candidate.id === taskId.value)?.state !==
      "Done"
  ) {
    context.ui.notify("TIBER_DONE_RECEIPT_NOT_PUBLISHED", "error");
    return;
  }
  context.ui.notify("TIBER_TASK_DONE", "info");
}

export function handleDoneCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  runDoneCommand(argumentsText, context);
  return Promise.resolve();
}
