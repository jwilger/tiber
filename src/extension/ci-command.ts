import { randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileCiAuthorityStore } from "../adapters/ci/file-ci-authority-store.js";
import { observeGithubActionsAuthority } from "../adapters/ci/github-actions-ci-authority.js";
import { observeCiAuthority } from "../adapters/ci/user-local-ci-authority.js";
import { GhGitHubHttpClient } from "../adapters/github/gh-github-http-client.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import {
  decideCiEvaluation,
  type CiSuccessReceipt,
} from "../core/ci/ci-authority.js";
import { parseCiRevision } from "../core/ci/ci-values.js";
import type { TaskCiRecordedEvent } from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  type SpecificationDigest,
  type TaskClaimId,
  type TaskId,
} from "../core/tasks/task-values.js";

async function evaluate(
  taskText: string,
  context: ExtensionCommandContext,
): Promise<
  | {
      readonly store: FileCiAuthorityStore;
      readonly decision: ReturnType<typeof decideCiEvaluation>;
      readonly remote: GitTaskRemote;
      readonly taskId: TaskId;
      readonly claimId: TaskClaimId;
      readonly specificationDigest: SpecificationDigest;
    }
  | undefined
> {
  const taskId = parseTaskId(taskText);
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    board.mode !== "writable" ||
    task?.state !== "In Progress" ||
    task.delivery.kind !== "some" ||
    task.claim.kind !== "some" ||
    task.specificationDigest.kind !== "some"
  ) {
    context.ui.notify("TIBER_CI_DELIVERY_REQUIRED", "error");
    return undefined;
  }
  if (task.ci.kind === "some") {
    context.ui.notify("TIBER_CI_ALREADY_RECORDED", "info");
    return undefined;
  }
  const revision = parseCiRevision(task.delivery.value.commit);
  if (!revision.ok) {
    context.ui.notify(revision.failure.code, "error");
    return undefined;
  }
  const store = new FileCiAuthorityStore(context.cwd, getAgentDir());
  const catalog = store.loadCatalog();
  if (!catalog.ok) {
    context.ui.notify(catalog.failure.code, "error");
    return undefined;
  }
  const github = new GhGitHubHttpClient();
  const observations = await Promise.all(
    catalog.value.authorities.map((authority) =>
      "kind" in authority
        ? observeGithubActionsAuthority(authority, revision.value, github)
        : Promise.resolve(observeCiAuthority(authority, revision.value)),
    ),
  );
  const failed = observations.find((observation) => !observation.ok);
  if (failed !== undefined) {
    context.ui.notify(failed.failure.code, "error");
    return undefined;
  }
  return {
    store,
    remote,
    taskId: task.id,
    claimId: task.claim.value.claimId,
    specificationDigest: task.specificationDigest.value,
    decision: decideCiEvaluation(
      revision.value,
      catalog.value.authorities.map(({ name }) => name),
      observations.flatMap((observation) =>
        observation.ok ? [observation.value] : [],
      ),
    ),
  };
}

function publishCiReceipt(
  result: NonNullable<Awaited<ReturnType<typeof evaluate>>>,
  receipt: CiSuccessReceipt,
): boolean {
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok) return false;
  const event: TaskCiRecordedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-ci-recorded",
    occurredAt: occurredAt.value,
    taskId: result.taskId,
    specificationDigest: result.specificationDigest,
    claimId: result.claimId,
    receipt,
  };
  const published = result.remote.publish(event);
  return (
    published.mode === "writable" &&
    published.tasks.find((task) => task.id === result.taskId)?.ci.kind ===
      "some"
  );
}

export async function handleCiCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const [taskText, ...diagnosisParts] = argumentsText.trim().split(/\s+/u);
  if (taskText === undefined || taskText.length === 0) {
    context.ui.notify(
      "Usage: /tiber:ci <task-id> [--recover <causal-diagnosis>]",
      "info",
    );
    return Promise.resolve();
  }
  const result = await evaluate(taskText, context);
  if (result === undefined) return Promise.resolve();
  const { decision, store } = result;
  if (diagnosisParts[0] === "--recover") {
    const diagnosis = store.parseDiagnosis(diagnosisParts.slice(1).join(" "));
    if (!diagnosis.ok || decision.status !== "succeeded") {
      context.ui.notify(
        diagnosis.ok
          ? "TIBER_CI_RECOVERY_SUCCESS_REQUIRED"
          : diagnosis.failure.code,
        "error",
      );
      return Promise.resolve();
    }
    const recovered = store.recoverHold(diagnosis.value, decision.receipt);
    const published =
      recovered.ok && publishCiReceipt(result, decision.receipt);
    if (recovered.ok) context.ui.setStatus("tiber", "Tiber: CI hold recovered");
    context.ui.notify(
      published
        ? "TIBER_CI_HOLD_RECOVERED"
        : recovered.ok
          ? "TIBER_CI_RECEIPT_NOT_PUBLISHED"
          : recovered.failure.code,
      published ? "info" : "error",
    );
    return Promise.resolve();
  }
  if (diagnosisParts.length > 0) {
    context.ui.notify("TIBER_CI_ARGUMENTS_INVALID", "error");
    return Promise.resolve();
  }
  if (decision.status === "failed") {
    const recorded = store.recordHold(decision.hold);
    if (recorded.ok) context.ui.setStatus("tiber", "Tiber: CI delivery hold");
    context.ui.notify(
      recorded.ok ? decision.code : recorded.failure.code,
      "error",
    );
  } else if (decision.status === "succeeded") {
    const recorded = store.recordSuccess(decision.receipt);
    const published = recorded.ok && publishCiReceipt(result, decision.receipt);
    context.ui.notify(
      published
        ? `TIBER_CI_SUCCEEDED: ${decision.receipt.revision}`
        : recorded.ok
          ? "TIBER_CI_RECEIPT_NOT_PUBLISHED"
          : recorded.failure.code,
      published ? "info" : "error",
    );
  } else {
    context.ui.notify(
      decision.code,
      decision.status === "waiting" ? "info" : "error",
    );
  }
  return Promise.resolve();
}
