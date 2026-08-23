import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  formatTaskBoard,
  parseTaskCreatedEvent,
  parseTaskEvent,
  type TaskCreatedEvent,
  type TaskClaimedEvent,
  type TaskClaimReleasedEvent,
  type TaskClaimTakenOverEvent,
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

const claimed: TaskClaimedEvent = {
  schemaVersion: 1,
  eventId: "55555555-5555-4555-8555-555555555555",
  kind: "task-claimed",
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  claim: {
    claimId: "66666666-6666-4666-8666-666666666666",
    owner: "developer@example.test",
    baselineRevision: "a".repeat(40),
    workflowDigest: `sha256:${"b".repeat(64)}`,
  },
};
const takenOver: TaskClaimTakenOverEvent = {
  schemaVersion: 1,
  eventId: "88888888-8888-4888-8888-888888888888",
  kind: "task-claim-taken-over",
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  previousClaimId: claimed.claim.claimId,
  claim: {
    ...claimed.claim,
    claimId: "99999999-9999-4999-8999-999999999999",
    owner: "takeover@example.test",
  },
};
const released: TaskClaimReleasedEvent = {
  schemaVersion: 1,
  eventId: "77777777-7777-4777-8777-777777777777",
  kind: "task-claim-released",
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  claimId: claimed.claim.claimId,
  reason: "baseline-drift",
};

