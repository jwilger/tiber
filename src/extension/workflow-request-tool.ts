import { randomUUID } from "node:crypto";

import type {
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type, type Static } from "typebox";

import { reviewSpecification } from "../adapters/models/pi-readiness-reviewer.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { parseWorkflowRequest } from "../core/workflow/workflow-request.js";
import { none, some, type Option } from "../core/types/option.js";
import {
  digestTaskSpecification,
  type TaskSpecification,
} from "../core/tasks/readiness.js";
import type {
  TaskReadyEvent,
  TaskSpecifiedEvent,
} from "../core/tasks/task-board.js";
import {
  parseTaskEventId,
  parseTaskEventOccurredAt,
  type TaskId,
} from "../core/tasks/task-values.js";
import { handleCampaignCommand } from "./campaign-command.js";
import { handleWorkCommand } from "./work-command.js";

const candidateSchema = Type.Object({
  taskId: Type.String(),
  initiativeId: Type.String(),
  rank: Type.Integer({ minimum: 0 }),
  blockerPhase: StringEnum(["none", "pre-mutation", "post-mutation"] as const),
  estimatedCostMicros: Type.Integer({ minimum: 0 }),
  estimatedTokens: Type.Integer({ minimum: 0 }),
});

const boundsSchema = Type.Object({
  taskLimit: Type.Integer({ minimum: 1 }),
  initiativeTaskLimit: Type.Integer({ minimum: 1 }),
  durationLimitMs: Type.Integer({ minimum: 1 }),
  costLimitMicros: Type.Integer({ minimum: 1 }),
  tokenLimit: Type.Integer({ minimum: 1 }),
  concurrencyLimit: Type.Integer({ minimum: 1 }),
});

const workflowRequestSchema = Type.Object({
  kind: StringEnum([
    "begin-task",
    "campaign-start",
    "campaign-tick",
    "campaign-goal",
    "campaign-status",
  ] as const),
  taskId: Type.Optional(Type.String()),
  specification: Type.Optional(
    Type.Object({
      outcome: Type.String(),
      scenarios: Type.Array(
        Type.Object({
          name: Type.String(),
          given: Type.Array(Type.String()),
          when: Type.Array(Type.String()),
          then: Type.Array(Type.String()),
        }),
      ),
      acceptanceCriteria: Type.Array(Type.String()),
      exclusions: Type.Array(Type.String()),
      dependencies: Type.Array(Type.String()),
      testMappings: Type.Array(Type.String()),
      architectureImplications: Type.String(),
    }),
  ),
  bounds: Type.Optional(boundsSchema),
  candidates: Type.Optional(Type.Array(candidateSchema, { maxItems: 100 })),
  goal: Type.Optional(Type.String({ minLength: 1, maxLength: 200 })),
});

export type WorkflowRequestToolInput = Static<typeof workflowRequestSchema>;

function encoded(value: unknown): string {
  return Buffer.from(JSON.stringify(value), "utf8").toString("base64url");
}

function ensureSpecification(
  taskId: TaskId,
  specification: Option<TaskSpecification>,
  context: ExtensionContext,
): Option<string> {
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = board.tasks.find((candidate) => candidate.id === taskId);
  if (board.mode !== "writable" || task === undefined)
    return some("TIBER_TASK_NOT_FOUND: begin-task requires signed task state");
  if (specification.kind === "none") return none;
  const digest = digestTaskSpecification(specification.value);
  if (
    task.specificationDigest.kind === "some" &&
    task.specificationDigest.value === digest
  )
    return none;
  if (task.specificationDigest.kind === "some" && task.state !== "Backlog")
    return some(
      "TIBER_SPECIFICATION_STALE: only a Backlog specification can be corrected before work",
    );
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok)
    return some(
      "TIBER_TASK_VALUE_INVALID: specification event values are invalid",
    );
  const event: TaskSpecifiedEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-specified",
    occurredAt: occurredAt.value,
    taskId,
    specificationDigest: digest,
    specification: specification.value,
  };
  const published = remote.publish(event);
  return published.mode === "writable"
    ? none
    : some("TIBER_SPECIFICATION_NOT_PUBLISHED: signed publication failed");
}

