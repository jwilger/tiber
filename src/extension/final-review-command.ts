import { createHash, randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../adapters/commands/structured-command-runner.js";
import { observeSourceDiff } from "../adapters/git/git-source-diff.js";
import { reviewFinalLens } from "../adapters/models/pi-final-reviewer.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import { parseCommandName } from "../core/commands/command-values.js";
import { decideCommandExecution } from "../core/commands/structured-command.js";
import type { TaskFinalReviewRecordedEvent } from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";
import {
  decideScopeCompletion,
  finalReviewRiskSignals,
  selectFinalReviewLenses,
} from "../core/workflow/final-review.js";
import {
  parseSourceSnapshotDigest,
  parseVerificationDiagnosticDigest,
} from "../core/workflow/workflow-values.js";

export async function handleFinalReviewCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const match = /^(\S+)\s+(\S+)$/u.exec(argumentsText.trim());
  if (match === null) {
    context.ui.notify(
      "Usage: /tiber:final-review <task-id> <verification-command>",
      "info",
    );
    return;
  }
  const taskId = parseTaskId(match[1]);
  const commandName = parseCommandName(match[2]);
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    board.mode !== "writable" ||
    task?.state !== "In Progress" ||
    task.claim.kind !== "some" ||
    task.specification.kind !== "some" ||
    task.specificationDigest.kind !== "some" ||
    !commandName.ok ||
    decideScopeCompletion(task.specification.value, task.preservedIncrements)
      .status !== "complete"
  ) {
    context.ui.notify("TIBER_FINAL_REVIEW_SCOPE_INCOMPLETE", "error");
    return;
  }
  const claim = task.claim.value;
  const specification = task.specification.value;
  const specificationDigest = task.specificationDigest.value;
  const worktrees = new GitOwnedWorktrees(context.cwd, getAgentDir()).read();
  const worktree = worktrees.ok
    ? worktrees.value.worktrees.find(
        (entry) => entry.taskId === task.id && entry.claimId === claim.claimId,
      )
    : undefined;
  const authority = new FileCommandAuthority(context.cwd);
  const catalog = authority.loadCatalog();
  const grant = authority.readGrant();
  if (worktree === undefined || !catalog.ok || !grant.ok) {
    context.ui.notify("TIBER_FINAL_REVIEW_AUTHORITY_INVALID", "error");
    return;
  }
  const command = decideCommandExecution(catalog.value, commandName.value, {
    claimStatus: "published",
    grantedCatalogDigest: grant.value,
  });
  if (!command.ok || command.command.purpose !== "verification") {
    context.ui.notify("TIBER_FULL_VERIFICATION_COMMAND_REQUIRED", "error");
    return;
  }
  const executed = await new StructuredCommandRunner(
    new FileProcessGroupRegistry(getAgentDir()),
  ).run(
    command.command,
    worktree.path,
    { taskId: task.id, claimId: claim.claimId },
    context.signal,
  );
  if (
    !executed.ok ||
    executed.output.exitCode.kind === "none" ||
    executed.output.exitCode.value !== 0
  ) {
    context.ui.notify(
      executed.ok ? "TIBER_FULL_VERIFICATION_FAILED" : executed.failure.code,
      "error",
    );
    return;
  }
  const diagnosticDigest = parseVerificationDiagnosticDigest(
    `sha256:${createHash("sha256")
      .update(`${executed.output.stdout}\n${executed.output.stderr}`)
      .digest("hex")}`,
  );
  const sourceDiff = observeSourceDiff(worktree.path, claim.baselineRevision);
  if (!diagnosticDigest.ok) {
    context.ui.notify(diagnosticDigest.failure.code, "error");
    return;
  }
  if (!sourceDiff.ok) {
    context.ui.notify(sourceDiff.failure.code, "error");
    return;
  }
  const sourceSnapshotDigest = parseSourceSnapshotDigest(
    `sha256:${createHash("sha256").update(sourceDiff.value).digest("hex")}`,
  );
  if (!sourceSnapshotDigest.ok) {
    context.ui.notify(sourceSnapshotDigest.failure.code, "error");
    return;
  }
  const selectedLenses = selectFinalReviewLenses(
    finalReviewRiskSignals(specification),
  );
  const reviews = [];
  for (const lens of selectedLenses) {
    const reviewed = await reviewFinalLens(
      worktree.path,
      specification,
      lens,
      sourceDiff.value,
      sourceSnapshotDigest.value,
      diagnosticDigest.value,
    );
    if (!reviewed.ok) {
      context.ui.notify(reviewed.failure.code, "error");
      return;
    }
    reviews.push(reviewed.value);
  }
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok) {
    context.ui.notify("TIBER_FINAL_REVIEW_RECEIPT_INVALID", "error");
    return;
  }
  const event: TaskFinalReviewRecordedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-final-review-recorded",
    occurredAt: occurredAt.value,
    taskId: task.id,
    specificationDigest,
    verification: {
      claimId: claim.claimId,
      specificationDigest,
      commandCatalogDigest: catalog.value.digest,
      diagnosticDigest: diagnosticDigest.value,
      sourceSnapshotDigest: sourceSnapshotDigest.value,
    },
    iteration: {
      sourceSnapshotDigest: sourceSnapshotDigest.value,
      verificationDiagnosticDigest: diagnosticDigest.value,
      selectedLenses,
      reviews,
    },
  };
  const published = remote.publish(event);
  const progress = published.tasks.find(
    (candidate) => candidate.id === task.id,
  )?.finalReviewProgress;
  if (published.mode !== "writable" || progress?.kind !== "some") {
    context.ui.notify("TIBER_FINAL_REVIEW_RECEIPT_NOT_PUBLISHED", "error");
    return;
  }
  context.ui.notify(
    progress.value.cleanStreak === 3
      ? "TIBER_FINAL_REVIEW_COMPLETE"
      : `TIBER_FINAL_REVIEW_STREAK_${String(progress.value.cleanStreak)}`,
    reviews.some((review) => review.findingCount !== 0) ? "error" : "info",
  );
}
