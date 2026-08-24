import { describe, expect, it } from "vitest";

import {
  assembleReviewGateObservation,
  authorizeReviewAutoMerge,
  classifyReviewKind,
  parseOpenedReview,
  parseReviewGateReceipt,
  reviewDisposition,
  type ReviewGateObservation,
} from "../../src/core/reviews/review-service.js";
import {
  parseReviewBaseRef,
  parseReviewHeadRef,
  parseReviewRevision,
  parseReviewTitle,
} from "../../src/core/reviews/review-service-values.js";

function parsed<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  if (!result.ok) throw new Error("fixture invalid");
  return result.value;
}

const revision = parsed(parseReviewRevision("a".repeat(40)));
const otherRevision = parsed(parseReviewRevision("b".repeat(40)));
const ordinaryHead = parsed(parseReviewHeadRef("refs/heads/feat/review"));
const releaseHead = parsed(
  parseReviewHeadRef(
    "refs/heads/release-please--branches--main--components--tiber",
  ),
);
const base = parsed(parseReviewBaseRef("main"));
const ordinaryTitle = parsed(
  parseReviewTitle("feat(review): add service adapter"),
);
const releaseTitle = parsed(
  parseReviewTitle("chore(main): release tiber 1.2.3"),
);

function observation(
  overrides: Partial<ReviewGateObservation> = {},
): ReviewGateObservation {
  return {
    headRevision: revision,
    reviewStatus: "approved",
    conversationStatus: "resolved",
    ciStatus: "success",
    authorMergePermission: "granted",
    ...overrides,
  };
}

