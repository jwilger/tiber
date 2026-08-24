import { randomUUID } from "node:crypto";

import {
  getAgentDir,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";

import {
  FileCampaignStore,
  type CampaignCheckpoint,
} from "../adapters/campaigns/file-campaign-store.js";
import {
  createAdHocCampaignTask,
  decideCampaignSchedule,
  mergeCampaignAttention,
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignCheckpointTime,
  parseCampaignConsumption,
  parseCampaignGoal,
  parseCampaignId,
  type CampaignCandidate,
  type CampaignEffect,
  type CampaignTaskId,
} from "../core/campaigns/campaign.js";
import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { none, some, type Option } from "../core/types/option.js";
import type {
  CampaignGoalTaskCreatedEvent,
  TaskClaimReleasedEvent,
} from "../core/tasks/task-board.js";
import {
  parseTaskDescription,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  parseTaskTitle,
} from "../core/tasks/task-values.js";
import { handleWorkCommand } from "./work-command.js";

function decode(value: string): unknown {
  try {
    return JSON.parse(Buffer.from(value, "base64url").toString("utf8"));
  } catch {
    return undefined;
  }
}

function notifyFailure(context: ExtensionContext, code: string): void {
  context.ui.notify(code, "error");
}

function start(argumentsText: string, context: ExtensionContext): void {
  const bounds = parseCampaignBounds(decode(argumentsText.trim()));
  const consumption = parseCampaignConsumption({
    startedTasks: 0,
    elapsedMs: 0,
    costMicros: 0,
    tokens: 0,
    activeTasks: 0,
    startedTaskIds: [],
    activeTaskIds: [],
    initiativeStarts: {},
  });
  if (!bounds.ok || !consumption.ok) {
    notifyFailure(context, "TIBER_CAMPAIGN_VALUE_INVALID");
    return;
  }
  const now = parseCampaignCheckpointTime(new Date().toISOString());
  const campaignId = parseCampaignId(randomUUID());
  if (!now.ok || !campaignId.ok) {
    notifyFailure(context, "TIBER_CAMPAIGN_VALUE_INVALID");
    return;
  }
  const store = new FileCampaignStore(getAgentDir(), context.cwd);
  const existing = store.read();
  if (
    !existing.ok ||
    (existing.value.kind === "some" && existing.value.value.status === "active")
  ) {
    notifyFailure(
      context,
      existing.ok ? "TIBER_CAMPAIGN_ALREADY_ACTIVE" : existing.failure.code,
    );
    return;
  }
  const checkpoint: CampaignCheckpoint = {
    schemaVersion: 1,
    campaignId: campaignId.value,
    repositoryPath: context.cwd,
    status: "active",
    startedAt: now.value,
    updatedAt: now.value,
    bounds: bounds.value,
    consumption: consumption.value,
    candidates: [],
    attention: [],
    reason: "no-eligible-work",
  };
  const stored = store.write(checkpoint);
  context.ui.notify(
    stored.ok
      ? `Campaign ${checkpoint.campaignId} started`
      : stored.failure.code,
    stored.ok ? "info" : "error",
  );
}

function parseTick(value: unknown): Option<{
  readonly candidates: readonly CampaignCandidate[];
}> {
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    !("candidates" in value) ||
    !Array.isArray(value.candidates)
  )
    return none;
  const candidates = value.candidates.map(parseCampaignCandidate);
  if (candidates.some((candidate) => !candidate.ok)) return none;
  return some({
    candidates: candidates.flatMap((candidate) =>
      candidate.ok ? [candidate.value] : [],
    ),
  });
}

function authoritativeCandidates(
  proposed: readonly CampaignCandidate[],
  retained: readonly CampaignCandidate[],
  context: ExtensionContext,
): Option<readonly CampaignCandidate[]> {
  const board = new GitTaskRemote(context.cwd).read();
  if (board.mode !== "writable") return none;
  const candidates: CampaignCandidate[] = [];
  for (const candidate of proposed) {
    const taskId = parseTaskId(candidate.taskId);
    if (!taskId.ok) return none;
    const rank = board.tasks.findIndex((task) => task.id === taskId.value);
    const task = board.tasks[rank];
    if (
      task === undefined ||
      (task.state !== "Ready" && task.state !== "In Progress")
    )
      return none;
    const pinned = retained.find((entry) => entry.taskId === candidate.taskId);
    if (
      pinned !== undefined &&
      (pinned.initiativeId !== candidate.initiativeId ||
        pinned.estimatedCostMicros !== candidate.estimatedCostMicros ||
        pinned.estimatedTokens !== candidate.estimatedTokens)
    )
      return none;
    let blockerPhase: CampaignCandidate["blockerPhase"] = "none";
    if (task.claim.kind === "some") {
      const run = new FileRunJournal(getAgentDir()).read(task.id);
      blockerPhase =
        run.ok &&
        run.value.kind === "some" &&
        (run.value.value.state === "red-accepted" ||
          run.value.value.state === "green-review-clean" ||
          run.value.value.state === "green-rework-required" ||
          run.value.value.state === "red-reinstated")
          ? "post-mutation"
          : "pre-mutation";
    }
    candidates.push({
      ...(pinned ?? candidate),
      rank,
      blockerPhase,
    });
  }
  return some(candidates);
}

