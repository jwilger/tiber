import {
  parseClaimBaselineRevision,
  parseClaimOwnerIdentity,
  parseSpecificationDigest,
  parseSpecificationReviewFindingCount,
  parseTaskClaimId,
  parseTaskDescription,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  parseTaskTitle,
  type ClaimBaselineRevision,
  type ClaimOwnerIdentity,
  type SpecificationDigest,
  type TaskClaimId,
  type TaskDescription,
  type TaskEventId,
  type TaskEventOccurredAt,
  type TaskId,
  type TaskTitle,
} from "./task-values.js";
import {
  parseCompiledWorkflowDigest,
  type CompiledWorkflowDigest,
} from "../workflow/workflow-values.js";
import type { TiberFailure, TiberResult } from "../failures/tiber-failure.js";
import { none, some, type Option } from "../types/option.js";
import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
  type TaskSpecification,
} from "./readiness.js";

export type TaskState = "Backlog" | "Ready" | "In Progress" | "Done";
export type TaskBlockStatus = "blocked" | "unblocked";

export interface TaskClaim {
  readonly claimId: TaskClaimId;
  readonly owner: ClaimOwnerIdentity;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly workflowDigest: CompiledWorkflowDigest;
}

export interface Task {
  readonly id: TaskId;
  readonly title: TaskTitle;
  readonly description: TaskDescription;
  readonly state: TaskState;
  readonly blockStatus: TaskBlockStatus;
  readonly specification: Option<TaskSpecification>;
  readonly specificationDigest: Option<SpecificationDigest>;
  readonly claim: Option<TaskClaim>;
}

export interface TaskCreatedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-created";
  readonly occurredAt: TaskEventOccurredAt;
  readonly task: {
    readonly id: TaskId;
    readonly title: TaskTitle;
    readonly description: TaskDescription;
  };
}

export interface TaskSpecifiedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-specified";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly specification: TaskSpecification;
}

export interface TaskReadyEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-ready";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly review: ReadinessReview;
}

export interface TaskClaimedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claimed";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claim: TaskClaim;
}

export interface TaskClaimTakenOverEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claim-taken-over";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly previousClaimId: TaskClaimId;
  readonly claim: TaskClaim;
}

export interface TaskClaimReleasedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claim-released";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claimId: TaskClaimId;
  readonly reason: "baseline-drift" | "released" | "completed";
}

export type TaskEvent =
  | TaskCreatedEvent
  | TaskSpecifiedEvent
  | TaskReadyEvent
  | TaskClaimedEvent
  | TaskClaimTakenOverEvent
  | TaskClaimReleasedEvent;

export type TaskBoardFailureReason =
  | "duplicate-authority-event"
  | "non-exclusive-claim"
  | "non-exact-claim-release"
  | "non-exact-claim-takeover"
  | "stale-readiness-review"
  | "task-history-verification"
  | "unknown-task";
export type TaskBoardFailure = TiberFailure<
  "TIBER_TASK_BOARD_INVALID",
  {
    readonly domain: "task-board";
    readonly reason: TaskBoardFailureReason;
  },
  "corrected-task-history"
>;

export function taskBoardFailure(
  reason: TaskBoardFailureReason,
  message: string,
): TaskBoardFailure {
  return {
    code: "TIBER_TASK_BOARD_INVALID",
    message,
    safeContext: { domain: "task-board", reason },
    causes: [],
    retryability: "retry-after-state-change",
    requiredRecoveryEvidence: ["corrected-task-history"],
    redaction: "public",
  };
}

