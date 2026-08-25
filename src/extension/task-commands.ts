import { randomUUID } from "node:crypto";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { reviewSpecification } from "../adapters/models/pi-readiness-reviewer.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import {
  digestTaskSpecification,
  parseTaskSpecification,
} from "../core/tasks/readiness.js";
import {
  formatTaskBoard,
  type TaskCreatedEvent,
  type TaskReadyEvent,
  type TaskSpecifiedEvent,
} from "../core/tasks/task-board.js";
import {
  parseTaskDescription,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  parseTaskTitle,
} from "../core/tasks/task-values.js";

export function registerTaskCommands(pi: ExtensionAPI): void {
  pi.registerCommand("tiber:tasks", {
    description: "Show the signed shared Kanban board",
    handler: (_arguments, context) => {
      context.ui.notify(
        formatTaskBoard(new GitTaskRemote(context.cwd).read()),
        "info",
      );
      return Promise.resolve();
    },
  });

  pi.registerCommand("tiber:task", {
    description: "Create, specify, review, or inspect a signed shared task",
    handler: async (argumentsText, context) => {
      const [operation, ...rest] = argumentsText.trim().split(/\s+/u);
      const remote = new GitTaskRemote(context.cwd);
      if (operation === "create" && rest.join(" ").trim().length > 0) {
        const eventId = parseTaskEventId(randomUUID());
        const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
        const taskId = parseTaskId(randomUUID());
        const title = parseTaskTitle(rest.join(" ").trim());
        const description = parseTaskDescription("");
        if (
          !eventId.ok ||
          !occurredAt.ok ||
          !taskId.ok ||
          !title.ok ||
          !description.ok
        ) {
          context.ui.notify(
            "TIBER_TASK_VALUE_INVALID: task creation values are invalid",
            "error",
          );
          return;
        }
        const event: TaskCreatedEvent = {
          schemaVersion: 1,
          eventId: eventId.value,
          kind: "task-created",
          occurredAt: occurredAt.value,
          task: {
            id: taskId.value,
            title: title.value,
            description: description.value,
          },
        };
        const board = remote.publish(event);
        context.ui.notify(
          formatTaskBoard(board),
          board.mode === "writable" ? "info" : "error",
        );
        return;
      }

      const board = remote.read();
      if (operation === "specify" && rest.length === 2) {
        const taskId = parseTaskId(rest[0]);
        let decoded: unknown;
        try {
          decoded = JSON.parse(
            Buffer.from(rest[1] ?? "", "base64url").toString("utf8"),
          );
        } catch {
          decoded = undefined;
        }
        const specification = parseTaskSpecification(decoded);
        const task = taskId.ok
          ? board.tasks.find((candidate) => candidate.id === taskId.value)
          : undefined;
        if (!specification.ok || !taskId.ok || task === undefined) {
          context.ui.notify(
            "TIBER_SPECIFICATION_INVALID: expected an existing task and base64url specification JSON",
            "error",
          );
          return;
        }
        if (task.state !== "Backlog") {
          context.ui.notify(
            "TIBER_SPECIFICATION_STALE: only a Backlog specification can be corrected",
            "error",
          );
          return;
        }
        const eventId = parseTaskEventId(randomUUID());
        const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
        if (!eventId.ok || !occurredAt.ok) {
          context.ui.notify(
            "TIBER_TASK_VALUE_INVALID: task event values are invalid",
            "error",
          );
          return;
        }
        const event: TaskSpecifiedEvent = {
          schemaVersion: 1,
          eventId: eventId.value,
          kind: "task-specified",
          occurredAt: occurredAt.value,
          taskId: taskId.value,
          specificationDigest: digestTaskSpecification(specification.value),
          specification: specification.value,
        };
        context.ui.notify(formatTaskBoard(remote.publish(event)), "info");
        return;
      }

      if (operation === "ready" && rest.length === 1) {
        const task = board.tasks.find((candidate) => candidate.id === rest[0]);
        if (
          task?.state !== "Backlog" ||
          task.specification.kind !== "some" ||
          task.specificationDigest.kind !== "some"
        ) {
          context.ui.notify(
            "TIBER_SPECIFICATION_NOT_READY: task must be Backlog with a canonical specification",
            "error",
          );
          return;
        }
        const review = await reviewSpecification(
          context.cwd,
          task.specification.value,
          task.specificationDigest.value,
        );
        if (!review.ok) {
          context.ui.notify(
            `${review.failure.code}: independent readiness review failed`,
            "error",
          );
          return;
        }
        if (review.value.findings.length !== 0) {
          context.ui.notify(
            [
              "TIBER_SPECIFICATION_NOT_READY: independent readiness review returned actionable findings:",
              ...review.value.findings.map((finding) => `- ${finding}`),
            ].join("\n"),
            "error",
          );
          return;
        }
        const eventId = parseTaskEventId(randomUUID());
        const occurredAt = parseTaskEventOccurredAt(new Date().toISOString());
        if (!eventId.ok || !occurredAt.ok) {
          context.ui.notify(
            "TIBER_TASK_VALUE_INVALID: task event values are invalid",
            "error",
          );
          return;
        }
        const event: TaskReadyEvent = {
          schemaVersion: 1,
          eventId: eventId.value,
          kind: "task-ready",
          occurredAt: occurredAt.value,
          taskId: task.id,
          specificationDigest: task.specificationDigest.value,
          review: review.value.review,
        };
        context.ui.notify(formatTaskBoard(remote.publish(event)), "info");
        return;
      }

      if (operation === "show" && rest.length === 1) {
        const task = board.tasks.find((candidate) => candidate.id === rest[0]);
        context.ui.notify(
          task === undefined
            ? `TIBER_TASK_NOT_FOUND: ${rest[0] ?? ""}`
            : `${task.id}\n${task.state}${task.blockStatus === "blocked" ? " [Blocked]" : ""}\n${task.title}\n${task.description}`,
          task === undefined ? "error" : "info",
        );
        return;
      }
      context.ui.notify(
        "usage: /tiber:task create <title> | show <id> | specify <id> <base64url-json> | ready <id>",
        "error",
      );
    },
  });
}
