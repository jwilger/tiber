import { describe, expect, it } from "vitest";

import {
  foldTaskEvents,
  formatTaskBoard,
  parseTaskEvent,
  type TaskEvent,
} from "../../src/core/tasks/task-board.js";
import { digestTaskSpecification } from "../../src/core/tasks/readiness.js";
import { none, some } from "../../src/core/types/option.js";
import {
  expectedTaskBoardFailure as taskBoardFailure,
  expectedTaskEventParseFailure,
} from "../fixtures/failures.js";
import { requireTaskEvent } from "../fixtures/task-events.js";
import { requireTaskSpecification } from "../fixtures/task-specification.js";

const eventDocument = {
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
const event = requireTaskEvent(eventDocument, "task-created");

describe("signed task event boundary", () => {
  it("parses and canonicalizes a task creation", () => {
    expect(parseTaskEvent(eventDocument)).toEqual({
      ok: true,
      value: event,
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
    expect(parseTaskEvent(input)).toEqual({
      ok: false,
      failure: expectedTaskEventParseFailure(
        "Task event is malformed or violates its semantic invariants",
      ),
    });
  });
});

const specification = requireTaskSpecification({
  outcome: "Deliver reviewed readiness",
  scenarios: [
    { name: "ready", given: ["complete"], when: ["reviewed"], then: ["Ready"] },
    { name: "other", given: ["complete"], when: ["reviewed"], then: ["Other"] },
  ],
  acceptanceCriteria: ["shared Ready"],
  exclusions: ["no priority mutation"],
  dependencies: [],
  testMappings: ["readiness.test.ts", "other.test.ts"],
  architectureImplications: "Deterministic authority consumes review evidence.",
});
const digest = digestTaskSpecification(specification);

const specified = requireTaskEvent(
  {
    schemaVersion: 1,
    eventId: "33333333-3333-4333-8333-333333333333",
    kind: "task-specified",
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    specification,
  },
  "task-specified",
);
const ready = requireTaskEvent(
  {
    schemaVersion: 1,
    eventId: "44444444-4444-4444-8444-444444444444",
    kind: "task-ready",
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    review: {
      contextFreshness: "fresh",
      reviewerRole: "specification-reviewer",
      findingCount: 0,
      reviewedSpecificationDigest: digest,
    },
  },
  "task-ready",
);

const claimed = requireTaskEvent(
  {
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
  },
  "task-claimed",
);
const incrementPreserved = requireTaskEvent(
  {
    schemaVersion: 1,
    eventId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    kind: "task-increment-preserved",
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    increment: {
      scenarioName: "ready",
      testMapping: "readiness.test.ts",
      baselineRevision: claimed.claim.baselineRevision,
      commandCatalogDigest: `sha256:${"f".repeat(64)}`,
      commandName: "unit-tests",
      redDiagnosticDigest: `sha256:${"b".repeat(64)}`,
      greenDiagnosticDigest: `sha256:${"c".repeat(64)}`,
      sourceDiffDigest: `sha256:${"d".repeat(64)}`,
      reviewRationale: "The increment is minimal and scenario-focused.",
    },
  },
  "task-increment-preserved",
);
const secondIncrement = requireTaskEvent(
  {
    ...incrementPreserved,
    eventId: "abababab-abab-4bab-8bab-abababababab",
    increment: {
      ...incrementPreserved.increment,
      scenarioName: "other",
      testMapping: "other.test.ts",
      sourceDiffDigest: `sha256:${"e".repeat(64)}`,
    },
  },
  "task-increment-preserved",
);
const finalReviewDocument = {
  schemaVersion: 1 as const,
  eventId: "bcbcbcbc-bcbc-4bcb-8bcb-bcbcbcbcbcbc",
  kind: "task-final-review-recorded" as const,
  occurredAt: event.occurredAt,
  taskId: event.task.id,
  specificationDigest: digest,
  verification: {
    claimId: claimed.claim.claimId,
    specificationDigest: digest,
    commandCatalogDigest: `sha256:${"f".repeat(64)}`,
    diagnosticDigest: `sha256:${"1".repeat(64)}`,
    sourceSnapshotDigest: `sha256:${"2".repeat(64)}`,
  },
  iteration: {
    sourceSnapshotDigest: `sha256:${"2".repeat(64)}`,
    verificationDiagnosticDigest: `sha256:${"1".repeat(64)}`,
    selectedLenses: ["behavior", "architecture"],
    reviews: [
      {
        lens: "behavior",
        contextFreshness: "fresh",
        findingCount: 0,
        rationale: "All acceptance behavior is complete and correct.",
      },
      {
        lens: "architecture",
        contextFreshness: "fresh",
        findingCount: 0,
        rationale: "The architecture remains coherent and compliant.",
      },
    ],
  },
};
const takenOver = requireTaskEvent(
  {
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
  },
  "task-claim-taken-over",
);
const released = requireTaskEvent(
  {
    schemaVersion: 1,
    eventId: "77777777-7777-4777-8777-777777777777",
    kind: "task-claim-released",
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    reason: "baseline-drift",
  },
  "task-claim-released",
);

function createdEvent(value: unknown) {
  const parsed = parseTaskEvent(value);
  if (!parsed.ok || parsed.value.kind !== "task-created")
    throw new Error("task creation fixture must parse");
  return parsed.value;
}

function parsedEvent(value: unknown): TaskEvent {
  const parsed = parseTaskEvent(value);
  if (!parsed.ok) throw new Error("task event fixture must parse");
  return parsed.value;
}

describe("signed final completion authority", () => {
  const created = createdEvent(event);
  const review = parsedEvent(finalReviewDocument);
  const secondReview = parsedEvent({
    ...finalReviewDocument,
    eventId: "cacacaca-caca-4aca-8aca-cacacacacaca",
  });
  const thirdReview = parsedEvent({
    ...finalReviewDocument,
    eventId: "dadadada-dada-4ada-8ada-dadadadadada",
  });
  const completedRelease = parsedEvent({
    ...released,
    eventId: "eaeaeaea-eaea-4aea-8aea-eaeaeaeaeaea",
    reason: "completed",
  });
  const deliveryDocument = {
    schemaVersion: 1 as const,
    eventId: "15151515-1515-4515-8515-151515151515",
    kind: "task-delivery-recorded" as const,
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    receipt: {
      mode: "branch-push" as const,
      baselineRevision: claimed.claim.baselineRevision,
      commit: "1".repeat(40),
      tree: "2".repeat(40),
      sourceSnapshotDigest:
        finalReviewDocument.verification.sourceSnapshotDigest,
      destination: { kind: "some" as const, value: "refs/heads/feature/task" },
      observedRemoteCommit: {
        kind: "some" as const,
        value: "1".repeat(40),
      },
    },
  };
  const ciDocument = {
    schemaVersion: 1 as const,
    eventId: "16161616-1616-4616-8616-161616161616",
    kind: "task-ci-recorded" as const,
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    receipt: {
      revision: deliveryDocument.receipt.commit,
      requiredAuthorities: ["quality", "acceptance"],
      observations: [
        {
          authority: "quality",
          revision: deliveryDocument.receipt.commit,
          status: "success" as const,
          adapterDigest: "5".repeat(64),
          observationDigest: "3".repeat(64),
        },
        {
          authority: "acceptance",
          revision: deliveryDocument.receipt.commit,
          status: "success" as const,
          adapterDigest: "6".repeat(64),
          observationDigest: "4".repeat(64),
        },
      ],
    },
  };
  const reviewOpenedDocument = {
    schemaVersion: 1 as const,
    eventId: "27272727-2727-4727-8727-272727272727",
    kind: "task-review-opened" as const,
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    review: {
      kind: "ordinary" as const,
      request: {
        repositoryOwner: "owner",
        repositoryName: "repository",
        headRef: deliveryDocument.receipt.destination.value,
        headRevision: deliveryDocument.receipt.commit,
        baseRef: "main",
        title: "feat(review): add service delivery",
        body: "Exact reviewed delivery.",
      },
      pullRequest: {
        number: 42,
        nodeId: "PR_node",
        url: "https://github.com/owner/repository/pull/42",
        author: "author",
        headRevision: deliveryDocument.receipt.commit,
      },
    },
  };
  const reviewRecordedDocument = {
    schemaVersion: 1 as const,
    eventId: "28282828-2828-4828-8828-282828282828",
    kind: "task-review-recorded" as const,
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    pullRequestNumber: 42,
    receipt: {
      observation: {
        headRevision: deliveryDocument.receipt.commit,
        reviewStatus: "approved" as const,
        conversationStatus: "resolved" as const,
        ciStatus: "success" as const,
        authorMergePermission: "granted" as const,
      },
      mergeStatus: "open" as const,
      disposition: "auto-merge-enabled" as const,
    },
  };
  const completed = parsedEvent({
    schemaVersion: 1,
    eventId: "fafafafa-fafa-4afa-8afa-fafafafafafa",
    kind: "task-completed",
    occurredAt: event.occurredAt,
    taskId: event.task.id,
    specificationDigest: digest,
    claimId: claimed.claim.claimId,
    sourceSnapshotDigest: finalReviewDocument.verification.sourceSnapshotDigest,
    cleanup: {
      processCleanupStatus: "clean",
      worktreeCleanupStatus: "clean",
    },
  });

  it.each(["behavior", "architecture", "security", "operability"] as const)(
    "parses the closed %s final review lens",
    (lens) => {
      expect(
        parseTaskEvent({
          ...finalReviewDocument,
          iteration: {
            ...finalReviewDocument.iteration,
            selectedLenses: [lens],
            reviews: [
              {
                ...finalReviewDocument.iteration.reviews[0],
                lens,
              },
            ],
          },
        }),
      ).toMatchObject({
        ok: true,
        value: {
          iteration: {
            selectedLenses: [lens],
            reviews: [{ lens }],
          },
        },
      });
    },
  );

  it("parses complete final review and cleanup authority exactly", () => {
    expect(parsedEvent(finalReviewDocument)).toEqual(finalReviewDocument);
    expect(completed).toEqual({
      schemaVersion: 1,
      eventId: "fafafafa-fafa-4afa-8afa-fafafafafafa",
      kind: "task-completed",
      occurredAt: event.occurredAt,
      taskId: event.task.id,
      specificationDigest: digest,
      claimId: claimed.claim.claimId,
      sourceSnapshotDigest:
        finalReviewDocument.verification.sourceSnapshotDigest,
      cleanup: {
        processCleanupStatus: "clean",
        worktreeCleanupStatus: "clean",
      },
    });
  });

  it.each([
    { ...finalReviewDocument, verification: null },
    {
      ...finalReviewDocument,
      verification: { ...finalReviewDocument.verification, claimId: "bad" },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        specificationDigest: `sha256:${"9".repeat(64)}`,
      },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        commandCatalogDigest: "bad",
      },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        diagnosticDigest: "bad",
      },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        sourceSnapshotDigest: "bad",
      },
    },
    { ...finalReviewDocument, iteration: null },
    {
      ...finalReviewDocument,
      iteration: { ...finalReviewDocument.iteration, selectedLenses: null },
    },
    {
      ...finalReviewDocument,
      iteration: { ...finalReviewDocument.iteration, reviews: null },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        sourceSnapshotDigest: "bad",
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        verificationDiagnosticDigest: "bad",
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        selectedLenses: ["behavior", "unknown"],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [null],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [
          { ...finalReviewDocument.iteration.reviews[0], lens: "unknown" },
        ],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [
          {
            ...finalReviewDocument.iteration.reviews[0],
            contextFreshness: "stale",
          },
        ],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [
          {
            ...finalReviewDocument.iteration.reviews[0],
            findingCount: -1,
          },
        ],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [
          { ...finalReviewDocument.iteration.reviews[0], rationale: "short" },
        ],
      },
    },
  ])("rejects malformed final review evidence %#", (candidate) => {
    expect(parseTaskEvent(candidate)).toMatchObject({ ok: false });
  });

  it.each([
    { cleanup: null },
    { claimId: "bad" },
    { sourceSnapshotDigest: "bad" },
    {
      cleanup: {
        processCleanupStatus: "dirty",
        worktreeCleanupStatus: "clean",
      },
    },
    {
      cleanup: {
        processCleanupStatus: "clean",
        worktreeCleanupStatus: "dirty",
      },
    },
  ])("rejects malformed completion evidence %#", (change) => {
    expect(
      parseTaskEvent({
        schemaVersion: 1,
        eventId: "fafafafa-fafa-4afa-8afa-fafafafafafa",
        kind: "task-completed",
        occurredAt: event.occurredAt,
        taskId: event.task.id,
        specificationDigest: digest,
        claimId: claimed.claim.claimId,
        sourceSnapshotDigest:
          finalReviewDocument.verification.sourceSnapshotDigest,
        cleanup: {
          processCleanupStatus: "clean",
          worktreeCleanupStatus: "clean",
        },
        ...change,
      }),
    ).toMatchObject({ ok: false });
  });

  it("prevents partial scenario completion from entering final review", () => {
    const result = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      review,
    ]);
    expect(result.failure).toEqual(
      some(
        taskBoardFailure(
          "incomplete-final-review",
          "final review is incomplete, stale, or not state-bound",
        ),
      ),
    );
    expect(result.tasks).toHaveLength(1);
    expect(result).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "incomplete-final-review" } },
      },
    });
  });

  it("resets the persisted streak on findings and source deltas", () => {
    const finding = parsedEvent({
      ...finalReviewDocument,
      eventId: "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd",
      iteration: {
        ...finalReviewDocument.iteration,
        reviews: [
          {
            ...finalReviewDocument.iteration.reviews[0],
            findingCount: 1,
          },
          finalReviewDocument.iteration.reviews[1],
        ],
      },
    });
    const changed = parsedEvent({
      ...finalReviewDocument,
      eventId: "dededede-dede-4ede-8ede-dededededede",
      verification: {
        ...finalReviewDocument.verification,
        sourceSnapshotDigest: `sha256:${"3".repeat(64)}`,
      },
      iteration: {
        ...finalReviewDocument.iteration,
        sourceSnapshotDigest: `sha256:${"3".repeat(64)}`,
      },
    });
    const prefix = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
    ];
    expect(foldTaskEvents([...prefix, finding]).tasks[0]).toMatchObject({
      finalReviewProgress: { kind: "some", value: { cleanStreak: 0 } },
    });
    expect(foldTaskEvents([...prefix, changed]).tasks[0]).toMatchObject({
      finalReviewProgress: { kind: "some", value: { cleanStreak: 1 } },
    });
  });

  it.each([
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        claimId: "76767676-7676-4676-8676-767676767676",
      },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        sourceSnapshotDigest: `sha256:${"7".repeat(64)}`,
      },
    },
    {
      ...finalReviewDocument,
      verification: {
        ...finalReviewDocument.verification,
        diagnosticDigest: `sha256:${"7".repeat(64)}`,
      },
    },
    {
      ...finalReviewDocument,
      specificationDigest: `sha256:${"6".repeat(64)}`,
      verification: {
        ...finalReviewDocument.verification,
        specificationDigest: `sha256:${"6".repeat(64)}`,
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        selectedLenses: ["behavior"],
        reviews: [finalReviewDocument.iteration.reviews[0]],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        selectedLenses: ["behavior", "architecture", "operability"],
        reviews: [
          ...finalReviewDocument.iteration.reviews,
          {
            ...finalReviewDocument.iteration.reviews[0],
            lens: "operability",
          },
        ],
      },
    },
    {
      ...finalReviewDocument,
      iteration: {
        ...finalReviewDocument.iteration,
        selectedLenses: ["behavior", "security"],
        reviews: [
          finalReviewDocument.iteration.reviews[0],
          {
            ...finalReviewDocument.iteration.reviews[1],
            lens: "security",
          },
        ],
      },
    },
  ])("rejects unbound final review authority %#", (candidate) => {
    const badReview = parsedEvent(candidate);
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        incrementPreserved,
        secondIncrement,
        badReview,
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "incomplete-final-review" } },
      },
    });
  });

  it.each(["local-only", "branch-push", "direct", "review"] as const)(
    "parses the closed %s Git delivery mode",
    (deliveryMode) => {
      const local = deliveryMode === "local-only";
      const candidate = {
        ...deliveryDocument,
        receipt: {
          ...deliveryDocument.receipt,
          mode: deliveryMode,
          destination: local
            ? { kind: "none" as const }
            : deliveryDocument.receipt.destination,
          observedRemoteCommit: local
            ? { kind: "none" as const }
            : deliveryDocument.receipt.observedRemoteCommit,
        },
      };
      expect(parseTaskEvent(candidate)).toEqual({
        ok: true,
        value: candidate,
      });
    },
  );

  it("records one exact Git-only delivery receipt", () => {
    const delivery = parsedEvent(deliveryDocument);
    expect(delivery).toEqual(deliveryDocument);
    const prefix = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
      thirdReview,
    ];
    const recorded = foldTaskEvents([...prefix, delivery]);
    expect(recorded.tasks[0]).toMatchObject({
      delivery: {
        kind: "some",
        value: {
          commit: deliveryDocument.receipt.commit,
          observedRemoteCommit: {
            kind: "some",
            value: deliveryDocument.receipt.commit,
          },
        },
      },
    });
    const failureCases = [
      parsedEvent({
        ...deliveryDocument,
        eventId: "26262626-2626-4626-8626-262626262626",
        claimId: "27272727-2727-4727-8727-272727272727",
      }),
      parsedEvent({
        ...deliveryDocument,
        eventId: "28282828-2828-4828-8828-282828282828",
        specificationDigest: `sha256:${"8".repeat(64)}`,
      }),
      parsedEvent({
        ...deliveryDocument,
        eventId: "30303030-3030-4030-8030-303030303030",
        receipt: {
          ...deliveryDocument.receipt,
          baselineRevision: "3".repeat(40),
        },
      }),
      parsedEvent({
        ...deliveryDocument,
        eventId: "29292929-2929-4929-8929-292929292929",
        receipt: {
          ...deliveryDocument.receipt,
          sourceSnapshotDigest: `sha256:${"8".repeat(64)}`,
        },
      }),
    ];
    for (const invalid of failureCases)
      expect(foldTaskEvents([...prefix, invalid])).toMatchObject({
        mode: "degraded-read-only",
        failure: {
          kind: "some",
          value: { safeContext: { reason: "invalid-delivery-receipt" } },
        },
      });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        incrementPreserved,
        secondIncrement,
        delivery,
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-delivery-receipt" } },
      },
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        incrementPreserved,
        secondIncrement,
        review,
        secondReview,
        delivery,
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-delivery-receipt" } },
      },
    });
    const duplicateDelivery = parsedEvent({
      ...deliveryDocument,
      eventId: "25252525-2525-4525-8525-252525252525",
    });
    const duplicate = foldTaskEvents([...prefix, delivery, duplicateDelivery]);
    expect(duplicate.failure).toEqual(
      some(
        taskBoardFailure(
          "invalid-delivery-receipt",
          "delivery receipt is duplicate, stale, or not state-bound",
        ),
      ),
    );
    expect(duplicate.tasks).toHaveLength(1);
    expect(duplicate).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-delivery-receipt" } },
      },
    });
  });

  it("records exact success from every required CI authority", () => {
    const reviewedPrefix = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
      thirdReview,
    ];
    const delivery = parsedEvent(deliveryDocument);
    const ci = parsedEvent(ciDocument);
    expect(ci).toEqual(ciDocument);
    const board = foldTaskEvents([...reviewedPrefix, delivery, ci]);
    expect(board.mode).toBe("writable");
    expect(board.tasks[0]?.ci).toEqual(some(ciDocument.receipt));

    const wrongRevision = parsedEvent({
      ...ciDocument,
      eventId: "17171717-1717-4717-8717-171717171717",
      receipt: {
        ...ciDocument.receipt,
        revision: "5".repeat(40),
        observations: ciDocument.receipt.observations.map((observation) => ({
          ...observation,
          revision: "5".repeat(40),
        })),
      },
    });
    expect(
      foldTaskEvents([...reviewedPrefix, delivery, wrongRevision]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-ci-receipt" } },
      },
    });
  });

  it("rejects malformed CI receipts", () => {
    const candidates = [
      { ...ciDocument, kind: "unknown" },
      { ...ciDocument, receipt: null },
      { ...ciDocument, receipt: { ...ciDocument.receipt, extra: true } },
      { ...ciDocument, claimId: "bad" },
      { ...ciDocument, receipt: { ...ciDocument.receipt, revision: "bad" } },
      {
        ...ciDocument,
        receipt: { ...ciDocument.receipt, requiredAuthorities: null },
      },
      {
        ...ciDocument,
        receipt: { ...ciDocument.receipt, requiredAuthorities: ["bad_name"] },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          requiredAuthorities: ["quality"],
        },
      },
      { ...ciDocument, receipt: { ...ciDocument.receipt, observations: null } },
      {
        ...ciDocument,
        receipt: { ...ciDocument.receipt, observations: [] },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          requiredAuthorities: [],
          observations: [],
        },
      },
      {
        ...ciDocument,
        receipt: { ...ciDocument.receipt, observations: [null] },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0 ? { ...observation, extra: true } : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0 ? { ...observation, status: "pending" } : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0
                ? { ...observation, authority: "bad_name" }
                : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0 ? { ...observation, revision: "bad" } : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0
                ? { ...observation, adapterDigest: "bad" }
                : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: ciDocument.receipt.observations.map(
            (observation, index) =>
              index === 0
                ? { ...observation, observationDigest: "bad" }
                : observation,
          ),
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: [
            ciDocument.receipt.observations[0],
            ciDocument.receipt.observations[0],
          ],
        },
      },
      {
        ...ciDocument,
        receipt: {
          ...ciDocument.receipt,
          observations: [
            {
              ...ciDocument.receipt.observations[0],
              revision: "5".repeat(40),
            },
          ],
        },
      },
    ];
    for (const candidate of candidates)
      expect(parseTaskEvent(candidate).ok).toBe(false);
  });

  it("rejects duplicate and stale CI task authority", () => {
    const reviewedPrefix = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
      thirdReview,
    ];
    const delivery = parsedEvent(deliveryDocument);
    const ci = parsedEvent(ciDocument);
    const releasedBoard = foldTaskEvents([
      ...reviewedPrefix,
      delivery,
      released,
    ]);
    expect(releasedBoard.tasks[0]?.state).toBe("Ready");
    expect(releasedBoard.tasks[0]?.claim).toEqual(none);

    const invalidEvents = [
      [...reviewedPrefix, ci],
      [
        ...reviewedPrefix,
        delivery,
        parsedEvent({
          ...ciDocument,
          claimId: "77777777-7777-4777-8777-777777777777",
        }),
      ],
      [
        ...reviewedPrefix,
        delivery,
        parsedEvent({
          ...ciDocument,
          specificationDigest:
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        }),
      ],
      [
        ...reviewedPrefix,
        delivery,
        ci,
        {
          ...ci,
          eventId: parsedEvent({
            ...ciDocument,
            eventId: "18181818-1818-4818-8818-181818181818",
          }).eventId,
        },
      ],
      [...reviewedPrefix, delivery, released, ci],
    ];
    for (const events of invalidEvents) {
      const board = foldTaskEvents(events);
      expect(board.mode).toBe("degraded-read-only");
      expect(board.tasks).toHaveLength(1);
      expect(board.failure).toEqual(
        some(
          taskBoardFailure(
            "invalid-ci-receipt",
            "CI receipt is duplicate, stale, or not state-bound",
          ),
        ),
      );
    }
  });

  it("records an exact opened review and authorized auto-merge disposition", () => {
    const prefix = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
      thirdReview,
      parsedEvent({
        ...deliveryDocument,
        receipt: { ...deliveryDocument.receipt, mode: "review" },
      }),
      parsedEvent(ciDocument),
    ];
    const opened = parsedEvent(reviewOpenedDocument);
    const recorded = parsedEvent(reviewRecordedDocument);
    expect(opened).toEqual(reviewOpenedDocument);
    expect(recorded).toEqual(reviewRecordedDocument);
    if (recorded.kind !== "task-review-recorded")
      throw new Error("fixture event invalid");
    const board = foldTaskEvents([...prefix, opened, recorded]);
    expect(board.mode).toBe("writable");
    expect(board.tasks[0]).toMatchObject({
      openedReview: { kind: "some", value: { kind: "ordinary" } },
      reviewReceipt: {
        kind: "some",
        value: { disposition: "auto-merge-enabled" },
      },
    });
    const merged = parsedEvent({
      ...reviewRecordedDocument,
      eventId: "33333333-aaaa-4333-8333-333333333333",
      receipt: {
        ...reviewRecordedDocument.receipt,
        mergeStatus: "merged",
        disposition: "merged",
      },
    });
    const mergedBoard = foldTaskEvents([...prefix, opened, recorded, merged]);
    expect(mergedBoard.mode).toBe("writable");
    expect(mergedBoard.tasks[0]?.reviewReceipt).toMatchObject({
      kind: "some",
      value: { disposition: "merged" },
    });
    expect(foldTaskEvents([...prefix, completedRelease])).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "non-exact-claim-release" } },
      },
    });
    expect(
      foldTaskEvents([...prefix, opened, recorded, completedRelease]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "non-exact-claim-release" } },
      },
    });
    expect(
      foldTaskEvents([
        ...prefix,
        opened,
        recorded,
        merged,
        completedRelease,
        completed,
      ]).tasks[0],
    ).toMatchObject({ state: "Done" });

    const missingPermission = parsedEvent({
      ...reviewRecordedDocument,
      eventId: "29292929-2929-4929-8929-292929292929",
      receipt: {
        observation: {
          ...reviewRecordedDocument.receipt.observation,
          authorMergePermission: "missing",
        },
        mergeStatus: "open",
        disposition: "permission-missing",
      },
    });
    expect(foldTaskEvents([...prefix, opened, missingPermission]).mode).toBe(
      "writable",
    );
    const invalidDisposition = foldTaskEvents([
      ...prefix,
      opened,
      {
        ...recorded,
        receipt: { ...recorded.receipt, disposition: "human-merge-required" },
      },
    ]);
    expect(invalidDisposition.mode).toBe("degraded-read-only");
    expect(invalidDisposition.tasks).toHaveLength(1);
    expect(invalidDisposition.failure).toEqual(
      some(
        taskBoardFailure(
          "invalid-review-receipt",
          "review disposition is not authorized by exact gates",
        ),
      ),
    );
    const failedGate = parsedEvent({
      ...reviewRecordedDocument,
      eventId: "32323232-3232-4232-8232-323232323232",
      receipt: {
        observation: {
          ...reviewRecordedDocument.receipt.observation,
          ciStatus: "failure",
        },
        mergeStatus: "open",
        disposition: "permission-missing",
      },
    });
    expect(foldTaskEvents([...prefix, opened, failedGate])).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-review-receipt" } },
      },
    });

    const releaseRef =
      "refs/heads/release-please--branches--main--components--tiber";
    const releaseDelivery = parsedEvent({
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        mode: "review",
        destination: { kind: "some", value: releaseRef },
      },
    });
    const releaseOpened = parsedEvent({
      ...reviewOpenedDocument,
      review: {
        ...reviewOpenedDocument.review,
        kind: "release",
        request: {
          ...reviewOpenedDocument.review.request,
          headRef: releaseRef,
          title: "chore(main): release tiber 1.2.3",
        },
      },
    });
    const releaseRecorded = parsedEvent({
      ...reviewRecordedDocument,
      receipt: {
        ...reviewRecordedDocument.receipt,
        disposition: "human-merge-required",
      },
    });
    const releaseBoard = foldTaskEvents([
      ...prefix.slice(0, -2),
      releaseDelivery,
      parsedEvent(ciDocument),
      releaseOpened,
      releaseRecorded,
    ]);
    expect(releaseBoard.mode).toBe("writable");
    expect(releaseBoard.tasks[0]?.reviewReceipt).toMatchObject({
      kind: "some",
      value: { disposition: "human-merge-required" },
    });
    const releaseMerged = foldTaskEvents([
      ...prefix.slice(0, -2),
      releaseDelivery,
      parsedEvent(ciDocument),
      releaseOpened,
      releaseRecorded,
      merged,
    ]);
    expect(releaseMerged.mode).toBe("writable");
    expect(releaseMerged.tasks[0]?.reviewReceipt).toMatchObject({
      kind: "some",
      value: { disposition: "merged" },
    });

    for (const malformed of [
      { ...reviewOpenedDocument, claimId: "bad" },
      { ...reviewOpenedDocument, review: null },
      { ...reviewRecordedDocument, claimId: "bad" },
      { ...reviewRecordedDocument, pullRequestNumber: 0 },
      { ...reviewRecordedDocument, receipt: null },
    ])
      expect(parseTaskEvent(malformed).ok).toBe(false);

    const wrongClaimOpened = parsedEvent({
      ...reviewOpenedDocument,
      claimId: "77777777-7777-4777-8777-777777777777",
    });
    const wrongSpecOpened = parsedEvent({
      ...reviewOpenedDocument,
      specificationDigest:
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    });
    const wrongRefOpened = parsedEvent({
      ...reviewOpenedDocument,
      review: {
        ...reviewOpenedDocument.review,
        request: {
          ...reviewOpenedDocument.review.request,
          headRef: "refs/heads/other",
        },
      },
    });
    const otherRevision = "7".repeat(40);
    const wrongRevisionOpened = parsedEvent({
      ...reviewOpenedDocument,
      review: {
        ...reviewOpenedDocument.review,
        request: {
          ...reviewOpenedDocument.review.request,
          headRevision: otherRevision,
        },
        pullRequest: {
          ...reviewOpenedDocument.review.pullRequest,
          headRevision: otherRevision,
        },
      },
    });
    const duplicateOpened = parsedEvent({
      ...reviewOpenedDocument,
      eventId: "30303030-3030-4030-8030-303030303030",
    });
    const branchPrefix = [
      ...prefix.slice(0, -2),
      parsedEvent(deliveryDocument),
      parsedEvent(ciDocument),
    ];
    const noCiPrefix = prefix.slice(0, -1);
    const releasedPrefix = [...prefix, released];
    for (const history of [
      [...branchPrefix, opened],
      [...noCiPrefix, opened],
      [...prefix, wrongClaimOpened],
      [...prefix, wrongSpecOpened],
      [...prefix, wrongRefOpened],
      [...prefix, wrongRevisionOpened],
      [...prefix, opened, duplicateOpened],
      [...releasedPrefix, opened],
    ]) {
      const invalid = foldTaskEvents(history);
      expect(invalid.mode).toBe("degraded-read-only");
      expect(invalid.tasks).toHaveLength(1);
      expect(invalid.failure).toEqual(
        some(
          taskBoardFailure(
            "invalid-review-receipt",
            "opened review is duplicate, stale, or not state-bound",
          ),
        ),
      );
    }

    const wrongClaimRecorded = parsedEvent({
      ...reviewRecordedDocument,
      claimId: "77777777-7777-4777-8777-777777777777",
    });
    const wrongSpecRecorded = parsedEvent({
      ...reviewRecordedDocument,
      specificationDigest:
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    });
    const wrongNumberRecorded = parsedEvent({
      ...reviewRecordedDocument,
      pullRequestNumber: 43,
    });
    const duplicateRecorded = parsedEvent({
      ...reviewRecordedDocument,
      eventId: "31313131-3131-4131-8131-313131313131",
    });
    const duplicateMerged = parsedEvent({
      ...reviewRecordedDocument,
      eventId: "34343434-3434-4434-8434-343434343434",
      receipt: {
        ...reviewRecordedDocument.receipt,
        mergeStatus: "merged",
        disposition: "merged",
      },
    });
    for (const history of [
      [...prefix, recorded],
      [...prefix, opened, wrongClaimRecorded],
      [...prefix, opened, wrongSpecRecorded],
      [...prefix, opened, wrongNumberRecorded],
      [...prefix, opened, recorded, duplicateRecorded],
      [...prefix, opened, recorded, merged, duplicateRecorded],
      [...prefix, opened, recorded, merged, duplicateMerged],
      [...prefix, opened, released, recorded],
    ]) {
      const invalid = foldTaskEvents(history);
      expect(invalid.mode).toBe("degraded-read-only");
      expect(invalid.tasks).toHaveLength(1);
      expect(invalid.failure).toEqual(
        some(
          taskBoardFailure(
            "invalid-review-receipt",
            "review gate receipt is duplicate, stale, or not state-bound",
          ),
        ),
      );
    }
  });

  it.each([
    { ...deliveryDocument, kind: "unknown" },
    { ...deliveryDocument, receipt: null },
    { ...deliveryDocument, claimId: "bad" },
    {
      ...deliveryDocument,
      receipt: { ...deliveryDocument.receipt, mode: "unknown" },
    },
    {
      ...deliveryDocument,
      receipt: { ...deliveryDocument.receipt, baselineRevision: "bad" },
    },
    {
      ...deliveryDocument,
      receipt: { ...deliveryDocument.receipt, commit: "bad" },
    },
    {
      ...deliveryDocument,
      receipt: { ...deliveryDocument.receipt, tree: "bad" },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        sourceSnapshotDigest: "bad",
      },
    },
    {
      ...deliveryDocument,
      receipt: { ...deliveryDocument.receipt, destination: null },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        destination: {
          kind: "unknown",
          value: "refs/heads/feature/task",
        },
      },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        destination: { kind: "some", value: "bad" },
      },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        observedRemoteCommit: {
          kind: "unknown",
          value: "1".repeat(40),
        },
      },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        observedRemoteCommit: { kind: "some", value: "bad" },
      },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        observedRemoteCommit: null,
      },
    },
    {
      ...deliveryDocument,
      receipt: {
        ...deliveryDocument.receipt,
        observedRemoteCommit: { kind: "none" },
      },
    },
  ])("rejects malformed delivery receipt %#", (candidate) => {
    expect(parseTaskEvent(candidate)).toMatchObject({ ok: false });
  });

  it("requires three clean exact iterations, completed release, and cleanup", () => {
    const events = [
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      secondIncrement,
      review,
      secondReview,
    ];
    expect(foldTaskEvents(events).tasks[0]?.finalReviewProgress).toMatchObject({
      kind: "some",
      value: { cleanStreak: 2 },
    });
    expect(foldTaskEvents([...events, released]).tasks[0]).toMatchObject({
      state: "Ready",
      finalReviewProgress: { kind: "none" },
      completionRelease: { kind: "none" },
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        incrementPreserved,
        secondIncrement,
        completedRelease,
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "non-exact-claim-release" } },
      },
    });
    expect(foldTaskEvents([...events, completedRelease])).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "non-exact-claim-release" } },
      },
    });
    const reviewedButNotCi = [...events, thirdReview];
    for (const incomplete of [
      [...reviewedButNotCi, completedRelease],
      [...reviewedButNotCi, parsedEvent(deliveryDocument), completedRelease],
    ]) {
      expect(foldTaskEvents(incomplete)).toMatchObject({
        mode: "degraded-read-only",
        failure: {
          kind: "some",
          value: { safeContext: { reason: "non-exact-claim-release" } },
        },
      });
    }
    const releasable = [
      ...reviewedButNotCi,
      parsedEvent(deliveryDocument),
      parsedEvent(ciDocument),
    ];
    const changedSnapshot = `sha256:${"6".repeat(64)}`;
    const changedReviews = [
      "35353535-3535-4535-8535-353535353535",
      "36363636-3636-4636-8636-363636363636",
      "37373737-3737-4737-8737-373737373737",
    ].map((eventId) =>
      parsedEvent({
        ...finalReviewDocument,
        eventId,
        verification: {
          ...finalReviewDocument.verification,
          sourceSnapshotDigest: changedSnapshot,
        },
        iteration: {
          ...finalReviewDocument.iteration,
          sourceSnapshotDigest: changedSnapshot,
        },
      }),
    );
    expect(
      foldTaskEvents([...releasable, ...changedReviews, completedRelease]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "non-exact-claim-release" } },
      },
    });
    const releasedForCompletion = [...releasable, completedRelease];
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        incrementPreserved,
        secondIncrement,
        released,
        completed,
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-task-completion" } },
      },
    });
    const prematureCompletion = foldTaskEvents([...releasable, completed]);
    expect(prematureCompletion.failure).toEqual(
      some(
        taskBoardFailure(
          "invalid-task-completion",
          "task completion lacks exact review, release, or cleanup evidence",
        ),
      ),
    );
    expect(prematureCompletion.tasks).toHaveLength(1);
    expect(prematureCompletion).toMatchObject({
      mode: "degraded-read-only",
      failure: {
        kind: "some",
        value: { safeContext: { reason: "invalid-task-completion" } },
      },
    });
    for (const invalidCompletion of [
      parsedEvent({
        ...completed,
        eventId: "26262626-2626-4626-8626-262626262626",
        specificationDigest: `sha256:${"6".repeat(64)}`,
      }),
      parsedEvent({
        ...completed,
        eventId: "17171717-1717-4717-8717-171717171717",
        claimId: "18181818-1818-4818-8818-181818181818",
      }),
      parsedEvent({
        ...completed,
        eventId: "19191919-1919-4919-8919-191919191919",
        sourceSnapshotDigest: `sha256:${"6".repeat(64)}`,
      }),
    ]) {
      expect(
        foldTaskEvents([...releasedForCompletion, invalidCompletion]),
      ).toMatchObject({
        mode: "degraded-read-only",
        failure: {
          kind: "some",
          value: { safeContext: { reason: "invalid-task-completion" } },
        },
      });
    }
    expect(
      foldTaskEvents([...releasedForCompletion, completed]).tasks[0],
    ).toMatchObject({ state: "Done", claim: { kind: "none" } });
  });
});