async function ensureReady(
  taskId: TaskId,
  context: ExtensionContext,
  signal: AbortSignal | undefined,
  onProgress: (message: string) => void,
): Promise<Option<string>> {
  const remote = new GitTaskRemote(context.cwd);
  const board = remote.read();
  const task = board.tasks.find((candidate) => candidate.id === taskId);
  if (board.mode !== "writable" || task === undefined)
    return some("TIBER_TASK_NOT_FOUND: begin-task requires signed task state");
  if (task.state !== "Backlog") return none;
  if (
    task.specification.kind === "none" ||
    task.specificationDigest.kind === "none"
  )
    return some(
      "TIBER_SPECIFICATION_NOT_READY: normal conversation must establish a canonical specification before work",
    );
  const review = await reviewSpecification(
    context.cwd,
    task.specification.value,
    task.specificationDigest.value,
    signal === undefined ? { onProgress } : { signal, onProgress },
  );
  if (!review.ok)
    return some(`${review.failure.code}: independent readiness review failed`);
  if (review.value.findings.length !== 0)
    return some(
      [
        "TIBER_SPECIFICATION_NOT_READY: resolve the blocking specification findings, then retry begin-task:",
        ...review.value.findings.map((finding) => `- ${finding}`),
      ].join("\n"),
    );
  const eventId = parseTaskEventId(randomUUID());
  const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
  if (!eventId.ok || !occurredAt.ok)
    return some(
      "TIBER_TASK_VALUE_INVALID: readiness receipt values are invalid",
    );
  const event: TaskReadyEvent = {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-ready",
    occurredAt: occurredAt.value,
    taskId,
    specificationDigest: task.specificationDigest.value,
    review: review.value.review,
  };
  const published = remote.publish(event);
  return published.mode === "writable"
    ? none
    : some(
        "TIBER_SPECIFICATION_NOT_READY: readiness receipt was not published",
      );
}

async function dispatch(
  value: unknown,
  context: ExtensionContext,
  signal: AbortSignal | undefined,
  onProgress: (message: string) => void,
): Promise<string> {
  const observations: string[] = [];
  const requestContext = new Proxy(context, {
    get(target, property, receiver) {
      if (property !== "ui") {
        const member: unknown = Reflect.get(target, property, receiver);
        return member;
      }
      return new Proxy(target.ui, {
        get(ui, uiProperty, uiReceiver) {
          if (uiProperty !== "notify") {
            const member: unknown = Reflect.get(ui, uiProperty, uiReceiver);
            return member;
          }
          return (message: string, level: "info" | "warning" | "error") => {
            observations.push(message);
            ui.notify(message, level);
          };
        },
      });
    },
  });
  const evaluated = (fallback: string): string =>
    observations.at(-1) ?? fallback;
  const request = parseWorkflowRequest(value);
  if (!request.ok) return `${request.failure.code}: request denied`;
  if (request.value.kind === "begin-task") {
    const specificationFailure = ensureSpecification(
      request.value.taskId,
      request.value.specification,
      requestContext,
    );
    if (specificationFailure.kind === "some") return specificationFailure.value;
    const readinessFailure = await ensureReady(
      request.value.taskId,
      requestContext,
      signal,
      onProgress,
    );
    if (readinessFailure.kind === "some") return readinessFailure.value;
    await handleWorkCommand(request.value.taskId, requestContext);
    return evaluated(
      `Workflow host evaluated begin-task for ${request.value.taskId}`,
    );
  }
  if (request.value.kind === "campaign-start") {
    await handleCampaignCommand(
      `start ${encoded(request.value.bounds)}`,
      requestContext,
    );
    return evaluated("Workflow host evaluated campaign-start");
  }
  if (request.value.kind === "campaign-tick") {
    await handleCampaignCommand(
      `tick ${encoded({
        candidates: request.value.candidates,
      })}`,
      requestContext,
    );
    return evaluated("Workflow host evaluated campaign-tick");
  }
  if (request.value.kind === "campaign-goal") {
    await handleCampaignCommand(`goal ${request.value.goal}`, requestContext);
    return evaluated("Workflow host evaluated campaign-goal");
  }
  await handleCampaignCommand("status", requestContext);
  return evaluated("Workflow host evaluated campaign-status");
}

export function registerWorkflowRequestTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "tiber_workflow_request",
    label: "Tiber workflow request",
    description:
      "Request a semantic Tiber workflow operation inferred from the user's normal conversation. The deterministic host validates signed state and authority; this request never grants authority itself.",
    promptSnippet:
      "Request deterministic Tiber workflow progression without asking the user to type slash commands",
    promptGuidelines: [
      "Use tiber_workflow_request when normal user intent requires beginning governed task work or advancing a bounded campaign; never ask the user to translate ordinary intent into /tiber commands.",
    ],
    parameters: workflowRequestSchema,
    async execute(_toolCallId, parameters, signal, onUpdate, context) {
      if (signal?.aborted)
        return {
          content: [{ type: "text", text: "TIBER_WORKFLOW_REQUEST_CANCELLED" }],
          details: { disposition: "cancelled" },
        };
      const result = await dispatch(parameters, context, signal, (message) => {
        onUpdate?.({
          content: [{ type: "text", text: message }],
          details: { disposition: "reviewing" },
        });
      });
      return signal?.aborted
        ? {
            content: [{ type: "text", text: result }],
            details: { disposition: "cancelled" },
          }
        : {
            content: [{ type: "text", text: result }],
            details: { disposition: "evaluated" },
          };
    },
  });
}