export interface TaskBoard {
  readonly mode: "writable" | "degraded-read-only";
  readonly tasks: readonly Task[];
  readonly failure: Option<TaskBoardFailure>;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives safely expose undefined required fields and are rejected by the shape parser; typeof establishes the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseTaskCreatedEventValue(
  value: unknown,
): TaskCreatedEvent | undefined {
  if (
    !isRecord(value) ||
    value.schemaVersion !== 1 ||
    value.kind !== "task-created" ||
    !isRecord(value.task)
  )
    return undefined;
  const eventId = parseTaskEventId(value.eventId);
  const occurredAt = parseTaskEventOccurredAt(value.occurredAt);
  const taskId = parseTaskId(value.task.id);
  const title = parseTaskTitle(
    typeof value.task.title === "string"
      ? value.task.title.trim()
      : value.task.title,
  );
  const description = parseTaskDescription(value.task.description);
  if (
    !eventId.ok ||
    !occurredAt.ok ||
    !taskId.ok ||
    !title.ok ||
    !description.ok
  )
    return undefined;
  return {
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
}

function parseCommonEvent(value: Readonly<Record<string, unknown>>):
  | {
      readonly eventId: TaskEventId;
      readonly occurredAt: TaskEventOccurredAt;
    }
  | undefined {
  if (value.schemaVersion !== 1) return undefined;
  const eventId = parseTaskEventId(value.eventId);
  const occurredAt = parseTaskEventOccurredAt(value.occurredAt);
  return eventId.ok && occurredAt.ok
    ? { eventId: eventId.value, occurredAt: occurredAt.value }
    : undefined;
}

function parseTaskClaim(value: unknown): TaskClaim | undefined {
  if (!isRecord(value)) return undefined;
  const claimId = parseTaskClaimId(value.claimId);
  const owner = parseClaimOwnerIdentity(
    typeof value.owner === "string" ? value.owner.trim() : value.owner,
  );
  const baselineRevision = parseClaimBaselineRevision(value.baselineRevision);
  const workflowDigest = parseCompiledWorkflowDigest(value.workflowDigest);
  return claimId.ok && owner.ok && baselineRevision.ok && workflowDigest.ok
    ? {
        claimId: claimId.value,
        owner: owner.value,
        baselineRevision: baselineRevision.value,
        workflowDigest: workflowDigest.value,
      }
    : undefined;
}

function parseTaskEventValue(value: unknown): TaskEvent | undefined {
  const created = parseTaskCreatedEventValue(value);
  if (created !== undefined) return created;
  if (!isRecord(value)) return undefined;
  const common = parseCommonEvent(value);
  if (common === undefined) return undefined;
  const taskId = parseTaskId(value.taskId);
  const specificationDigest = parseSpecificationDigest(
    value.specificationDigest,
  );
  if (!taskId.ok || !specificationDigest.ok) return undefined;
  if (value.kind === "task-specified") {
    const specification = parseTaskSpecification(value.specification);
    return !specification.ok ||
      digestTaskSpecification(specification.value) !== specificationDigest.value
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-specified",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          specification: specification.value,
        };
  }
  if (value.kind === "task-claimed") {
    const claim = parseTaskClaim(value.claim);
    return claim === undefined
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-claimed",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          claim,
        };
  }
  if (value.kind === "task-claim-taken-over") {
    const previousClaimId = parseTaskClaimId(value.previousClaimId);
    const claim = parseTaskClaim(value.claim);
    return !previousClaimId.ok || claim === undefined
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-claim-taken-over",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          previousClaimId: previousClaimId.value,
          claim,
        };
  }
  if (value.kind === "task-claim-released") {
    const claimId = parseTaskClaimId(value.claimId);
    if (
      !claimId.ok ||
      (value.reason !== "baseline-drift" &&
        value.reason !== "released" &&
        value.reason !== "completed")
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-claim-released",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      claimId: claimId.value,
      reason: value.reason,
    };
  }
  if (value.kind === "task-ready" && isRecord(value.review)) {
    const review = value.review;
    const findingCount = parseSpecificationReviewFindingCount(
      review.findingCount,
    );
    if (
      review.contextFreshness !== "fresh" ||
      review.reviewerRole !== "specification-reviewer" ||
      !findingCount.ok ||
      review.reviewedSpecificationDigest !== specificationDigest.value
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-ready",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      review: {
        contextFreshness: "fresh",
        reviewerRole: "specification-reviewer",
        findingCount: findingCount.value,
        reviewedSpecificationDigest: specificationDigest.value,
      },
    };
  }
  return undefined;
}

type TaskEventParseFailure = TiberFailure<
  "TIBER_TASK_EVENT_INVALID",
  { readonly boundary: "task-event" },
  "corrected-task-event"
>;

export function parseTaskEvent(
  value: unknown,
): TiberResult<TaskEvent, TaskEventParseFailure> {
  const event = parseTaskEventValue(value);
  return event === undefined
    ? {
        ok: false,
        failure: {
          code: "TIBER_TASK_EVENT_INVALID",
          message:
            "Task event is malformed or violates its semantic invariants",
          safeContext: { boundary: "task-event" },
          causes: [],
          retryability: "retry-after-input",
          requiredRecoveryEvidence: ["corrected-task-event"],
          redaction: "public",
        },
      }
    : { ok: true, value: event };
}