describe("exclusive claims", () => {
  const created = createdEvent(event);

  it("parses, publishes, and releases one state-bound claim", () => {
    expect(parsedEvent(claimed)).toEqual(claimed);
    expect(parsedEvent({ ...released, reason: "released" })).toEqual({
      ...released,
      reason: "released",
    });
    expect(parsedEvent({ ...released, reason: "completed" })).toEqual({
      ...released,
      reason: "completed",
    });
    expect(
      parsedEvent({
        ...claimed,
        claim: { ...claimed.claim, owner: "  developer@example.test  " },
      }),
    ).toEqual(claimed);
    expect(parsedEvent(released)).toEqual(released);
    expect(parsedEvent({ ...released, reason: "released" })).toEqual({
      ...released,
      reason: "released",
    });
    expect(parsedEvent({ ...released, reason: "completed" })).toEqual({
      ...released,
      reason: "completed",
    });
    const inProgress = foldTaskEvents([created, specified, ready, claimed]);
    expect(inProgress.tasks[0]).toMatchObject({
      state: "In Progress",
      claim: some(claimed.claim),
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
      blockStatus: "unblocked",
      specification: some(specification),
      specificationDigest: some(digest),
      claim: none,
      preservedIncrements: [],
      finalReviewProgress: none,
      completionRelease: none,
      delivery: none,
      ci: none,
      openedReview: none,
      reviewReceipt: none,
      campaignProvenance: none,
    });
  });

  it("preserves one signed scenario increment under the exact claim", () => {
    expect(parsedEvent(incrementPreserved)).toEqual(incrementPreserved);
    expect(
      parsedEvent({ ...claimed, increment: incrementPreserved.increment }),
    ).toEqual(claimed);
    expect(
      parsedEvent({ ...ready, increment: incrementPreserved.increment }),
    ).toEqual(ready);
    expect(
      parseTaskEvent({ ...incrementPreserved, claimId: "bad" }),
    ).toMatchObject({ ok: false });
    for (const increment of [
      { ...incrementPreserved.increment, scenarioName: "" },
      { ...incrementPreserved.increment, testMapping: "../outside" },
      { ...incrementPreserved.increment, baselineRevision: "bad" },
      { ...incrementPreserved.increment, commandCatalogDigest: "bad" },
      { ...incrementPreserved.increment, commandName: "Bad Command" },
      { ...incrementPreserved.increment, sourceDiffDigest: "bad" },
      { ...incrementPreserved.increment, redDiagnosticDigest: "bad" },
      { ...incrementPreserved.increment, greenDiagnosticDigest: "bad" },
      { ...incrementPreserved.increment, reviewRationale: "short" },
    ]) {
      expect(
        parseTaskEvent({ ...incrementPreserved, increment }),
      ).toMatchObject({
        ok: false,
      });
    }
    const prefix = [created, specified, ready, claimed] as const;
    expect(
      foldTaskEvents([created, specified, ready, incrementPreserved]),
    ).toMatchObject({ mode: "degraded-read-only" });
    for (const [eventId, candidate] of [
      [
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        { ...incrementPreserved, claimId: takenOver.claim.claimId },
      ],
      [
        "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        {
          ...incrementPreserved,
          specificationDigest: `sha256:${"0".repeat(64)}`,
        },
      ],
      [
        "12121212-1212-4212-8212-121212121212",
        {
          ...incrementPreserved,
          increment: {
            ...incrementPreserved.increment,
            baselineRevision: "0".repeat(40),
          },
        },
      ],
      [
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
        {
          ...incrementPreserved,
          increment: {
            ...incrementPreserved.increment,
            scenarioName: "missing",
          },
        },
      ],
      [
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        {
          ...incrementPreserved,
          increment: {
            ...incrementPreserved.increment,
            testMapping: "missing.test.ts",
          },
        },
      ],
    ] as const) {
      const invalid = requireTaskEvent(
        { ...candidate, eventId },
        "task-increment-preserved",
      );
      expect(foldTaskEvents([...prefix, invalid])).toMatchObject({
        mode: "degraded-read-only",
        failure: { kind: "some" },
      });
    }
    const projected = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
    ]);
    expect(projected.tasks[0]?.preservedIncrements).toEqual([
      incrementPreserved.increment,
    ]);
    const otherScenario = requireTaskEvent(
      {
        ...incrementPreserved,
        eventId: "99999999-9999-4999-8999-999999999998",
        increment: {
          ...incrementPreserved.increment,
          scenarioName: "other",
          testMapping: "other.test.ts",
          sourceDiffDigest: `sha256:${"e".repeat(64)}`,
        },
      },
      "task-increment-preserved",
    );
    expect(
      foldTaskEvents([...prefix, incrementPreserved, otherScenario]).tasks[0]
        ?.preservedIncrements,
    ).toEqual([incrementPreserved.increment, otherScenario.increment]);

    const duplicateScenario = requireTaskEvent(
      {
        ...incrementPreserved,
        eventId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      },
      "task-increment-preserved",
    );
    const duplicateBoard = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      incrementPreserved,
      duplicateScenario,
    ]);
    expect(duplicateBoard).toMatchObject({
      mode: "degraded-read-only",
      failure: some(
        taskBoardFailure(
          "invalid-preserved-increment",
          "preserved increment is not unique or state-bound",
        ),
      ),
    });
    expect(duplicateBoard.tasks).toEqual(projected.tasks);
  });

  it("allows only an exact human-published claim takeover", () => {
    expect(parsedEvent(takenOver)).toEqual(takenOver);
    expect(
      parsedEvent({
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
      claim: some(takenOver.claim),
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
        requireTaskEvent(invalid, "task-claim-taken-over"),
      ]);
      expect(denied.tasks).toHaveLength(1);
      expect(denied).toMatchObject({
        mode: "degraded-read-only",
        failure: some(
          taskBoardFailure(
            "non-exact-claim-takeover",
            "task claim takeover is not exact or state-bound",
          ),
        ),
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
    expect(parseTaskEvent(input)).toEqual({
      ok: false,
      failure: expectedTaskEventParseFailure(
        "Task event is malformed or violates its semantic invariants",
      ),
    });
  });

  it("denies a second, stale, or mismatched claim transition", () => {
    const duplicateClaim = foldTaskEvents([
      created,
      specified,
      ready,
      claimed,
      requireTaskEvent(
        { ...claimed, eventId: "88888888-8888-4888-8888-888888888888" },
        "task-claimed",
      ),
    ]);
    expect(duplicateClaim.tasks).toHaveLength(1);
    expect(duplicateClaim).toMatchObject({
      mode: "degraded-read-only",
      failure: some(
        taskBoardFailure(
          "non-exclusive-claim",
          "task claim is not exclusive or state-bound",
        ),
      ),
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        requireTaskEvent(
          { ...claimed, specificationDigest: `sha256:${"c".repeat(64)}` },
          "task-claimed",
        ),
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
      failure: some(
        taskBoardFailure(
          "non-exact-claim-release",
          "task claim release does not match the active claim",
        ),
      ),
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        ready,
        claimed,
        requireTaskEvent(
          { ...released, claimId: "99999999-9999-4999-8999-999999999999" },
          "task-claim-released",
        ),
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: some(
        taskBoardFailure(
          "non-exact-claim-release",
          "task claim release does not match the active claim",
        ),
      ),
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
    { ...released, reason: "" },
  ])("rejects malformed claim event %j", (candidate) => {
    expect(parseTaskEvent(candidate)).toEqual({
      ok: false,
      failure: expectedTaskEventParseFailure(
        "Task event is malformed or violates its semantic invariants",
      ),
    });
  });
});

describe("reviewed Ready events", () => {
  it("parses specification and exact clean review events", () => {
    expect(parsedEvent(specified)).toEqual(specified);
    expect(parsedEvent(ready)).toEqual(ready);
  });

  it("rejects specification amendments after Backlog authority has ended", () => {
    const created = createdEvent(event);
    const lateAmendment = requireTaskEvent(
      {
        ...specified,
        eventId: "34343434-3434-4434-8434-343434343434",
      },
      "task-specified",
    );

    expect(
      foldTaskEvents([created, specified, ready, lateAmendment]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: some(
        taskBoardFailure(
          "invalid-task-specification",
          "task specification can only change while the task is Backlog",
        ),
      ),
    });
  });

  it("projects Ready only after the canonical specification and clean review", () => {
    const created = createdEvent(event);
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
          blockStatus: "unblocked",
          specification: none,
          specificationDigest: none,
          claim: none,
          preservedIncrements: [],
          finalReviewProgress: none,
          completionRelease: none,
          delivery: none,
          ci: none,
          openedReview: none,
          reviewReceipt: none,
          campaignProvenance: none,
        },
      ],
      failure: some(
        taskBoardFailure(
          "stale-readiness-review",
          "Ready event lacks an exact clean specification review",
        ),
      ),
    });
    expect(
      foldTaskEvents([
        created,
        specified,
        requireTaskEvent(
          { ...ready, review: { ...ready.review, findingCount: 1 } },
          "task-ready",
        ),
      ]),
    ).toMatchObject({ mode: "degraded-read-only" });
    const staleDigest = `sha256:${"b".repeat(64)}`;
    expect(
      foldTaskEvents([
        created,
        specified,
        requireTaskEvent(
          {
            ...ready,
            specificationDigest: staleDigest,
            review: {
              ...ready.review,
              reviewedSpecificationDigest: staleDigest,
            },
          },
          "task-ready",
        ),
      ]),
    ).toMatchObject({
      mode: "degraded-read-only",
      failure: some(
        taskBoardFailure(
          "stale-readiness-review",
          "Ready event lacks an exact clean specification review",
        ),
      ),
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
    { ...ready, review: { ...ready.review, contextFreshness: "stale" } },
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
    expect(parseTaskEvent(candidate)).toEqual({
      ok: false,
      failure: expectedTaskEventParseFailure(
        "Task event is malformed or violates its semantic invariants",
      ),
    });
  });

  it("degrades when a shared event references an unknown task", () => {
    const created = createdEvent(event);
    expect(
      foldTaskEvents([
        created,
        requireTaskEvent(
          { ...specified, taskId: "55555555-5555-4555-8555-555555555555" },
          "task-specified",
        ),
      ]),
    ).toEqual({
      mode: "degraded-read-only",
      tasks: [
        {
          id: created.task.id,
          title: created.task.title,
          description: created.task.description,
          state: "Backlog",
          blockStatus: "unblocked",
          specification: none,
          specificationDigest: none,
          claim: none,
          preservedIncrements: [],
          finalReviewProgress: none,
          completionRelease: none,
          delivery: none,
          ci: none,
          openedReview: none,
          reviewReceipt: none,
          campaignProvenance: none,
        },
      ],
      failure: some(
        taskBoardFailure(
          "unknown-task",
          "task event references an unknown task",
        ),
      ),
    });
  });
});

describe("Kanban projection", () => {
  const parsed = createdEvent(event);

  it("projects shared tasks deterministically in Backlog", () => {
    const second = requireTaskEvent(
      {
        ...parsed,
        eventId: "33333333-3333-4333-8333-333333333333",
        task: {
          ...parsed.task,
          id: "00000000-0000-4000-8000-000000000000",
          title: "First by identity",
        },
      },
      "task-created",
    );
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
          blockStatus: "unblocked",
          specification: none,
          specificationDigest: none,
          claim: none,
          preservedIncrements: [],
          finalReviewProgress: none,
          completionRelease: none,
          delivery: none,
          ci: none,
          openedReview: none,
          reviewReceipt: none,
          campaignProvenance: none,
        },
      ],
      failure: some(
        taskBoardFailure(
          "duplicate-authority-event",
          "duplicate task authority event",
        ),
      ),
    });
    expect(
      foldTaskEvents([
        parsed,
        requireTaskEvent(
          { ...parsed, eventId: "33333333-3333-4333-8333-333333333333" },
          "task-created",
        ),
      ]),
    ).toEqual({
      mode: "degraded-read-only",
      tasks: [
        {
          id: parsed.task.id,
          title: parsed.task.title,
          description: parsed.task.description,
          state: "Backlog",
          blockStatus: "unblocked",
          specification: none,
          specificationDigest: none,
          claim: none,
          preservedIncrements: [],
          finalReviewProgress: none,
          completionRelease: none,
          delivery: none,
          ci: none,
          openedReview: none,
          reviewReceipt: none,
          campaignProvenance: none,
        },
      ],
      failure: some(
        taskBoardFailure(
          "duplicate-authority-event",
          "duplicate task authority event",
        ),
      ),
    });
    expect(
      foldTaskEvents([
        parsed,
        requireTaskEvent(
          {
            ...parsed,
            task: {
              ...parsed.task,
              id: "33333333-3333-4333-8333-333333333333",
            },
          },
          "task-created",
        ),
      ]).mode,
    ).toBe("degraded-read-only");
  });

  it("formats degraded and blocked board evidence", () => {
    const blocked = requireTaskEvent(
      {
        ...parsed,
        task: { ...parsed.task, title: "Blocked task" },
      },
      "task-created",
    );
    const blockedTask = foldTaskEvents([blocked]).tasks[0];
    if (blockedTask === undefined) throw new Error("missing blocked fixture");
    expect(
      formatTaskBoard({
        mode: "degraded-read-only",
        failure: some(
          taskBoardFailure("task-history-verification", "signature invalid"),
        ),
        tasks: [{ ...blockedTask, blockStatus: "blocked" }],
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
