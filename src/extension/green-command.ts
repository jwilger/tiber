import { createHash, randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../adapters/commands/structured-command-runner.js";
import { reviewIncrement } from "../adapters/models/pi-increment-reviewer.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { observeSourceDiff } from "../adapters/git/git-source-diff.js";
import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import { parseCommandName } from "../core/commands/command-values.js";
import { decideCommandExecution } from "../core/commands/structured-command.js";
import type { TaskIncrementPreservedEvent } from "../core/tasks/task-board.js";
import {
  parseScenarioName,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";
import {
  decideGreenIncrement,
  type GreenObservation,
} from "../core/workflow/green-increment.js";
import {
  parseGreenDiagnosticDigest,
  parseSourceDiffDigest,
} from "../core/workflow/workflow-values.js";

export async function handleGreenCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const match = /^(\S+)\s+(\S+)\s+(.+)$/u.exec(argumentsText.trim());
  if (match === null) {
    context.ui.notify(
      "Usage: /tiber:green <task-id> <test-command> <exact-scenario-name>",
      "info",
    );
    return;
  }
  const taskId = parseTaskId(match[1]);
  const commandName = parseCommandName(match[2]);
  const scenarioName = parseScenarioName(match[3]);
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
    !scenarioName.ok ||
    task.specification.value.testMappings.length !== 1
  ) {
    context.ui.notify("TIBER_GREEN_AUTHORITY_INVALID", "error");
    return;
  }
  const claim = task.claim.value;
  const specification = task.specification.value;
  const specificationDigest = task.specificationDigest.value;
  const testMapping = specification.testMappings[0];
  const worktrees = new GitOwnedWorktrees(context.cwd, getAgentDir()).read();
  const worktree = worktrees.ok
    ? worktrees.value.worktrees.find(
        (entry) => entry.taskId === task.id && entry.claimId === claim.claimId,
      )
    : undefined;
  const journal = new FileRunJournal(getAgentDir());
  const runResult = journal.read(task.id);
  if (testMapping === undefined || worktree === undefined || !runResult.ok) {
    context.ui.notify("TIBER_GREEN_RED_RECEIPT_REQUIRED", "error");
    return;
  }
  const runOption = runResult.value;
  if (runOption.kind === "none") {
    context.ui.notify("TIBER_GREEN_RED_RECEIPT_REQUIRED", "error");
    return;
  }
  const run = runOption.value;
  if (
    run.redReceipt.kind === "none" ||
    run.redReceipt.value.scenarioName !== scenarioName.value ||
    run.redReceipt.value.testMapping !== testMapping ||
    run.redReceipt.value.specificationDigest !== specificationDigest ||
    run.claimId !== claim.claimId
  ) {
    context.ui.notify("TIBER_GREEN_RED_RECEIPT_REQUIRED", "error");
    return;
  }
  const redReceipt = run.redReceipt.value;
  const authority = new FileCommandAuthority(context.cwd);
  const catalog = authority.loadCatalog();
  const grant = authority.readGrant();
  if (
    !catalog.ok ||
    !grant.ok ||
    catalog.value.digest !== redReceipt.commandCatalogDigest
  ) {
    context.ui.notify("TIBER_GREEN_COMMAND_AUTHORITY_INVALID", "error");
    return;
  }
  const command = decideCommandExecution(catalog.value, commandName.value, {
    claimStatus: "published",
    grantedCatalogDigest: grant.value,
  });
  if (!command.ok || command.command.purpose !== "test") {
    context.ui.notify("TIBER_GREEN_TEST_COMMAND_REQUIRED", "error");
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
  if (!executed.ok) {
    context.ui.notify(executed.failure.code, "error");
    return;
  }
  const diagnosticText = `${executed.output.stdout}\n${executed.output.stderr}`;
  const diagnosticDigest = parseGreenDiagnosticDigest(
    `sha256:${createHash("sha256").update(diagnosticText).digest("hex")}`,
  );
  if (!diagnosticDigest.ok) {
    context.ui.notify(diagnosticDigest.failure.code, "error");
    return;
  }
  if (
    executed.output.exitCode.kind === "none" ||
    executed.output.exitCode.value !== 0
  ) {
    const receipt = journal.write({ ...run, state: "red-reinstated" });
    context.ui.notify(
      receipt.ok ? "TIBER_GREEN_NOT_OBSERVED" : receipt.failure.code,
      "error",
    );
    return;
  }
  const diff = observeSourceDiff(worktree.path, claim.baselineRevision);
  if (!diff.ok) {
    context.ui.notify(diff.failure.code, "error");
    return;
  }
  const diffDigest = parseSourceDiffDigest(
    `sha256:${createHash("sha256").update(diff.value).digest("hex")}`,
  );
  if (!diffDigest.ok) return;
  const observation: GreenObservation = {
    schemaVersion: 1,
    taskId: task.id,
    specificationDigest,
    baselineRevision: claim.baselineRevision,
    scenarioName: scenarioName.value,
    testMapping,
    commandCatalogDigest: catalog.value.digest,
    redDiagnosticDigest: redReceipt.diagnosticDigest,
    commandName: commandName.value,
    exitCode: executed.output.exitCode,
    diagnosticDigest: diagnosticDigest.value,
    sourceDiffDigest: diffDigest.value,
  };
  const review = await reviewIncrement(
    worktree.path,
    specification,
    scenarioName.value,
    diff.value,
    diffDigest.value,
  );
  if (!review.ok) {
    context.ui.notify(review.failure.code, "error");
    return;
  }
  const decision = decideGreenIncrement(
    {
      taskId: task.id,
      specificationDigest,
      baselineRevision: claim.baselineRevision,
      scenarioName: scenarioName.value,
      testMapping,
      redDiagnosticDigest: redReceipt.diagnosticDigest,
      commandCatalogDigest: redReceipt.commandCatalogDigest,
    },
    observation,
    review.value,
  );
  if (decision.state === "rework-required") {
    const receipt = journal.write({ ...run, state: "green-rework-required" });
    context.ui.notify(
      receipt.ok ? decision.code : receipt.failure.code,
      "error",
    );
    return;
  }
  if (decision.state !== "review-clean") {
    context.ui.notify(decision.code, "error");
    return;
  }
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok) {
    context.ui.notify("TIBER_GREEN_RECEIPT_INVALID", "error");
    return;
  }
  const event: TaskIncrementPreservedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-increment-preserved",
    occurredAt: occurredAt.value,
    taskId: task.id,
    specificationDigest,
    claimId: claim.claimId,
    increment: decision.receipt,
  };
  const published = remote.publish(event);
  const observed = published.tasks
    .find((candidate) => candidate.id === task.id)
    ?.preservedIncrements.some(
      (increment) =>
        increment.sourceDiffDigest === decision.receipt.sourceDiffDigest,
    );
  if (published.mode !== "writable" || observed !== true) {
    context.ui.notify("TIBER_GREEN_RECEIPT_NOT_PUBLISHED", "error");
    return;
  }
  const receipt = journal.write({ ...run, state: "green-review-clean" });
  if (!receipt.ok) {
    context.ui.notify(receipt.failure.code, "error");
    return;
  }
  context.ui.notify(
    `GREEN increment preserved\nScenario: ${scenarioName.value}\nDiff: ${decision.receipt.sourceDiffDigest}`,
    "info",
  );
}