function releaseDeferredClaim(
  taskIdValue: CampaignTaskId,
  context: ExtensionContext,
): void {
  const taskId = parseTaskId(taskIdValue);
  if (!taskId.ok) return;
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = board.tasks.find((candidate) => candidate.id === taskId.value);
  if (
    board.mode !== "writable" ||
    task?.claim.kind !== "some" ||
    task.specificationDigest.kind !== "some"
  )
    return;
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok) return;
  const event: TaskClaimReleasedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-claim-released",
    occurredAt: occurredAt.value,
    taskId: task.id,
    specificationDigest: task.specificationDigest.value,
    claimId: task.claim.value.claimId,
    reason: "released",
  };
  remote.publish(event);
}

async function interpretCampaignEffects(
  effects: readonly CampaignEffect[],
  context: ExtensionContext,
): Promise<void> {
  for (const effect of effects) {
    if (effect.kind === "start-task")
      await handleWorkCommand(effect.taskId, context);
    if (effect.kind === "release-and-defer")
      releaseDeferredClaim(effect.taskId, context);
  }
}

async function tick(
  argumentsText: string,
  context: ExtensionContext,
): Promise<void> {
  const store = new FileCampaignStore(getAgentDir(), context.cwd);
  const existing = store.read();
  const tickInput = parseTick(decode(argumentsText.trim()));
  if (
    !existing.ok ||
    existing.value.kind === "none" ||
    tickInput.kind === "none"
  ) {
    notifyFailure(
      context,
      existing.ok ? "TIBER_CAMPAIGN_NOT_ACTIVE" : existing.failure.code,
    );
    return;
  }
  const checkpoint = existing.value.value;
  const board = new GitTaskRemote(context.cwd).read();
  if (checkpoint.status !== "active" || board.mode !== "writable") {
    notifyFailure(context, "TIBER_CAMPAIGN_NOT_ACTIVE");
    return;
  }
  const completedTaskIds = checkpoint.consumption.activeTaskIds.filter(
    (taskId) => {
      const parsed = parseTaskId(taskId);
      return (
        parsed.ok &&
        board.tasks.some(
          (task) => task.id === parsed.value && task.state === "Done",
        )
      );
    },
  );
  const elapsed = Date.now() - Date.parse(checkpoint.startedAt);
  const consumption = parseCampaignConsumption({
    ...checkpoint.consumption,
    elapsedMs: Math.max(checkpoint.consumption.elapsedMs, elapsed),
    activeTasks: checkpoint.consumption.activeTasks - completedTaskIds.length,
    activeTaskIds: checkpoint.consumption.activeTaskIds.filter(
      (taskId) => !completedTaskIds.includes(taskId),
    ),
  });
  if (!consumption.ok) {
    notifyFailure(context, consumption.failure.code);
    return;
  }
  const candidates = authoritativeCandidates(
    tickInput.value.candidates,
    checkpoint.candidates,
    context,
  );
  if (candidates.kind === "none") {
    notifyFailure(context, "TIBER_CAMPAIGN_CANDIDATE_AUTHORITY_INVALID");
    return;
  }
  const decision = decideCampaignSchedule({
    bounds: checkpoint.bounds,
    consumption: consumption.value,
    candidates: candidates.value,
  });
  const candidateRegistry = [
    ...checkpoint.candidates.filter(
      (retained) =>
        !candidates.value.some(
          (candidate) => candidate.taskId === retained.taskId,
        ),
    ),
    ...candidates.value,
  ];
  const updatedAt = parseCampaignCheckpointTime(new Date().toISOString());
  if (!updatedAt.ok) {
    notifyFailure(context, updatedAt.failure.code);
    return;
  }
  const terminal =
    decision.checkpoint.reason === "task-bound" ||
    decision.checkpoint.reason === "time-bound" ||
    decision.checkpoint.reason === "cost-bound" ||
    decision.checkpoint.reason === "token-bound";
  const updated: CampaignCheckpoint = {
    ...checkpoint,
    status:
      terminal && decision.checkpoint.consumption.activeTasks === 0
        ? "completed"
        : "active",
    updatedAt: updatedAt.value,
    consumption: decision.checkpoint.consumption,
    candidates: candidateRegistry,
    attention: mergeCampaignAttention(checkpoint.attention, decision.attention),
    reason: decision.checkpoint.reason,
  };
  const written = store.write(updated);
  if (!written.ok) {
    notifyFailure(context, written.failure.code);
    return;
  }
  const effects = decision.effects
    .map((effect) => `${effect.kind}:${effect.taskId}`)
    .join(", ");
  await interpretCampaignEffects(decision.effects, context);
  context.ui.notify(
    `Campaign ${checkpoint.campaignId}: ${decision.checkpoint.reason}${effects.length === 0 ? "" : `\n${effects}`}`,
    "info",
  );
}

