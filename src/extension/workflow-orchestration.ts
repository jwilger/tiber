import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import type { TaskBoard } from "../core/tasks/task-board.js";

export function workflowGuidance(board: TaskBoard): string {
  if (board.mode !== "writable")
    return "TIBER_WORKFLOW_STATE: signed task authority is read-only; inspect diagnostics and do not request mutation.";
  const visibleTasks = board.tasks.slice(0, 20);
  const tasks = visibleTasks
    .map((task) => {
      const specification =
        task.specification.kind === "some" ? "specified" : "unspecified";
      const claim = task.claim.kind === "some" ? "claimed" : "unclaimed";
      const description = task.description.slice(0, 200);
      return `${task.id} | ${task.state} | ${specification} | ${claim} | ${task.title} | ${description}`;
    })
    .join("\n");
  return [
    "TIBER_WORKFLOW_STATE:",
    tasks.length === 0 ? "No signed tasks." : tasks,
    board.tasks.length > visibleTasks.length
      ? `${String(board.tasks.length - visibleTasks.length)} additional tasks omitted from bounded context.`
      : "All signed tasks shown.",
    "Infer the needed governed workflow request from the user's ordinary intent.",
    "Use tiber_workflow_request for safe progression; do not ask the user to type /tiber commands.",
    "A workflow request is not authority; report any deterministic host denial and its evidence requirement.",
  ].join("\n");
}

export function registerAutomaticWorkflowOrchestration(pi: ExtensionAPI): void {
  pi.on("before_agent_start", (_event, context) => {
    const board = new GitTaskRemote(context.cwd).read();
    return {
      message: {
        customType: "tiber-workflow-state",
        content: workflowGuidance(board),
        display: false,
      },
    };
  });
}
