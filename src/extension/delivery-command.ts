import { randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { deliverGit } from "../adapters/git/git-delivery.js";
import { observeSourceSnapshot } from "../adapters/git/git-source-diff.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import {
  parseDeliveryCommitBody,
  parseDeliveryCommitSubject,
  parseDeliveryDestinationRef,
} from "../core/delivery/git-delivery-values.js";
import {
  authorizeGitDelivery,
  type GitDeliveryMode,
} from "../core/delivery/git-delivery.js";
import type { TaskDeliveryRecordedEvent } from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";
import { none, some } from "../core/types/option.js";

function mode(value: unknown): GitDeliveryMode | undefined {
  return value === "local-only" ||
    value === "branch-push" ||
    value === "direct" ||
    value === "review"
    ? value
    : undefined;
}

function runDeliveryCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): void {
  const match = /^(\S+)\s+(\S+)\s+(\S+)\s+(.+?)\s+--\s+(.+)$/su.exec(
    argumentsText.trim(),
  );
  if (match === null) {
    context.ui.notify(
      "Usage: /tiber:deliver <task-id> <mode> <destination-ref-or-> <subject> -- <body>",
      "info",
    );
    return;
  }
  const taskId = parseTaskId(match[1]);
  const deliveryMode = mode(match[2]);
  const destination =
    match[3] === "-" ? none : parseDeliveryDestinationRef(match[3]);
  const subject = parseDeliveryCommitSubject(match[4]);
  const body = parseDeliveryCommitBody(match[5]);
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    board.mode !== "writable" ||
    task?.state !== "In Progress" ||
    task.claim.kind !== "some" ||
    task.specificationDigest.kind !== "some" ||
    task.finalReviewProgress.kind !== "some" ||
    task.delivery.kind === "some" ||
    deliveryMode === undefined ||
    ("ok" in destination && !destination.ok) ||
    !subject.ok ||
    !body.ok
  ) {
    context.ui.notify("TIBER_DELIVERY_AUTHORITY_INVALID", "error");
    return;
  }
  const semanticDestination =
    "ok" in destination ? some(destination.value) : destination;
  const claim = task.claim.value;
  const owned = new GitOwnedWorktrees(context.cwd, getAgentDir()).read();
  const worktree = owned.ok
    ? owned.value.worktrees.find(
        (entry) => entry.taskId === task.id && entry.claimId === claim.claimId,
      )
    : undefined;
  if (worktree === undefined) {
    context.ui.notify("TIBER_DELIVERY_WORKTREE_REQUIRED", "error");
    return;
  }
  const snapshot = observeSourceSnapshot(worktree.path, claim.baselineRevision);
  if (!snapshot.ok) {
    context.ui.notify(snapshot.failure.code, "error");
    return;
  }
  const authorization = authorizeGitDelivery({
    mode: deliveryMode,
    destination: semanticDestination,
    reviewedProgress: task.finalReviewProgress.value,
    observedSourceSnapshot: snapshot.value,
  });
  if (authorization.status !== "authorized") {
    context.ui.notify(authorization.code, "error");
    return;
  }
  const delivered = deliverGit({
    worktree: worktree.path,
    baselineRevision: claim.baselineRevision,
    mode: deliveryMode,
    destination: semanticDestination,
    subject: subject.value,
    body: body.value,
    sourceSnapshotDigest: snapshot.value,
  });
  if (!delivered.ok) {
    context.ui.notify(delivered.failure.code, "error");
    return;
  }
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok) {
    context.ui.notify("TIBER_DELIVERY_RECEIPT_INVALID", "error");
    return;
  }
  const event: TaskDeliveryRecordedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-delivery-recorded",
    occurredAt: occurredAt.value,
    taskId: task.id,
    specificationDigest: task.specificationDigest.value,
    claimId: claim.claimId,
    receipt: delivered.value,
  };
  const published = remote.publish(event);
  if (
    published.mode !== "writable" ||
    published.tasks.find((candidate) => candidate.id === task.id)?.delivery
      .kind !== "some"
  ) {
    context.ui.notify("TIBER_DELIVERY_RECEIPT_NOT_PUBLISHED", "error");
    return;
  }
  context.ui.notify(
    `TIBER_DELIVERY_RECORDED: ${delivered.value.commit}`,
    "info",
  );
}

export function handleDeliveryCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  runDeliveryCommand(argumentsText, context);
  return Promise.resolve();
}