function createGoal(argumentsText: string, context: ExtensionContext): void {
  const goal = parseCampaignGoal(argumentsText.trim());
  if (!goal.ok) {
    notifyFailure(context, goal.failure.code);
    return;
  }
  const identifiers = {
    eventId: parseTaskEventId(randomUUID()),
    taskId: parseTaskId(randomUUID()),
    occurredAt: parseTaskEventOccurredAt(new Date().toISOString()),
  };
  const campaign = new FileCampaignStore(getAgentDir(), context.cwd).read();
  if (
    !campaign.ok ||
    campaign.value.kind === "none" ||
    campaign.value.value.status !== "active"
  ) {
    notifyFailure(
      context,
      campaign.ok ? "TIBER_CAMPAIGN_NOT_ACTIVE" : campaign.failure.code,
    );
    return;
  }
  const campaignId = campaign.value.value.campaignId;
  const proposal = createAdHocCampaignTask(goal.value, campaignId);
  const title = parseTaskTitle(proposal.title);
  const description = parseTaskDescription(
    `${proposal.description}\nProvenance: campaign-goal/${campaignId}`,
  );
  if (
    !identifiers.eventId.ok ||
    !identifiers.taskId.ok ||
    !identifiers.occurredAt.ok ||
    !title.ok ||
    !description.ok
  ) {
    notifyFailure(context, "TIBER_CAMPAIGN_VALUE_INVALID");
    return;
  }
  const event: CampaignGoalTaskCreatedEvent = {
    schemaVersion: 1,
    eventId: identifiers.eventId.value,
    kind: "task-campaign-goal-created",
    occurredAt: identifiers.occurredAt.value,
    campaignId,
    task: {
      id: identifiers.taskId.value,
      title: title.value,
      description: description.value,
    },
  };
  const board = new GitTaskRemote(context.cwd).publish(event);
  context.ui.notify(
    board.mode === "writable"
      ? `Campaign goal task created: ${event.task.id}`
      : "TIBER_CAMPAIGN_GOAL_NOT_PUBLISHED",
    board.mode === "writable" ? "info" : "error",
  );
}

export async function handleCampaignCommand(
  argumentsText: string,
  context: ExtensionContext,
): Promise<void> {
  const match = /^(start|tick|goal|status)\s*(.*)$/su.exec(
    argumentsText.trim(),
  );
  if (match?.[1] === "start") start(match[2] ?? "", context);
  else if (match?.[1] === "tick") await tick(match[2] ?? "", context);
  else if (match?.[1] === "goal") createGoal(match[2] ?? "", context);
  else if (match?.[1] === "status") {
    const current = new FileCampaignStore(getAgentDir(), context.cwd).read();
    context.ui.notify(
      !current.ok
        ? current.failure.code
        : current.value.kind === "none"
          ? "No campaign checkpoint"
          : `Campaign ${current.value.value.campaignId}: ${current.value.value.status}/${current.value.value.reason}\nAttention: ${String(current.value.value.attention.length)}`,
      !current.ok ? "error" : "info",
    );
  } else
    context.ui.notify(
      "Usage: /tiber:campaign start <bounds-base64url> | tick <input-base64url> | goal <title> | status",
      "error",
    );
  return Promise.resolve();
}

export function handleAttentionCommand(
  _argumentsText: string,
  context: ExtensionContext,
): Promise<void> {
  const current = new FileCampaignStore(getAgentDir(), context.cwd).read();
  context.ui.notify(
    !current.ok
      ? current.failure.code
      : current.value.kind === "none" ||
          current.value.value.attention.length === 0
        ? "No campaign attention items"
        : current.value.value.attention
            .map(
              (item) => `${item.kind} | ${item.taskId} | ${item.initiativeId}`,
            )
            .join("\n"),
    !current.ok ? "error" : "info",
  );
  return Promise.resolve();
}
