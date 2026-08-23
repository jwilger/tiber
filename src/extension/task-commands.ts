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
        const event: TaskCreatedEvent = {
          schemaVersion: 1,
          eventId: randomUUID(),
          kind: "task-created",
          occurredAt: new Date().toISOString(),
          task: {
            id: randomUUID(),
            title: rest.join(" ").trim(),
            description: "",
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
        const taskId = rest[0] ?? "";
        let decoded: unknown;
        try {
          decoded = JSON.parse(
            Buffer.from(rest[1] ?? "", "base64url").toString("utf8"),
          );
        } catch {
          decoded = undefined;
        }
        const specification = parseTaskSpecification(decoded);
        if (
          specification === undefined ||
          !board.tasks.some((task) => task.id === taskId)
        ) {
          context.ui.notify(
            "TIBER_SPECIFICATION_INVALID: expected an existing task and base64url specification JSON",
            "error",
          );
          return;
        }
        const event: TaskSpecifiedEvent = {
          schemaVersion: 1,
          eventId: randomUUID(),
          kind: "task-specified",
          occurredAt: new Date().toISOString(),
          taskId,
          specificationDigest: digestTaskSpecification(specification),
          specification,
        };
        context.ui.notify(formatTaskBoard(remote.publish(event)), "info");
        return;
      }

      if (operation === "ready" && rest.length === 1) {
        const task = board.tasks.find((candidate) => candidate.id === rest[0]);
        if (
          task?.specification === undefined ||
          task.specificationDigest === undefined
        ) {
          context.ui.notify(
            "TIBER_SPECIFICATION_NOT_READY: task has no canonical specification",
            "error",
          );
          return;
        }
        const review = await reviewSpecification(
          context.cwd,
          task.specification,
          task.specificationDigest,
        );
        if (review?.findingCount !== 0) {
          context.ui.notify(
            "TIBER_SPECIFICATION_NOT_READY: independent review failed or returned findings",
            "error",
          );
          return;
        }
        const event: TaskReadyEvent = {
          schemaVersion: 1,
          eventId: randomUUID(),
          kind: "task-ready",
          occurredAt: new Date().toISOString(),
          taskId: task.id,
          specificationDigest: task.specificationDigest,
          review,
        };
        context.ui.notify(formatTaskBoard(remote.publish(event)), "info");
        return;
      }

      if (operation === "show" && rest.length === 1) {
        const task = board.tasks.find((candidate) => candidate.id === rest[0]);
        context.ui.notify(
          task === undefined
            ? `TIBER_TASK_NOT_FOUND: ${rest[0] ?? ""}`
            : `${task.id}\n${task.state}${task.blocked ? " [Blocked]" : ""}\n${task.title}\n${task.description}`,
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
