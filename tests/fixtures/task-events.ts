import {
  parseTaskEvent,
  type TaskEvent,
} from "../../src/core/tasks/task-board.js";

export function requireTaskEvent<Kind extends TaskEvent["kind"]>(
  value: unknown,
  kind: Kind,
): Extract<TaskEvent, { readonly kind: Kind }>;
export function requireTaskEvent(
  value: unknown,
  kind: TaskEvent["kind"],
): TaskEvent {
  const event = parseTaskEvent(value);
  if (!event.ok || event.value.kind !== kind)
    throw new Error(`invalid ${kind} fixture`);
  return event.value;
}
