import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  formatTaskBoard,
  parseTaskCreatedEvent,
  parseTaskEvent,
  type TaskCreatedEvent,
  type TaskReadyEvent,
  type TaskSpecifiedEvent,
} from "../../src/core/tasks/task-board.js";
import {
  digestTaskSpecification,
  type TaskSpecification,
} from "../../src/core/tasks/readiness.js";

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

const specification: TaskSpecification = {
  outcome: "Deliver reviewed readiness",
  scenarios: [
    { name: "ready", given: ["complete"], when: ["reviewed"], then: ["Ready"] },
  ],
  acceptanceCriteria: ["shared Ready"],
  exclusions: ["no priority mutation"],
  dependencies: [],
  testMappings: ["readiness.test.ts"],
  architectureImplications: "Deterministic authority consumes review evidence.",
};
const digest = digestTaskSpecification(specification);

const specified: TaskSpecifiedEvent = {
  schemaVersion: 1,
  eventId: "33333333-3333-4333-8333-333333333333",
  kind: "task-specified",
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  specification,
};
const ready: TaskReadyEvent = {
  schemaVersion: 1,
  eventId: "44444444-4444-4444-8444-444444444444",
  kind: "task-ready",
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  review: {
    freshContext: true,
    reviewerRole: "specification-reviewer",
    findingCount: 0,
    reviewedSpecificationDigest: digest,
  },
};

describe("reviewed Ready events", () => {
  it("parses specification and exact clean review events", () => {
    expect(parseTaskEvent(specified)).toEqual(specified);
    expect(parseTaskEvent(ready)).toEqual(ready);
  });

  it("projects Ready only after the canonical specification and clean review", () => {
    const created = parseTaskCreatedEvent(event);
    if (created === undefined) throw new Error("fixture must parse");
    expect(foldTaskEvents([created, specified, ready]).tasks[0]?.state).toBe(
      "Ready",
    );
    expect(foldTaskEvents([created, ready])).toEqual({
      mode: "degraded-read-only",
      tasks: [
        {
          id: created.task.id,
          title: created.task.title,
          description: created.task.description,
          state: "Backlog",
          blocked: false,
        },
      ],
      failure: "Ready event lacks an exact clean specification review",
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        { ...ready, review: { ...ready.review, findingCount: 1 } },
      ]),
    ).toMatchObject({ mode: "degraded-read-only" });
    const staleDigest = `sha256:${"b".repeat(64)}`;
    expect(
      foldTaskEvents([
        created,
        specified,
        {
          ...ready,
          specificationDigest: staleDigest,
          review: {
            ...ready.review,
            reviewedSpecificationDigest: staleDigest,
          },
        },
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: "Ready event lacks an exact clean specification review",
    });
  });

  it.each([
    null,
    { ...specified, schemaVersion: 2 },
    { ...specified, eventId: 1 },
    { ...specified, eventId: "bad" },
    { ...specified, eventId: `x${specified.eventId}` },
    { ...specified, eventId: `${specified.eventId}x` },
    { ...specified, occurredAt: 1 },
    { ...specified, occurredAt: "bad" },
    { ...specified, taskId: 1 },
    { ...specified, taskId: "bad" },
    { ...specified, taskId: `x${specified.taskId}` },
    { ...specified, taskId: `${specified.taskId}x` },
    { ...specified, specificationDigest: 1 },
    { ...specified, specificationDigest: "bad" },
    { ...specified, specificationDigest: `x${digest}` },
    { ...specified, specificationDigest: `${digest}x` },
    { ...specified, specificationDigest: `sha256:${"b".repeat(64)}` },
    {
      ...ready,
      specificationDigest: `x${digest}`,
      review: { ...ready.review, reviewedSpecificationDigest: `x${digest}` },
    },
    {
      ...ready,
      specificationDigest: `${digest}x`,
      review: { ...ready.review, reviewedSpecificationDigest: `${digest}x` },
    },
    { ...specified, specification: null },
    { ...specified, kind: "unknown" },
    { ...ready, kind: "unknown" },
    { ...ready, review: null },
    { ...ready, review: { ...ready.review, freshContext: false } },
    { ...ready, review: { ...ready.review, reviewerRole: "other" } },
    { ...ready, review: { ...ready.review, findingCount: "0" } },
    { ...ready, review: { ...ready.review, findingCount: 1.5 } },
    { ...ready, review: { ...ready.review, findingCount: -1 } },
    {
      ...ready,
      review: {
        ...ready.review,
        reviewedSpecificationDigest: `sha256:${"b".repeat(64)}`,
      },
    },
  ])("rejects malformed or stale shared readiness evidence %j", (candidate) => {
    expect(parseTaskEvent(candidate)).toBeUndefined();
  });

  it("degrades when a shared event references an unknown task", () => {
    const created = parseTaskCreatedEvent(event);
    if (created === undefined) throw new Error("fixture must parse");
    expect(
      foldTaskEvents([
        created,
        { ...specified, taskId: "55555555-5555-4555-8555-555555555555" },
      ]),
    ).toEqual({
      mode: "degraded-read-only",
      tasks: [
        {
          id: created.task.id,
          title: created.task.title,
          description: created.task.description,
          state: "Backlog",
          blocked: false,
        },
      ],
      failure: "task event references an unknown task",
    });
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
      ]),
    ).toEqual({
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
