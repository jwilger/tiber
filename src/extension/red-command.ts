import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileArtifactStore } from "../adapters/artifacts/file-artifact-store.js";
import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../adapters/commands/structured-command-runner.js";
import { reviewRedObservation } from "../adapters/models/pi-red-reviewer.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import { parseInlineOutputMaximumBytes } from "../core/artifacts/artifact-values.js";
import { virtualizeCommandOutput } from "../core/artifacts/output-virtualization.js";
import { parseScenarioName, parseTaskId } from "../core/tasks/task-values.js";
import { some } from "../core/types/option.js";
import { parseCommandName } from "../core/commands/command-values.js";
import { decideCommandExecution } from "../core/commands/structured-command.js";
import {
  decideRedAcceptance,
  projectScenarioFeature,
  type RedObservation,
} from "../core/workflow/semantic-red.js";
import { parseRedDiagnosticDigest } from "../core/workflow/workflow-values.js";

export async function handleRedCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const match = /^(\S+)\s+(\S+)\s+(.+)$/u.exec(argumentsText.trim());
  if (match === null) {
    context.ui.notify(
      "Usage: /tiber:red <task-id> <test-command> <exact-scenario-name>",
      "info",
    );
    return;
  }
  const taskId = parseTaskId(match[1]);
  const commandName = parseCommandName(match[2]);
  const scenarioName = parseScenarioName(match[3]);
  const board = new GitTaskRemote(context.cwd).read();
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
    context.ui.notify(
      "TIBER_RED_AUTHORITY_INVALID: exact claim and one mapped test are required",
      "error",
    );
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
  const journal = new FileRunJournal(getAgentDir());
  const runResult = journal.read(task.id);
  if (
    worktree === undefined ||
    !runResult.ok ||
    runResult.value.kind === "none" ||
    runResult.value.value.claimId !== claim.claimId ||
    runResult.value.value.baselineRevision !== claim.baselineRevision
  ) {
    context.ui.notify(
      "TIBER_RED_RUN_INVALID: durable run and owned worktree do not match the claim",
      "error",
    );
    return;
  }
  const projected = projectScenarioFeature(specification, scenarioName.value);
  if (!projected.ok) {
    context.ui.notify(projected.code, "error");
    return;
  }
  const featurePath = join(
    worktree.path,
    ".tiber",
    "features",
    `${task.id}-${createHash("sha256").update(scenarioName.value).digest("hex").slice(0, 12)}.feature`,
  );
  try {
    mkdirSync(dirname(featurePath), { recursive: true, mode: 0o700 });
    writeFileSync(featurePath, projected.feature, {
      encoding: "utf8",
      mode: 0o600,
      flag: "w",
    });
  } catch {
    context.ui.notify("TIBER_FEATURE_PROJECTION_FAILED", "error");
    return;
  }
  const run = runResult.value.value;

  const authority = new FileCommandAuthority(context.cwd);
  const catalog = authority.loadCatalog();
  if (!catalog.ok) {
    context.ui.notify(
      `${catalog.failure.code}: ${catalog.failure.message}`,
      "error",
    );
    return;
  }
  const grant = authority.readGrant();
  if (!grant.ok) {
    context.ui.notify(grant.failure.code, "error");
    return;
  }
  const command = decideCommandExecution(catalog.value, commandName.value, {
    claimStatus: "published",
    grantedCatalogDigest: grant.value,
  });
  if (!command.ok || command.command.purpose !== "test") {
    context.ui.notify("TIBER_RED_TEST_COMMAND_REQUIRED", "error");
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
    context.ui.notify(
      `${executed.failure.code}: ${executed.failure.message}`,
      "error",
    );
    return;
  }
  const inlineLimit = parseInlineOutputMaximumBytes(1);
  if (!inlineLimit.ok) {
    context.ui.notify(inlineLimit.failure.code, "error");
    return;
  }
  const diagnostic = virtualizeCommandOutput(
    executed.output,
    inlineLimit.value,
  );
  if (diagnostic.kind !== "artifact") {
    context.ui.notify("TIBER_RED_DIAGNOSTIC_EMPTY", "error");
    return;
  }
  const stored = new FileArtifactStore(getAgentDir()).put(diagnostic);
  if (!stored.ok || diagnostic.byteLength > 65_536) {
    context.ui.notify("TIBER_RED_DIAGNOSTIC_UNAVAILABLE", "error");
    return;
  }
  const testMapping = specification.testMappings[0];
  const diagnosticDigest = parseRedDiagnosticDigest(diagnostic.digest);
  if (testMapping === undefined || !diagnosticDigest.ok) return;
  const observation: RedObservation = {
    schemaVersion: 1,
    taskId: task.id,
    specificationDigest,
    scenarioName: scenarioName.value,
    testMapping,
    baselineRevision: claim.baselineRevision,
    commandCatalogDigest: catalog.value.digest,
    commandName: commandName.value,
    exitCode: executed.output.exitCode,
    diagnosticDigest: diagnosticDigest.value,
  };
  const review = await reviewRedObservation(
    worktree.path,
    specification,
    observation,
    diagnostic.content,
  );
  if (!review.ok) {
    context.ui.notify("TIBER_RED_REVIEW_INVALID", "error");
    return;
  }
  const decision = decideRedAcceptance(
    specification,
    observation,
    review.value,
    {
      taskId: task.id,
      specificationDigest,
      baselineRevision: claim.baselineRevision,
      commandCatalogDigest: catalog.value.digest,
    },
  );
  if (!decision.accepted) {
    context.ui.notify(decision.code, "error");
    return;
  }
  const journalReceipt = journal.write({
    ...run,
    state: "red-accepted",
    worktreePath: some(worktree.path),
    redReceipt: some({
      scenarioName: decision.receipt.scenarioName,
      testMapping: decision.receipt.testMapping,
      diagnosticDigest: decision.receipt.diagnosticDigest,
      missingPublicSurface: decision.receipt.missingPublicSurface,
    }),
  });
  if (!journalReceipt.ok) {
    context.ui.notify("TIBER_RED_RECEIPT_FAILED", "error");
    return;
  }
  context.ui.notify(
    `RED accepted\nScenario: ${scenarioName.value}\nDiagnostic: ${diagnostic.digest}`,
    "info",
  );
}
