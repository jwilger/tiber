import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  formatTaskBoard,
  parseTaskCreatedEvent,
  type TaskCreatedEvent,
} from "../../src/core/tasks/task-board.js";

const event: TaskCreatedEvent = {
  schemaVersion: 1,
  eventId: "11111111-1111-4111-8111-111111111111",
  kind: "task-created",
  occurredAt: "2026-08-23T00:00:00.000Z",
  task: {
    id: "22222222-2222-4222-8222-222222222222",
    title: "  Build shared board  ",
    description: "Publish append-only events",
  },
};

describe("signed task event boundary", () => {
  it("parses and canonicalizes a task creation", () => {
    expect(parseTaskCreatedEvent(event)).toEqual({
      ...event,
      task: { ...event.task, title: "Build shared board" },
    });
  });

  it.each([
    null,
    {},
    { ...event, schemaVersion: 2 },
    { ...event, kind: "changed" },
    { ...event, eventId: "bad" },
    { ...event, eventId: 1 },
    { ...event, eventId: `x${event.eventId}` },
    { ...event, eventId: `${event.eventId}x` },
    { ...event, occurredAt: "bad" },
    { ...event, occurredAt: 1 },
    { ...event, task: null },
    { ...event, task: { ...event.task, id: "bad" } },
    { ...event, task: { ...event.task, id: 1 } },
    { ...event, task: { ...event.task, id: `x${event.task.id}` } },
    { ...event, task: { ...event.task, id: `${event.task.id}x` } },
    { ...event, task: { ...event.task, title: " " } },
    { ...event, task: { ...event.task, title: 1 } },
    { ...event, task: { ...event.task, description: 1 } },
  ])("rejects malformed authority %j", (input) => {
    expect(parseTaskCreatedEvent(input)).toBeUndefined();
  });
});

describe("Kanban projection", () => {
  const parsed = parseTaskCreatedEvent(event);
  if (parsed === undefined) throw new Error("fixture must parse");

  it("projects shared tasks deterministically in Backlog", () => {
    const second = {
      ...parsed,
      eventId: "33333333-3333-4333-8333-333333333333",
      task: {
        ...parsed.task,
        id: "00000000-0000-4000-8000-000000000000",
        title: "First by identity",
      },
    };
    const board = foldTaskEvents([parsed, second]);
    expect(board.mode).toBe("writable");
    expect(board.tasks.map((task) => task.title)).toEqual([
      "First by identity",
      "Build shared board",
    ]);
    expect(formatTaskBoard(board)).toContain(
      "Backlog | 22222222-2222-4222-8222-222222222222 | Build shared board",
    );
  });

  it("degrades read-only on duplicate event or task authority", () => {
    expect(foldTaskEvents([parsed, parsed])).toEqual({
      mode: "degraded-read-only",
      tasks: [
        {
          id: parsed.task.id,
          title: parsed.task.title,
          description: parsed.task.description,
          state: "Backlog",
          blocked: false,
        },
      ],
      failure: "duplicate task authority event",
    });
    expect(
      foldTaskEvents([
        parsed,
        { ...parsed, eventId: "33333333-3333-4333-8333-333333333333" },
      ]).mode,
    ).toBe("degraded-read-only");
    expect(
      foldTaskEvents([
        parsed,
        {
          ...parsed,
          task: {
            ...parsed.task,
            id: "33333333-3333-4333-8333-333333333333",
          },
        },
      ]).mode,
    ).toBe("degraded-read-only");
  });

  it("formats degraded and blocked board evidence", () => {
    expect(
      formatTaskBoard({
        mode: "degraded-read-only",
        failure: "signature invalid",
        tasks: [
          {
            id: "22222222-2222-4222-8222-222222222222",
            title: "Blocked task",
            description: "",
            state: "Backlog",
            blocked: true,
          },
        ],
      }),
    ).toBe(
      "Task board: degraded-read-only\nFailure: signature invalid\nState | ID | Title\nBacklog [Blocked] | 22222222-2222-4222-8222-222222222222 | Blocked task",
    );
  });

  it("formats an empty writable board", () => {
    expect(formatTaskBoard(foldTaskEvents([]))).toBe(
      "Task board: writable\nState | ID | Title\n(no tasks)",
    );
  });
});
