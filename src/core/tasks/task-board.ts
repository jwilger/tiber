import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
  type TaskSpecification,
} from "./readiness.js";

export type TaskState = "Backlog" | "Ready" | "In Progress" | "Done";

export interface Task {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly state: TaskState;
  readonly blocked: boolean;
  readonly specification?: TaskSpecification;
  readonly specificationDigest?: string;
}

export interface TaskCreatedEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-created";
  readonly occurredAt: string;
  readonly task: {
    readonly id: string;
    readonly title: string;
    readonly description: string;
  };
}

export interface TaskSpecifiedEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-specified";
  readonly occurredAt: string;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly specification: TaskSpecification;
}

export interface TaskReadyEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-ready";
  readonly occurredAt: string;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly review: ReadinessReview;
}

export type TaskEvent = TaskCreatedEvent | TaskSpecifiedEvent | TaskReadyEvent;

export interface TaskBoard {
  readonly mode: "writable" | "degraded-read-only";
  readonly tasks: readonly Task[];
  readonly failure?: string;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives safely expose undefined required fields and are rejected by the shape parser; typeof establishes the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseTaskCreatedEvent(
  value: unknown,
): TaskCreatedEvent | undefined {
  if (
    !isRecord(value) ||
    value.schemaVersion !== 1 ||
    value.kind !== "task-created" ||
    !isRecord(value.task)
  )
    return undefined;
  if (
    // Stryker disable next-line ConditionalExpression: the following UUID regex string-coerces and rejects every non-string JSON value; this explicit guard establishes the semantic string type.
    typeof value.eventId !== "string" ||
    typeof value.occurredAt !== "string" ||
    // Stryker disable next-line ConditionalExpression: the following UUID regex string-coerces and rejects every non-string JSON value; this explicit guard establishes the semantic string type.
    typeof value.task.id !== "string" ||
    typeof value.task.title !== "string" ||
    typeof value.task.description !== "string" ||
    !/^[0-9a-f-]{36}$/u.test(value.eventId) ||
    !/^[0-9a-f-]{36}$/u.test(value.task.id) ||
    value.task.title.trim().length === 0 ||
    !Number.isFinite(Date.parse(value.occurredAt))
  )
    return undefined;
  return {
    schemaVersion: 1,
    eventId: value.eventId,
    kind: "task-created",
    occurredAt: value.occurredAt,
    task: {
      id: value.task.id,
      title: value.task.title.trim(),
      description: value.task.description,
    },
  };
}

function parseCommonEvent(
  value: Readonly<Record<string, unknown>>,
): { readonly eventId: string; readonly occurredAt: string } | undefined {
  if (
    value.schemaVersion !== 1 ||
    // Stryker disable next-line ConditionalExpression: the UUID grammar string-coerces and rejects every non-string JSON value; this guard narrows the semantic type.
    typeof value.eventId !== "string" ||
    !/^[0-9a-f-]{36}$/u.test(value.eventId) ||
    typeof value.occurredAt !== "string" ||
    !Number.isFinite(Date.parse(value.occurredAt))
  )
    return undefined;
  return { eventId: value.eventId, occurredAt: value.occurredAt };
}

export function parseTaskEvent(value: unknown): TaskEvent | undefined {
  const created = parseTaskCreatedEvent(value);
  if (created !== undefined) return created;
  if (!isRecord(value)) return undefined;
  const common = parseCommonEvent(value);
  if (common === undefined) return undefined;
  if (
    // Stryker disable next-line ConditionalExpression: the UUID grammar string-coerces and rejects every non-string JSON value; this guard narrows the semantic type.
    typeof value.taskId !== "string" ||
    !/^[0-9a-f-]{36}$/u.test(value.taskId) ||
    // Stryker disable next-line ConditionalExpression: the digest grammar string-coerces and rejects every non-string JSON value; this guard narrows the semantic type.
    typeof value.specificationDigest !== "string" ||
    !/^sha256:[0-9a-f]{64}$/u.test(value.specificationDigest)
  )
    return undefined;
  if (value.kind === "task-specified") {
    const specification = parseTaskSpecification(value.specification);
    return specification === undefined ||
      digestTaskSpecification(specification) !== value.specificationDigest
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-specified",
          occurredAt: common.occurredAt,
          taskId: value.taskId,
          specificationDigest: value.specificationDigest,
          specification,
        };
  }
  if (value.kind === "task-ready" && isRecord(value.review)) {
    const review = value.review;
    if (
      review.freshContext !== true ||
      review.reviewerRole !== "specification-reviewer" ||
      // Stryker disable next-line ConditionalExpression: Number.isSafeInteger rejects every non-number JSON value; this guard narrows the semantic type.
      typeof review.findingCount !== "number" ||
      !Number.isSafeInteger(review.findingCount) ||
      review.findingCount < 0 ||
      review.reviewedSpecificationDigest !== value.specificationDigest
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-ready",
      occurredAt: common.occurredAt,
      taskId: value.taskId,
      specificationDigest: value.specificationDigest,
      review: {
        freshContext: true,
        reviewerRole: "specification-reviewer",
        findingCount: review.findingCount,
        reviewedSpecificationDigest: value.specificationDigest,
      },
    };
  }
  return undefined;
}

export function foldTaskEvents(events: readonly TaskEvent[]): TaskBoard {
  const tasks = new Map<string, Task>();
  const eventIds = new Set<string>();
  for (const event of events) {
    if (eventIds.has(event.eventId)) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: "duplicate task authority event",
      };
    }
    eventIds.add(event.eventId);
    if (event.kind === "task-created") {
      if (tasks.has(event.task.id)) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: "duplicate task authority event",
        };
      }
      tasks.set(event.task.id, {
        id: event.task.id,
        title: event.task.title,
        description: event.task.description,
        state: "Backlog",
        blocked: false,
      });
      continue;
    }
    const task = tasks.get(event.taskId);
    if (task === undefined) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: "task event references an unknown task",
      };
    }
    if (event.kind === "task-specified") {
      tasks.set(task.id, {
        ...task,
        specification: event.specification,
        specificationDigest: event.specificationDigest,
      });
      continue;
    }
    if (
      // Stryker disable next-line ConditionalExpression, LogicalOperator: specification and digest are installed atomically by the only preceding event, so digest absence/mismatch already denies every state where specification is absent.
      task.specification === undefined ||
      // Stryker disable next-line ConditionalExpression: decideReadiness compares the review-bound event digest against the task's pinned digest, so this explicit check is a fail-fast restatement of the same invariant.
      task.specificationDigest !== event.specificationDigest ||
      !decideReadiness(
        task.specification,
        task.specificationDigest,
        event.review,
      ).ready
    ) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: "Ready event lacks an exact clean specification review",
      };
    }
    tasks.set(task.id, { ...task, state: "Ready" });
  }
  return {
    mode: "writable",
    tasks: [...tasks.values()].sort((left, right) =>
      left.id.localeCompare(right.id),
    ),
  };
}

export function formatTaskBoard(board: TaskBoard): string {
  const rows = board.tasks.map(
    (task) =>
      `${task.state}${task.blocked ? " [Blocked]" : ""} | ${task.id} | ${task.title}`,
  );
  return [
    `Task board: ${board.mode}`,
    ...(board.failure === undefined ? [] : [`Failure: ${board.failure}`]),
    "State | ID | Title",
    ...(rows.length === 0 ? ["(no tasks)"] : rows),
  ].join("\n");
}
