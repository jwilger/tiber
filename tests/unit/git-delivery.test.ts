import { describe, expect, it } from "vitest";

import {
  authorizeGitDelivery,
  validateGitDeliveryReceipt,
} from "../../src/core/delivery/git-delivery.js";
import {
  parseDeliveryCommitRevision,
  parseDeliveryDestinationRef,
  parseDeliveryTreeDigest,
} from "../../src/core/delivery/git-delivery-values.js";
import { none, some } from "../../src/core/types/option.js";
import { claimBaselineRevision } from "../fixtures/task-values.js";
import {
  finalReviewFindingCount,
  finalReviewRationale,
  sourceSnapshotDigest,
  verificationDiagnosticDigest,
} from "../fixtures/workflow-values.js";

function required<Value>(result: { ok: true; value: Value } | { ok: false }) {
  if (!result.ok) throw new Error("invalid delivery fixture");
  return result.value;
}
const snapshot = sourceSnapshotDigest(`sha256:${"a".repeat(64)}`);
const progress = {
  sourceSnapshotDigest: snapshot,
  verificationDiagnosticDigest: verificationDiagnosticDigest(
    `sha256:${"b".repeat(64)}`,
  ),
  selectedLenses: ["behavior"] as const,
  reviews: [
    {
      lens: "behavior" as const,
      contextFreshness: "fresh" as const,
      findingCount: finalReviewFindingCount(0),
      rationale: finalReviewRationale("The complete behavior remains correct."),
    },
  ],
  cleanStreak: 3 as const,
};
const destination = required(
  parseDeliveryDestinationRef("refs/heads/feature/example"),
);
const commit = required(parseDeliveryCommitRevision("c".repeat(40)));
const tree = required(parseDeliveryTreeDigest("d".repeat(40)));

describe("generic Git delivery authority", () => {
  it.each(["branch-push", "direct", "review"] as const)(
    "authorizes exact reviewed %s delivery only with a destination",
    (mode) => {
      expect(
        authorizeGitDelivery({
          mode,
          destination: some(destination),
          reviewedProgress: progress,
          observedSourceSnapshot: snapshot,
        }),
      ).toEqual({ status: "authorized" });
      expect(
        authorizeGitDelivery({
          mode,
          destination: none,
          reviewedProgress: progress,
          observedSourceSnapshot: snapshot,
        }),
      ).toMatchObject({ status: "denied" });
    },
  );

  it("allows local-only without a destination and rejects review drift", () => {
    expect(
      authorizeGitDelivery({
        mode: "local-only",
        destination: none,
        reviewedProgress: progress,
        observedSourceSnapshot: snapshot,
      }),
    ).toEqual({ status: "authorized" });
    expect(
      authorizeGitDelivery({
        mode: "local-only",
        destination: some(destination),
        reviewedProgress: progress,
        observedSourceSnapshot: snapshot,
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_DELIVERY_DESTINATION_INVALID",
    });
    expect(
      authorizeGitDelivery({
        mode: "local-only",
        destination: none,
        reviewedProgress: progress,
        observedSourceSnapshot: sourceSnapshotDigest(
          `sha256:${"e".repeat(64)}`,
        ),
      }),
    ).toEqual({ status: "denied", code: "TIBER_DELIVERY_REVIEW_REQUIRED" });
  });

  it("requires pushed receipts to observe the exact commit", () => {
    const receipt = {
      mode: "branch-push" as const,
      baselineRevision: claimBaselineRevision("9".repeat(40)),
      commit,
      tree,
      sourceSnapshotDigest: snapshot,
      destination: some(destination),
      observedRemoteCommit: some(commit),
    };
    expect(validateGitDeliveryReceipt(receipt)).toEqual({
      status: "authorized",
    });
    expect(
      validateGitDeliveryReceipt({
        ...receipt,
        observedRemoteCommit: some(
          required(parseDeliveryCommitRevision("f".repeat(40))),
        ),
      }),
    ).toMatchObject({ status: "denied" });
    expect(
      validateGitDeliveryReceipt({
        ...receipt,
        observedRemoteCommit: none,
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_DELIVERY_DESTINATION_INVALID",
    });
    expect(
      validateGitDeliveryReceipt({
        ...receipt,
        destination: none,
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_DELIVERY_DESTINATION_INVALID",
    });
    const local = { ...receipt, mode: "local-only" as const };
    expect(
      validateGitDeliveryReceipt({
        ...local,
        destination: none,
        observedRemoteCommit: none,
      }),
    ).toEqual({ status: "authorized" });
    expect(
      validateGitDeliveryReceipt({
        ...local,
        observedRemoteCommit: none,
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_DELIVERY_DESTINATION_INVALID",
    });
    expect(validateGitDeliveryReceipt(local)).toEqual({
      status: "denied",
      code: "TIBER_DELIVERY_DESTINATION_INVALID",
    });
    expect(validateGitDeliveryReceipt({ ...local, destination: none })).toEqual(
      {
        status: "denied",
        code: "TIBER_DELIVERY_DESTINATION_INVALID",
      },
    );
  });
});
