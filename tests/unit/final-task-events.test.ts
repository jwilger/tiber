import { describe, expect, it } from "vitest";

import { parseTaskEvent } from "../../src/core/tasks/task-board.js";

const digest = `sha256:${"a".repeat(64)}`;
const snapshot = `sha256:${"b".repeat(64)}`;
const diagnostic = `sha256:${"c".repeat(64)}`;
const base = {
  schemaVersion: 1,
  eventId: "11111111-1111-4111-8111-111111111111",
  kind: "task-final-review-recorded",
  occurredAt: "2026-08-23T00:00:00.000Z",
  taskId: "22222222-2222-4222-8222-222222222222",
  specificationDigest: digest,
  verification: {
    claimId: "33333333-3333-4333-8333-333333333333",
    specificationDigest: digest,
    commandCatalogDigest: `sha256:${"d".repeat(64)}`,
    diagnosticDigest: diagnostic,
    sourceSnapshotDigest: snapshot,
  },
  iteration: {
    sourceSnapshotDigest: snapshot,
    verificationDiagnosticDigest: diagnostic,
    selectedLenses: ["behavior"],
    reviews: [
      {
        lens: "behavior",
        contextFreshness: "fresh",
        findingCount: 0,
        rationale: "The complete behavior is correct and tested.",
      },
    ],
  },
};

function finalReview(lens: string) {
  return {
    ...base,
    iteration: {
      ...base.iteration,
      selectedLenses: [lens],
      reviews: [{ ...base.iteration.reviews[0], lens }],
    },
  };
}

describe("final task event semantic boundary", () => {
  it("parses every closed final review lens inside the boundary observation", () => {
    for (const lens of ["behavior", "architecture", "security", "operability"])
      expect(parseTaskEvent(finalReview(lens))).toEqual({
        ok: true,
        value: finalReview(lens),
      });
  });

  it("rejects each malformed final review component independently", () => {
    const firstReview = base.iteration.reviews[0];
    if (firstReview === undefined) throw new Error("missing review fixture");
    const candidates = [
      null,
      { ...base, kind: "unknown" },
      { ...base, iteration: null },
      { ...base, iteration: [] },
      { ...base, iteration: { ...base.iteration, selectedLenses: null } },
      { ...base, iteration: { ...base.iteration, reviews: null } },
      {
        ...base,
        iteration: { ...base.iteration, sourceSnapshotDigest: "bad" },
      },
      {
        ...base,
        iteration: { ...base.iteration, verificationDiagnosticDigest: "bad" },
      },
      {
        ...base,
        iteration: { ...base.iteration, selectedLenses: ["unknown"] },
      },
      { ...base, iteration: { ...base.iteration, reviews: [null] } },
      {
        ...base,
        iteration: {
          ...base.iteration,
          reviews: [firstReview, null],
        },
      },
      {
        ...base,
        iteration: {
          ...base.iteration,
          reviews: [{ ...firstReview, lens: "unknown" }],
        },
      },
      {
        ...base,
        iteration: {
          ...base.iteration,
          reviews: [{ ...firstReview, contextFreshness: "stale" }],
        },
      },
      {
        ...base,
        iteration: {
          ...base.iteration,
          reviews: [{ ...firstReview, findingCount: -1 }],
        },
      },
      {
        ...base,
        iteration: {
          ...base.iteration,
          reviews: [{ ...firstReview, rationale: "short" }],
        },
      },
      { ...base, verification: null },
      { ...base, verification: { ...base.verification, claimId: "bad" } },
      {
        ...base,
        verification: { ...base.verification, specificationDigest: snapshot },
      },
      {
        ...base,
        verification: { ...base.verification, commandCatalogDigest: "bad" },
      },
      {
        ...base,
        verification: { ...base.verification, diagnosticDigest: "bad" },
      },
      {
        ...base,
        verification: { ...base.verification, sourceSnapshotDigest: "bad" },
      },
    ];
    for (const candidate of candidates)
      expect(parseTaskEvent(candidate)).toMatchObject({ ok: false });
  });

  it("parses only the three closed claim release reasons", () => {
    const release = {
      schemaVersion: 1,
      eventId: "56565656-5656-4656-8656-565656565656",
      kind: "task-claim-released",
      occurredAt: base.occurredAt,
      taskId: base.taskId,
      specificationDigest: digest,
      claimId: base.verification.claimId,
      reason: "released",
    };
    for (const reason of ["baseline-drift", "released", "completed"])
      expect(parseTaskEvent({ ...release, reason })).toMatchObject({
        ok: true,
        value: { reason },
      });
    for (const reason of ["", "stolen"])
      expect(parseTaskEvent({ ...release, reason })).toMatchObject({
        ok: false,
      });
  });

  it("parses exact clean task completion and rejects every malformed field", () => {
    const completion = {
      schemaVersion: 1,
      eventId: "44444444-4444-4444-8444-444444444444",
      kind: "task-completed",
      occurredAt: base.occurredAt,
      taskId: base.taskId,
      specificationDigest: digest,
      claimId: base.verification.claimId,
      sourceSnapshotDigest: snapshot,
      cleanup: {
        processCleanupStatus: "clean",
        worktreeCleanupStatus: "clean",
      },
    };
    expect(parseTaskEvent(completion)).toEqual({ ok: true, value: completion });
    for (const candidate of [
      { ...completion, kind: "unknown" },
      { ...completion, claimId: "bad" },
      { ...completion, sourceSnapshotDigest: "bad" },
      { ...completion, cleanup: null },
      {
        ...completion,
        cleanup: { ...completion.cleanup, processCleanupStatus: "dirty" },
      },
      {
        ...completion,
        cleanup: { ...completion.cleanup, worktreeCleanupStatus: "dirty" },
      },
    ])
      expect(parseTaskEvent(candidate)).toMatchObject({ ok: false });
  });
});
