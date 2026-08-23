export type TaskState = "Backlog" | "Ready" | "In Progress" | "Done";

export interface Task {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly state: TaskState;
  readonly blocked: boolean;
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

export function foldTaskEvents(events: readonly TaskCreatedEvent[]): TaskBoard {
  const tasks = new Map<string, Task>();
  const eventIds = new Set<string>();
  for (const event of events) {
    if (eventIds.has(event.eventId) || tasks.has(event.task.id)) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: "duplicate task authority event",
      };
    }
    eventIds.add(event.eventId);
    tasks.set(event.task.id, {
      id: event.task.id,
      title: event.task.title,
      description: event.task.description,
      state: "Backlog",
      blocked: false,
    });
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
