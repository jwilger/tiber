import { randomUUID } from "node:crypto";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import {
  formatTaskBoard,
  type TaskCreatedEvent,
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
    description: "Create or inspect a signed shared task",
    handler: (argumentsText, context) => {
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
        return Promise.resolve();
      }
      const board = remote.read();
      if (operation === "show" && rest.length === 1) {
        const task = board.tasks.find((candidate) => candidate.id === rest[0]);
        context.ui.notify(
          task === undefined
            ? `TIBER_TASK_NOT_FOUND: ${rest[0] ?? ""}`
            : `${task.id}\n${task.state}${task.blocked ? " [Blocked]" : ""}\n${task.title}\n${task.description}`,
          task === undefined ? "error" : "info",
        );
        return Promise.resolve();
      }
      context.ui.notify(
        "usage: /tiber:task create <title> | /tiber:task show <id>",
        "error",
      );
      return Promise.resolve();
    },
  });
}