export function foldTaskEvents(events: readonly TaskEvent[]): TaskBoard {
  const tasks = new Map<string, Task>();
  const eventIds = new Set<string>();
  for (const event of events) {
    if (eventIds.has(event.eventId)) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "duplicate-authority-event",
            "duplicate task authority event",
          ),
        ),
      };
    }
    eventIds.add(event.eventId);
    if (event.kind === "task-created") {
      if (tasks.has(event.task.id)) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "duplicate-authority-event",
              "duplicate task authority event",
            ),
          ),
        };
      }
      tasks.set(event.task.id, {
        id: event.task.id,
        title: event.task.title,
        description: event.task.description,
        state: "Backlog",
        blockStatus: "unblocked",
        specification: none,
        specificationDigest: none,
        claim: none,
      });
      continue;
    }
    const task = tasks.get(event.taskId);
    if (task === undefined) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "unknown-task",
            "task event references an unknown task",
          ),
        ),
      };
    }
    if (event.kind === "task-specified") {
      tasks.set(task.id, {
        ...task,
        specification: some(event.specification),
        specificationDigest: some(event.specificationDigest),
      });
      continue;
    }
    if (event.kind === "task-claimed") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so either condition independently detects an existing claim; both document the closed state invariant.
        task.state !== "Ready" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: claim and In Progress state are installed atomically, so the state check already detects every existing claim; this check documents exclusivity directly.
        task.claim.kind === "some" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exclusive-claim",
              "task claim is not exclusive or state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        state: "In Progress",
        claim: some(event.claim),
      });
      continue;
    }
    if (event.kind === "task-claim-taken-over") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so exact claim identity independently establishes this state; both checks document the invariant.
        task.state !== "In Progress" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: claim and In Progress state are installed atomically; the state check already establishes presence.
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.previousClaimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        task.claim.value.baselineRevision !== event.claim.baselineRevision ||
        task.claim.value.workflowDigest !== event.claim.workflowDigest ||
        event.claim.claimId === event.previousClaimId
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exact-claim-takeover",
              "task claim takeover is not exact or state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, { ...task, claim: some(event.claim) });
      continue;
    }
    if (event.kind === "task-claim-released") {
      if (
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.claimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: active claims are installed only after specification and digest, so these checks document established invariants.
        task.specification.kind === "none" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: active claims are installed only after specification and digest, so these checks document established invariants.
        task.specificationDigest.kind === "none"
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exact-claim-release",
              "task claim release does not match the active claim",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        id: task.id,
        title: task.title,
        description: task.description,
        state: "Ready",
        blockStatus: "unblocked",
        specification: task.specification,
        specificationDigest: task.specificationDigest,
        claim: none,
      });
      continue;
    }
    if (
      // Stryker disable next-line ConditionalExpression, LogicalOperator, StringLiteral: specification and digest are installed atomically by the only preceding event, so digest absence/mismatch already denies every state where specification is absent.
      task.specification.kind === "none" ||
      // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
      task.specificationDigest.kind === "none" ||
      // Stryker disable next-line ConditionalExpression: the parsed Ready review is bound to the event digest, so decideReadiness below independently rejects every event/task digest mismatch as stale.
      task.specificationDigest.value !== event.specificationDigest ||
      decideReadiness(task.specificationDigest.value, event.review).status !==
        "ready"
    ) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "stale-readiness-review",
            "Ready event lacks an exact clean specification review",
          ),
        ),
      };
    }
    tasks.set(task.id, { ...task, state: "Ready" });
  }
  return {
    mode: "writable",
    tasks: [...tasks.values()].sort((left, right) =>
      left.id.localeCompare(right.id),
    ),
    failure: none,
  };
}

export function formatTaskBoard(board: TaskBoard): string {
  const rows = board.tasks.map(
    (task) =>
      `${task.state}${task.blockStatus === "blocked" ? " [Blocked]" : ""} | ${task.id} | ${task.title}`,
  );
  return [
    `Task board: ${board.mode}`,
    ...(board.failure.kind === "none"
      ? []
      : [`Failure: ${board.failure.value.message}`]),
    "State | ID | Title",
    ...(rows.length === 0 ? ["(no tasks)"] : rows),
  ].join("\n");
}
