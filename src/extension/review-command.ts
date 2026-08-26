import { randomUUID } from "node:crypto";

import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import { GhGitHubHttpClient } from "../adapters/github/gh-github-http-client.js";
import {
  GitHubCiAdapter,
  GitHubMergeAdapter,
  GitHubPullRequestAdapter,
  GitHubReviewAdapter,
  parseGitHubCiCredential,
  parseGitHubMergeCredential,
  parseGitHubPullRequestCredential,
  parseGitHubReviewCredential,
} from "../adapters/github/github-review-service.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import {
  assembleReviewGateObservation,
  authorizeReviewAutoMerge,
  classifyReviewKind,
  type ReviewGateReceipt,
} from "../core/reviews/review-service.js";
import {
  parseReviewBaseRef,
  parseReviewBody,
  parseReviewHeadRef,
  parseReviewRepositoryName,
  parseReviewRepositoryOwner,
  parseReviewRevision,
  parseReviewTitle,
} from "../core/reviews/review-service-values.js";
import type {
  TaskReviewOpenedEvent,
  TaskReviewRecordedEvent,
} from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
} from "../core/tasks/task-values.js";

function eventCoordinates():
  | {
      readonly eventId: TaskReviewOpenedEvent["eventId"];
      readonly occurredAt: TaskReviewOpenedEvent["occurredAt"];
    }
  | undefined {
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  return eventId.ok && occurredAt.ok
    ? { eventId: eventId.value, occurredAt: occurredAt.value }
    : undefined;
}

async function openReview(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const match = /^(\S+)\s+(\S+)\s+(\S+)\s+(.+?)\s+--\s+(.+)$/su.exec(
    argumentsText.trim(),
  );
  if (match === null) {
    context.ui.notify(
      "Usage: /tiber:review open <task-id> <owner/repository> <base> <title> -- <body>",
      "info",
    );
    return;
  }
  const repositoryParts = match[2]?.split("/");
  const taskId = parseTaskId(match[1]);
  const owner = parseReviewRepositoryOwner(repositoryParts?.[0]);
  const repositoryName = parseReviewRepositoryName(repositoryParts?.[1]);
  const baseRef = parseReviewBaseRef(match[3]);
  const title = parseReviewTitle(match[4]);
  const body = parseReviewBody(match[5]);
  const credential = parseGitHubPullRequestCredential("host-gh");
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    repositoryParts?.length !== 2 ||
    !owner.ok ||
    !repositoryName.ok ||
    !baseRef.ok ||
    !title.ok ||
    !body.ok ||
    !credential.ok ||
    board.mode !== "writable" ||
    task?.state !== "In Progress" ||
    task.claim.kind !== "some" ||
    task.specificationDigest.kind !== "some" ||
    task.delivery.kind !== "some" ||
    task.delivery.value.mode !== "review" ||
    task.delivery.value.destination.kind !== "some" ||
    task.ci.kind !== "some" ||
    task.openedReview.kind === "some"
  ) {
    context.ui.notify("TIBER_REVIEW_OPEN_AUTHORITY_INVALID", "error");
    return;
  }
  const headRef = parseReviewHeadRef(task.delivery.value.destination.value);
  const headRevision = parseReviewRevision(task.delivery.value.commit);
  const coordinates = eventCoordinates();
  if (!headRef.ok || !headRevision.ok || coordinates === undefined) {
    context.ui.notify("TIBER_REVIEW_OPEN_AUTHORITY_INVALID", "error");
    return;
  }
  const request = {
    repositoryOwner: owner.value,
    repositoryName: repositoryName.value,
    headRef: headRef.value,
    headRevision: headRevision.value,
    baseRef: baseRef.value,
    title: title.value,
    body: body.value,
  };
  const opened = await new GitHubPullRequestAdapter(
    new GhGitHubHttpClient(),
    credential.value,
  ).create(request);
  if (!opened.ok) {
    context.ui.notify(opened.failure.code, "error");
    return;
  }
  const event: TaskReviewOpenedEvent = {
    schemaVersion: 1,
    ...coordinates,
    kind: "task-review-opened",
    taskId: task.id,
    specificationDigest: task.specificationDigest.value,
    claimId: task.claim.value.claimId,
    review: {
      kind: classifyReviewKind(request.headRef, request.title),
      request,
      pullRequest: opened.value,
    },
  };
  const published = remote.publish(event);
  const observed = published.tasks.find(
    (candidate) => candidate.id === task.id,
  );
  context.ui.notify(
    published.mode === "writable" && observed?.openedReview.kind === "some"
      ? `TIBER_REVIEW_OPENED: ${opened.value.url}`
      : "TIBER_REVIEW_RECEIPT_NOT_PUBLISHED",
    published.mode === "writable" && observed?.openedReview.kind === "some"
      ? "info"
      : "error",
  );
}

