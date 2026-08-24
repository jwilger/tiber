import {
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignGoal,
  type CampaignBounds,
  type CampaignCandidate,
  type CampaignGoal,
} from "../campaigns/campaign.js";
import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";
import {
  parseTaskSpecification,
  type TaskSpecification,
} from "../tasks/readiness.js";
import { parseTaskId, type TaskId } from "../tasks/task-values.js";
import { none, some, type Option } from "../types/option.js";

export type WorkflowRequest =
  | {
      readonly kind: "begin-task";
      readonly taskId: TaskId;
      readonly specification: Option<TaskSpecification>;
    }
  | { readonly kind: "campaign-start"; readonly bounds: CampaignBounds }
  | {
      readonly kind: "campaign-tick";
      readonly candidates: readonly CampaignCandidate[];
    }
  | { readonly kind: "campaign-goal"; readonly goal: CampaignGoal }
  | { readonly kind: "campaign-status" };

type WorkflowRequestFailure = TiberFailure<
  "TIBER_WORKFLOW_REQUEST_INVALID",
  { readonly field: "workflow-request" },
  "corrected-workflow-request"
>;

type WorkflowRequestResult = TiberResult<
  WorkflowRequest,
  WorkflowRequestFailure
>;

function invalid(): WorkflowRequestResult {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_WORKFLOW_REQUEST_INVALID",
      "workflow-request",
      "corrected-workflow-request",
    ),
  };
}

function object(value: unknown): value is Record<string, unknown> {
  // Stryker disable next-line ConditionalExpression: null and array checks plus the required kind field reject all non-object primitives; typeof preserves explicit trust-boundary narrowing.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(
  value: Readonly<Record<string, unknown>>,
  keys: readonly string[],
): boolean {
  // Stryker disable next-line MethodExpression: equality uses length plus order-independent membership, so sorting cannot alter the decision.
  const actual = Object.keys(value).sort();
  return (
    actual.length === keys.length && keys.every((key) => actual.includes(key))
  );
}

export function parseWorkflowRequest(value: unknown): WorkflowRequestResult {
  // Stryker disable next-line ConditionalExpression: every closed branch compares kind to a string literal and the final fallback rejects it; this guard preserves explicit boundary narrowing.
  if (!object(value) || typeof value.kind !== "string") return invalid();
  if (
    value.kind === "begin-task" &&
    (exactKeys(value, ["kind", "taskId"]) ||
      exactKeys(value, ["kind", "specification", "taskId"]))
  ) {
    const taskId = parseTaskId(value.taskId);
    const specification =
      "specification" in value
        ? parseTaskSpecification(value.specification)
        : undefined;
    return taskId.ok && specification?.ok !== false
      ? {
          ok: true,
          value: {
            kind: "begin-task",
            taskId: taskId.value,
            specification:
              specification?.ok === true ? some(specification.value) : none,
          },
        }
      : invalid();
  }
  if (value.kind === "campaign-start" && exactKeys(value, ["bounds", "kind"])) {
    const bounds = parseCampaignBounds(value.bounds);
    return bounds.ok
      ? { ok: true, value: { kind: "campaign-start", bounds: bounds.value } }
      : invalid();
  }
  if (
    value.kind === "campaign-tick" &&
    exactKeys(value, ["candidates", "kind"]) &&
    Array.isArray(value.candidates)
  ) {
    const candidates = value.candidates.map(parseCampaignCandidate);
    return candidates.every((candidate) => candidate.ok)
      ? {
          ok: true,
          value: {
            kind: "campaign-tick",
            candidates: candidates.map((candidate) => candidate.value),
          },
        }
      : invalid();
  }
  if (value.kind === "campaign-goal" && exactKeys(value, ["goal", "kind"])) {
    const goal = parseCampaignGoal(value.goal);
    return goal.ok
      ? { ok: true, value: { kind: "campaign-goal", goal: goal.value } }
      : invalid();
  }
  return value.kind === "campaign-status" && exactKeys(value, ["kind"])
    ? { ok: true, value: { kind: "campaign-status" } }
    : invalid();
}
