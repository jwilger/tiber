import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import type {
  TaskClaimedEvent,
  TaskClaimReleasedEvent,
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

export function handleWorkCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const taskId = argumentsText.trim();
  const remote = new GitTaskRemote(context.cwd);
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
  journal.write({ ...baseRecord, state: "active" });
  context.ui.notify(
    `Tiber work started\nTask: ${task.id}\nClaim: ${claimId}\nBaseline: ${baselineRevision}\nWorkflow: ${workflow.value.digest}`,
    "info",
  );
  return Promise.resolve();
}