describe("exclusive claims", () => {
  const created = parseTaskCreatedEvent(event);
  if (created === undefined) throw new Error("fixture must parse");

  it("parses, publishes, and releases one state-bound claim", () => {
    expect(parseTaskEvent(claimed)).toEqual(claimed);
    expect(
      parseTaskEvent({
        ...claimed,
        claim: { ...claimed.claim, owner: "  developer@example.test  " },
      }),
    ).toEqual(claimed);
    expect(parseTaskEvent(released)).toEqual(released);
    expect(parseTaskEvent({ ...released, reason: "released" })).toEqual({
      ...released,
      reason: "released",
    });
    expect(parseTaskEvent({ ...released, reason: "completed" })).toEqual({
      ...released,
      reason: "completed",
    });
    const inProgress = foldTaskEvents([created, specified, ready, claimed]);
    expect(inProgress.tasks[0]).toMatchObject({
      state: "In Progress",
      claim: claimed.claim,
    });
    const backToReady = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      released,
    ]);
    expect(backToReady.tasks[0]).toEqual({
      id: created.task.id,
      title: created.task.title,
      description: created.task.description,
      state: "Ready",
      blocked: false,
      specification,
      specificationDigest: digest,
    });
  });

  it("allows only an exact human-published claim takeover", () => {
    expect(parseTaskEvent(takenOver)).toEqual(takenOver);
    expect(
      parseTaskEvent({
        ...takenOver,
        claim: { ...takenOver.claim, owner: "  takeover@example.test  " },
      }),
    ).toEqual(takenOver);
    const board = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      takenOver,
    ]);
    expect(board.tasks[0]).toMatchObject({
      state: "In Progress",
      claim: takenOver.claim,
    });
    for (const invalid of [
      { ...takenOver, previousClaimId: takenOver.claim.claimId },
      {
        ...takenOver,
        previousClaimId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      },
      { ...takenOver, specificationDigest: `sha256:${"c".repeat(64)}` },
      {
        ...takenOver,
        claim: { ...takenOver.claim, baselineRevision: "c".repeat(40) },
      },
      {
        ...takenOver,
        claim: {
          ...takenOver.claim,
          workflowDigest: `sha256:${"d".repeat(64)}`,
        },
      },
      {
        ...takenOver,
        claim: { ...takenOver.claim, claimId: claimed.claim.claimId },
      },
    ]) {
      const denied = foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        invalid,
      ]);
      expect(denied.tasks).toHaveLength(1);
      expect(denied).toMatchObject({
        mode: "degraded-read-only",
        failure: "task claim takeover is not exact or state-bound",
      });
    }
  });

  it.each([
    { ...takenOver, kind: "unknown" },
    { ...takenOver, previousClaimId: "bad" },
    { ...takenOver, previousClaimId: `x${takenOver.previousClaimId}` },
    { ...takenOver, previousClaimId: `${takenOver.previousClaimId}x` },
    { ...takenOver, previousClaimId: 1 },
    { ...takenOver, claim: null },
    { ...takenOver, claim: { ...takenOver.claim, claimId: "bad" } },
    {
      ...takenOver,
      claim: { ...takenOver.claim, claimId: `x${takenOver.claim.claimId}` },
    },
    {
      ...takenOver,
      claim: { ...takenOver.claim, claimId: `${takenOver.claim.claimId}x` },
    },
    { ...takenOver, claim: { ...takenOver.claim, claimId: 1 } },
    { ...takenOver, claim: { ...takenOver.claim, owner: " " } },
    { ...takenOver, claim: { ...takenOver.claim, owner: 1 } },
    { ...takenOver, claim: { ...takenOver.claim, baselineRevision: "bad" } },
    {
      ...takenOver,
      claim: {
        ...takenOver.claim,
        baselineRevision: `x${takenOver.claim.baselineRevision}`,
      },
    },
    {
      ...takenOver,
      claim: {
        ...takenOver.claim,
        baselineRevision: `${takenOver.claim.baselineRevision}x`,
      },
    },
    { ...takenOver, claim: { ...takenOver.claim, baselineRevision: 1 } },
    { ...takenOver, claim: { ...takenOver.claim, workflowDigest: "bad" } },
    {
      ...takenOver,
      claim: {
        ...takenOver.claim,
        workflowDigest: `x${takenOver.claim.workflowDigest}`,
      },
    },
    {
      ...takenOver,
      claim: {
        ...takenOver.claim,
        workflowDigest: `${takenOver.claim.workflowDigest}x`,
      },
    },
    { ...takenOver, claim: { ...takenOver.claim, workflowDigest: 1 } },
  ])("rejects malformed takeover authority %j", (input) => {
    expect(parseTaskEvent(input)).toBeUndefined();
  });

  it("denies a second, stale, or mismatched claim transition", () => {
    const duplicateClaim = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      { ...claimed, eventId: "88888888-8888-4888-8888-888888888888" },
    ]);
    expect(duplicateClaim.tasks).toHaveLength(1);
    expect(duplicateClaim).toMatchObject({
      mode: "degraded-read-only",
      failure: "task claim is not exclusive or state-bound",
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        { ...claimed, specificationDigest: `sha256:${"c".repeat(64)}` },
      ]),
    ).toMatchObject({ mode: "degraded-read-only" });
    const releaseWithoutClaim = foldTaskEvents([
      created,
      specified,
      ready,
      released,
    ]);
    expect(releaseWithoutClaim.tasks).toHaveLength(1);
    expect(releaseWithoutClaim).toMatchObject({
      mode: "degraded-read-only",
      failure: "task claim release does not match the active claim",
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        { ...released, claimId: "99999999-9999-4999-8999-999999999999" },
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: "task claim release does not match the active claim",
    });
  });

  it.each([
    { ...claimed, kind: "unknown" },
    { ...claimed, claim: null },
    { ...claimed, claim: { ...claimed.claim, claimId: "bad" } },
    {
      ...claimed,
      claim: { ...claimed.claim, claimId: `x${claimed.claim.claimId}` },
    },
    {
      ...claimed,
      claim: { ...claimed.claim, claimId: `${claimed.claim.claimId}x` },
    },
    { ...claimed, claim: { ...claimed.claim, owner: " " } },
    { ...claimed, claim: { ...claimed.claim, owner: 1 } },
    { ...claimed, claim: { ...claimed.claim, baselineRevision: "bad" } },
    {
      ...claimed,
      claim: {
        ...claimed.claim,
        baselineRevision: `x${claimed.claim.baselineRevision}`,
      },
    },
    {
      ...claimed,
      claim: {
        ...claimed.claim,
        baselineRevision: `${claimed.claim.baselineRevision}x`,
      },
    },
    { ...claimed, claim: { ...claimed.claim, workflowDigest: "bad" } },
    {
      ...claimed,
      claim: {
        ...claimed.claim,
        workflowDigest: `x${claimed.claim.workflowDigest}`,
      },
    },
    {
      ...claimed,
      claim: {
        ...claimed.claim,
        workflowDigest: `${claimed.claim.workflowDigest}x`,
      },
    },
    { ...released, claimId: "bad" },
    { ...released, claimId: `x${released.claimId}` },
    { ...released, claimId: `${released.claimId}x` },
    { ...released, reason: "stolen" },
  ])("rejects malformed claim event %j", (candidate) => {
    expect(parseTaskEvent(candidate)).toBeUndefined();
  });
});

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
