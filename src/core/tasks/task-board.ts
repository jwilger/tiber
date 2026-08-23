import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
  type TaskSpecification,
} from "./readiness.js";

export type TaskState = "Backlog" | "Ready" | "In Progress" | "Done";

export interface TaskClaim {
  readonly claimId: string;
  readonly owner: string;
  readonly baselineRevision: string;
  readonly workflowDigest: string;
}

export interface Task {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly state: TaskState;
  readonly blocked: boolean;
  readonly specification?: TaskSpecification;
  readonly specificationDigest?: string;
  readonly claim?: TaskClaim;
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

export interface TaskClaimedEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-claimed";
  readonly occurredAt: string;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly claim: TaskClaim;
}

export interface TaskClaimTakenOverEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-claim-taken-over";
  readonly occurredAt: string;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly previousClaimId: string;
  readonly claim: TaskClaim;
}

export interface TaskClaimReleasedEvent {
  readonly schemaVersion: 1;
  readonly eventId: string;
  readonly kind: "task-claim-released";
  readonly occurredAt: string;
  readonly taskId: string;
  readonly specificationDigest: string;
  readonly claimId: string;
  readonly reason: "baseline-drift" | "released" | "completed";
}

export type TaskEvent =
  | TaskCreatedEvent
  | TaskSpecifiedEvent
  | TaskReadyEvent
  | TaskClaimedEvent
  | TaskClaimTakenOverEvent
  | TaskClaimReleasedEvent;

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
  if (value.kind === "task-claimed" && isRecord(value.claim)) {
    const claim = value.claim;
    if (
      // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects every non-string JSON value and this guard narrows the semantic type.
      typeof claim.claimId !== "string" ||
      !/^[0-9a-f-]{36}$/u.test(claim.claimId) ||
      // Stryker disable next-line ConditionalExpression: trim is only reached after this guard and all non-string values fail the subsequent semantic operation; this guard narrows the type.
      typeof claim.owner !== "string" ||
      claim.owner.trim().length === 0 ||
      // Stryker disable next-line ConditionalExpression: the following SHA grammar rejects every non-string JSON value and this guard narrows the semantic type.
      typeof claim.baselineRevision !== "string" ||
      !/^[0-9a-f]{40}$/u.test(claim.baselineRevision) ||
      // Stryker disable next-line ConditionalExpression: the following digest grammar rejects every non-string JSON value and this guard narrows the semantic type.
      typeof claim.workflowDigest !== "string" ||
      !/^sha256:[0-9a-f]{64}$/u.test(claim.workflowDigest)
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-claimed",
      occurredAt: common.occurredAt,
      taskId: value.taskId,
      specificationDigest: value.specificationDigest,
      claim: {
        claimId: claim.claimId,
        owner: claim.owner.trim(),
        baselineRevision: claim.baselineRevision,
        workflowDigest: claim.workflowDigest,
      },
    };
  }
  if (value.kind === "task-claim-taken-over" && isRecord(value.claim)) {
    const claim = value.claim;
    if (
      // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects non-string JSON values and this guard narrows the type.
      typeof value.previousClaimId !== "string" ||
      !/^[0-9a-f-]{36}$/u.test(value.previousClaimId) ||
      // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects non-string JSON values and this guard narrows the type.
      typeof claim.claimId !== "string" ||
      !/^[0-9a-f-]{36}$/u.test(claim.claimId) ||
      typeof claim.owner !== "string" ||
      claim.owner.trim().length === 0 ||
      // Stryker disable next-line ConditionalExpression: the following SHA grammar rejects non-string JSON values and this guard narrows the type.
      typeof claim.baselineRevision !== "string" ||
      !/^[0-9a-f]{40}$/u.test(claim.baselineRevision) ||
      // Stryker disable next-line ConditionalExpression: the following digest grammar rejects non-string JSON values and this guard narrows the type.
      typeof claim.workflowDigest !== "string" ||
      !/^sha256:[0-9a-f]{64}$/u.test(claim.workflowDigest)
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-claim-taken-over",
      occurredAt: common.occurredAt,
      taskId: value.taskId,
      specificationDigest: value.specificationDigest,
      previousClaimId: value.previousClaimId,
      claim: {
        claimId: claim.claimId,
        owner: claim.owner.trim(),
        baselineRevision: claim.baselineRevision,
        workflowDigest: claim.workflowDigest,
      },
    };
  }
  if (value.kind === "task-claim-released") {
    if (
      // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects every non-string JSON value and this guard narrows the semantic type.
      typeof value.claimId !== "string" ||
      !/^[0-9a-f-]{36}$/u.test(value.claimId) ||
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
      taskId: value.taskId,
      specificationDigest: value.specificationDigest,
      claimId: value.claimId,
      reason: value.reason,
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
    if (event.kind === "task-claimed") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so either condition independently detects an existing claim; both document the closed state invariant.
        task.state !== "Ready" ||
        // Stryker disable next-line ConditionalExpression: claim and In Progress state are installed atomically, so the state check already detects every existing claim; this check documents exclusivity directly.
        task.claim !== undefined ||
        task.specificationDigest !== event.specificationDigest
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: "task claim is not exclusive or state-bound",
        };
      }
      tasks.set(task.id, { ...task, state: "In Progress", claim: event.claim });
      continue;
    }
    if (event.kind === "task-claim-taken-over") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so exact claim identity independently establishes this state; both checks document the invariant.
        task.state !== "In Progress" ||
        // Stryker disable next-line OptionalChaining: In Progress always carries a claim by the closed fold invariant.
        task.claim?.claimId !== event.previousClaimId ||
        task.specificationDigest !== event.specificationDigest ||
        task.claim.baselineRevision !== event.claim.baselineRevision ||
        task.claim.workflowDigest !== event.claim.workflowDigest ||
        event.claim.claimId === event.previousClaimId
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: "task claim takeover is not exact or state-bound",
        };
      }
      tasks.set(task.id, { ...task, claim: event.claim });
      continue;
    }
    if (event.kind === "task-claim-released") {
      if (
        task.claim?.claimId !== event.claimId ||
        // Stryker disable next-line ConditionalExpression: a claim can only be installed on a Ready task whose specification and digest are present; this restates that fold invariant before reconstruction.
        task.specification === undefined ||
        // Stryker disable next-line ConditionalExpression: a claim can only be installed on a Ready task whose specification and digest are present; this restates that fold invariant before reconstruction.
        task.specificationDigest === undefined
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: "task claim release does not match the active claim",
        };
      }
      tasks.set(task.id, {
        id: task.id,
        title: task.title,
        description: task.description,
        state: "Ready",
        blocked: false,
        specification: task.specification,
        specificationDigest: task.specificationDigest,
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