async function observeReview(
  taskText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const taskId = parseTaskId(taskText.trim());
  const reviewCredential = parseGitHubReviewCredential("host-gh");
  const ciCredential = parseGitHubCiCredential("host-gh");
  const mergeCredential = parseGitHubMergeCredential("host-gh");
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = taskId.ok
    ? board.tasks.find((candidate) => candidate.id === taskId.value)
    : undefined;
  if (
    !reviewCredential.ok ||
    !ciCredential.ok ||
    !mergeCredential.ok ||
    board.mode !== "writable" ||
    task?.state !== "In Progress" ||
    task.claim.kind !== "some" ||
    task.specificationDigest.kind !== "some" ||
    task.openedReview.kind !== "some" ||
    (task.reviewReceipt.kind === "some" &&
      task.reviewReceipt.value.disposition === "merged")
  ) {
    context.ui.notify("TIBER_REVIEW_OBSERVATION_AUTHORITY_INVALID", "error");
    return;
  }
  const opened = task.openedReview.value;
  const client = new GhGitHubHttpClient();
  const reviewAdapter = new GitHubReviewAdapter(client, reviewCredential.value);
  const ciAdapter = new GitHubCiAdapter(client, ciCredential.value);
  const mergeAdapter = new GitHubMergeAdapter(client, mergeCredential.value);
  const [review, ci, permission, merge] = await Promise.all([
    reviewAdapter.observe(opened.request, opened.pullRequest),
    ciAdapter.observe(opened.request),
    mergeAdapter.observeAuthorPermission(opened.request, opened.pullRequest),
    mergeAdapter.observeMerge(opened.request, opened.pullRequest),
  ]);
  if (!review.ok || !ci.ok || !permission.ok || !merge.ok) {
    context.ui.notify(
      !review.ok
        ? review.failure.code
        : !ci.ok
          ? ci.failure.code
          : !permission.ok
            ? permission.failure.code
            : merge.ok
              ? "TIBER_REVIEW_SERVICE_FAILED"
              : merge.failure.code,
      "error",
    );
    return;
  }
  const assembled = assembleReviewGateObservation({
    deliveredRevision: opened.request.headRevision,
    review: review.value,
    ci: ci.value,
    authorMergePermission: permission.value,
  });
  if (assembled.status !== "assembled") {
    context.ui.notify(assembled.code, "error");
    return;
  }
  const decision = authorizeReviewAutoMerge({
    kind: opened.kind,
    deliveredRevision: opened.request.headRevision,
    baseRef: opened.request.baseRef,
    observation: assembled.observation,
  });
  let receipt: ReviewGateReceipt | undefined;
  if (merge.value === "merged") {
    receipt = {
      observation: assembled.observation,
      mergeStatus: "merged",
      disposition: "merged",
    };
  } else if (decision.status === "authorized") {
    const enabled = await mergeAdapter.enableAutoMerge(
      opened.request,
      opened.pullRequest,
    );
    if (!enabled.ok) {
      context.ui.notify(enabled.failure.code, "error");
      return;
    }
    receipt = {
      observation: assembled.observation,
      mergeStatus: "open",
      disposition: "auto-merge-enabled",
    };
  } else if (decision.status === "human-required") {
    receipt = {
      observation: assembled.observation,
      mergeStatus: "open",
      disposition: "human-merge-required",
    };
  } else if (
    decision.status === "denied" &&
    decision.code === "TIBER_REVIEW_MERGE_PERMISSION_REQUIRED"
  ) {
    receipt = {
      observation: assembled.observation,
      mergeStatus: "open",
      disposition: "permission-missing",
    };
  } else {
    context.ui.notify(
      decision.code,
      decision.status === "waiting" ? "info" : "error",
    );
    return;
  }
  const coordinates = eventCoordinates();
  if (coordinates === undefined) {
    context.ui.notify("TIBER_REVIEW_RECEIPT_INVALID", "error");
    return;
  }
  const event: TaskReviewRecordedEvent = {
    schemaVersion: 1,
    ...coordinates,
    kind: "task-review-recorded",
    taskId: task.id,
    specificationDigest: task.specificationDigest.value,
    claimId: task.claim.value.claimId,
    pullRequestNumber: opened.pullRequest.number,
    receipt,
  };
  const published = remote.publish(event);
  const observed = published.tasks.find(
    (candidate) => candidate.id === task.id,
  );
  context.ui.notify(
    published.mode === "writable" && observed?.reviewReceipt.kind === "some"
      ? `TIBER_REVIEW_RECORDED: ${receipt.disposition}`
      : "TIBER_REVIEW_RECEIPT_NOT_PUBLISHED",
    published.mode === "writable" && observed?.reviewReceipt.kind === "some"
      ? "info"
      : "error",
  );
}

export function handleReviewCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const match = /^(open|observe)\s+(.+)$/su.exec(argumentsText.trim());
  if (match?.[1] === "open") return openReview(match[2] ?? "", context);
  if (match?.[1] === "observe") return observeReview(match[2] ?? "", context);
  context.ui.notify("Usage: /tiber:review <open|observe> ...", "info");
  return Promise.resolve();
}
