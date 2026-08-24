import { describe, expect, it } from "vitest";

import { parseTaskEvent } from "../../src/core/tasks/task-board.js";

describe("task event semantic boundary", () => {
  it("canonicalizes a task title", () => {
    const result = parseTaskEvent({
      schemaVersion: 1,
      eventId: "11111111-1111-4111-8111-111111111111",
      kind: "task-created",
      occurredAt: "2026-08-23T00:00:00.000Z",
      task: {
        id: "22222222-2222-4222-8222-222222222222",
        title: "  Canonical title  ",
        description: "description",
      },
    });
    expect(result.ok ? result.value : undefined).toMatchObject({
      kind: "task-created",
      task: { title: "Canonical title" },
    });
  });

  it("parses a complete takeover without confusing another event kind", () => {
    expect(
      parseTaskEvent({
        schemaVersion: 1,
        eventId: "88888888-8888-4888-8888-888888888888",
        kind: "task-claim-taken-over",
        occurredAt: "2026-08-23T00:00:00.000Z",
        taskId: "22222222-2222-4222-8222-222222222222",
        specificationDigest: `sha256:${"a".repeat(64)}`,
        previousClaimId: "66666666-6666-4666-8666-666666666666",
        claim: {
          claimId: "99999999-9999-4999-8999-999999999999",
          owner: "takeover@example.test",
          baselineRevision: "a".repeat(40),
          workflowDigest: `sha256:${"b".repeat(64)}`,
        },
      }),
    ).toMatchObject({
      ok: true,
      value: {
        kind: "task-claim-taken-over",
        previousClaimId: "66666666-6666-4666-8666-666666666666",
        claim: { claimId: "99999999-9999-4999-8999-999999999999" },
      },
    });
  });
});