describe("generic review service authority", () => {
  it("parses complete opened reviews and gate receipts at the event boundary", () => {
    const opened = {
      kind: "ordinary",
      request: {
        repositoryOwner: "owner",
        repositoryName: "repo",
        headRef: ordinaryHead,
        headRevision: revision,
        baseRef: base,
        title: ordinaryTitle,
        body: "body",
      },
      pullRequest: {
        number: 1,
        nodeId: "node",
        url: "https://github.com/owner/repo/pull/1",
        author: "author",
        headRevision: revision,
      },
    };
    expect(parseOpenedReview(opened)).toEqual(opened);
    const callable = Object.assign(() => undefined, opened);
    expect(parseOpenedReview(callable)).toBeUndefined();
    for (const candidate of [
      null,
      { ...opened, kind: "unknown" },
      { ...opened, request: null },
      { ...opened, request: { ...opened.request, repositoryOwner: "" } },
      { ...opened, request: { ...opened.request, repositoryName: "" } },
      { ...opened, request: { ...opened.request, headRef: "bad" } },
      { ...opened, request: { ...opened.request, headRevision: "bad" } },
      { ...opened, request: { ...opened.request, baseRef: "" } },
      { ...opened, request: { ...opened.request, title: "bad" } },
      { ...opened, request: { ...opened.request, body: "" } },
      { ...opened, pullRequest: null },
      { ...opened, pullRequest: { ...opened.pullRequest, number: 0 } },
      { ...opened, pullRequest: { ...opened.pullRequest, nodeId: "" } },
      { ...opened, pullRequest: { ...opened.pullRequest, url: "bad" } },
      { ...opened, pullRequest: { ...opened.pullRequest, author: "" } },
      {
        ...opened,
        pullRequest: { ...opened.pullRequest, headRevision: otherRevision },
      },
      { ...opened, kind: "release" },
    ])
      expect(parseOpenedReview(candidate)).toBeUndefined();

    const receipt = {
      observation: observation(),
      mergeStatus: "open",
      disposition: "auto-merge-enabled",
    };
    expect(parseReviewGateReceipt(receipt)).toEqual(receipt);
    for (const alternate of [
      {
        observation: {
          ...receipt.observation,
          reviewStatus: "pending",
          conversationStatus: "unresolved",
          ciStatus: "pending",
        },
        mergeStatus: "open",
        disposition: "permission-missing",
      },
      {
        observation: {
          ...receipt.observation,
          reviewStatus: "changes-requested",
          ciStatus: "failure",
        },
        mergeStatus: "merged",
        disposition: "human-merge-required",
      },
    ])
      expect(parseReviewGateReceipt(alternate)).toEqual(alternate);
    for (const candidate of [
      null,
      { ...receipt, observation: null },
      {
        ...receipt,
        observation: { ...receipt.observation, headRevision: "bad" },
      },
      {
        ...receipt,
        observation: { ...receipt.observation, reviewStatus: "bad" },
      },
      {
        ...receipt,
        observation: { ...receipt.observation, conversationStatus: "bad" },
      },
      { ...receipt, observation: { ...receipt.observation, ciStatus: "bad" } },
      {
        ...receipt,
        observation: { ...receipt.observation, authorMergePermission: "bad" },
      },
      { ...receipt, mergeStatus: "bad" },
      { ...receipt, disposition: "bad" },
    ])
      expect(parseReviewGateReceipt(candidate)).toBeUndefined();
  });

  it("classifies release PRs from deterministic branch or title identity", () => {
    expect(classifyReviewKind(ordinaryHead, ordinaryTitle)).toBe("ordinary");
    expect(classifyReviewKind(releaseHead, ordinaryTitle)).toBe("release");
    expect(classifyReviewKind(ordinaryHead, releaseTitle)).toBe("release");
  });

  it("authorizes ordinary auto-merge only after every exact gate", () => {
    expect(
      authorizeReviewAutoMerge({
        kind: "ordinary",
        deliveredRevision: revision,
        baseRef: base,
        observation: observation(),
      }),
    ).toEqual({ status: "authorized", effect: "enable-auto-merge" });

    for (const gate of [
      observation({ reviewStatus: "pending" }),
      observation({ conversationStatus: "unresolved" }),
      observation({ ciStatus: "pending" }),
    ]) {
      expect(
        authorizeReviewAutoMerge({
          kind: "ordinary",
          deliveredRevision: revision,
          baseRef: base,
          observation: gate,
        }),
      ).toEqual({ status: "waiting", code: "TIBER_REVIEW_GATES_INCOMPLETE" });
    }
  });

  it("leaves an ordinary PR open when author merge permission is missing", () => {
    const decision = authorizeReviewAutoMerge({
      kind: "ordinary",
      deliveredRevision: revision,
      baseRef: base,
      observation: observation({ authorMergePermission: "missing" }),
    });
    expect(decision).toEqual({
      status: "denied",
      code: "TIBER_REVIEW_MERGE_PERMISSION_REQUIRED",
    });
    expect(reviewDisposition(decision, "open")).toBe("permission-missing");
  });

  it("always requires explicit human merge for a release PR", () => {
    for (const authorMergePermission of ["granted", "missing"] as const) {
      const decision = authorizeReviewAutoMerge({
        kind: "release",
        deliveredRevision: revision,
        baseRef: base,
        observation: observation({ authorMergePermission }),
      });
      expect(decision).toEqual({
        status: "human-required",
        code: "TIBER_RELEASE_HUMAN_MERGE_REQUIRED",
      });
      expect(reviewDisposition(decision, "open")).toBe("human-merge-required");
    }
  });

  it("maps only merge-authorizing terminal decisions to dispositions", () => {
    const authorized = authorizeReviewAutoMerge({
      kind: "ordinary",
      deliveredRevision: revision,
      baseRef: base,
      observation: observation(),
    });
    expect(reviewDisposition(authorized, "open")).toBe("auto-merge-enabled");
    expect(reviewDisposition(authorized, "merged")).toBe("merged");
    const waiting = authorizeReviewAutoMerge({
      kind: "ordinary",
      deliveredRevision: revision,
      baseRef: base,
      observation: observation({ ciStatus: "pending" }),
    });
    expect(reviewDisposition(waiting, "open")).toBeUndefined();
    const failed = authorizeReviewAutoMerge({
      kind: "ordinary",
      deliveredRevision: revision,
      baseRef: base,
      observation: observation({ ciStatus: "failure" }),
    });
    expect(reviewDisposition(failed, "open")).toBeUndefined();
  });

  it("assembles only exact-revision observations from separate ports", () => {
    expect(
      assembleReviewGateObservation({
        deliveredRevision: revision,
        review: {
          headRevision: revision,
          reviewStatus: "approved",
          conversationStatus: "resolved",
        },
        ci: { headRevision: revision, ciStatus: "success" },
        authorMergePermission: "granted",
      }),
    ).toEqual({ status: "assembled", observation: observation() });
    for (const mismatched of [
      {
        review: {
          headRevision: otherRevision,
          reviewStatus: "approved" as const,
          conversationStatus: "resolved" as const,
        },
        ci: { headRevision: revision, ciStatus: "success" as const },
      },
      {
        review: {
          headRevision: revision,
          reviewStatus: "approved" as const,
          conversationStatus: "resolved" as const,
        },
        ci: { headRevision: otherRevision, ciStatus: "success" as const },
      },
    ]) {
      expect(
        assembleReviewGateObservation({
          deliveredRevision: revision,
          ...mismatched,
          authorMergePermission: "granted",
        }),
      ).toEqual({ status: "denied", code: "TIBER_REVIEW_REVISION_MISMATCH" });
    }
  });

  it("rejects wrong-revision observations and terminal gate failures", () => {
    expect(
      authorizeReviewAutoMerge({
        kind: "ordinary",
        deliveredRevision: revision,
        baseRef: base,
        observation: observation({ headRevision: otherRevision }),
      }),
    ).toEqual({ status: "denied", code: "TIBER_REVIEW_REVISION_MISMATCH" });
    for (const failed of [
      observation({ ciStatus: "failure" }),
      observation({ reviewStatus: "changes-requested" }),
    ]) {
      expect(
        authorizeReviewAutoMerge({
          kind: "ordinary",
          deliveredRevision: revision,
          baseRef: base,
          observation: failed,
        }),
      ).toEqual({ status: "denied", code: "TIBER_REVIEW_GATE_FAILED" });
    }
  });
});
