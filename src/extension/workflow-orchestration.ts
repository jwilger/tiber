import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import {
  parseContextBudget,
  parseContextSegment,
  planContext,
  type ContextSegment,
} from "../core/context/headroom.js";
import type { TaskBoard } from "../core/tasks/task-board.js";

const guidanceBudget = (() => {
  const parsed = parseContextBudget({
    contextWindowTokens: 49_152,
    reserveTokens: 16_384,
    hardInputTokens: 32_768,
    segmentByteLimit: 64 * 1024,
  });
  if (!parsed.ok) throw new Error("invalid built-in workflow context budget");
  return parsed.value;
})();

function authoritySegment(content: string): ContextSegment {
  const parsed = parseContextSegment({
    id: "signed-task-state",
    priority: "authority",
    content,
    provenance: "signed-ref:tiber/tasks/v1",
  });
  if (!parsed.ok) throw new Error("invalid signed workflow context segment");
  return parsed.value;
}

export function workflowGuidance(board: TaskBoard): string {
  const state =
    board.mode !== "writable"
      ? "TIBER_WORKFLOW_STATE: signed task authority is read-only; inspect diagnostics and do not request mutation."
      : writableWorkflowState(board);
  const planned = planContext({
    budget: guidanceBudget,
    stable: {
      prompt: "Tiber automatic workflow orchestration v1",
      initialContext: [
        "Infer the needed governed workflow request from ordinary user intent.",
        "Use tiber_workflow_request for safe progression; do not ask the user to type /tiber commands.",
        "A workflow request is not authority. Proactively address actionable host feedback and retry compliant recovery; stop only for a genuine external or human-only blocker.",
        "Report deterministic denials and exact evidence requirements when no compliant autonomous recovery remains.",
      ],
      toolSchemas: ["tiber_workflow_request:v1", "tiber_exception_request:v1"],
    },
    dynamic: [authoritySegment(state)],
  });
  return planned.ok
    ? planned.value.context
    : "TIBER_CONTEXT_BUDGET_EXHAUSTED: signed workflow authority does not fit the mandatory context budget; do not proceed.";
}

function writableWorkflowState(board: TaskBoard): string {
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
