import type { Option } from "../types/option.js";
import type { FinalReviewProgress } from "../workflow/final-review.js";
import { decideReviewedCompletion } from "../workflow/final-review.js";
import type { ClaimBaselineRevision } from "../tasks/task-values.js";
import type { SourceSnapshotDigest } from "../workflow/workflow-values.js";
import type {
  DeliveryCommitRevision,
  DeliveryDestinationRef,
  DeliveryTreeDigest,
} from "./git-delivery-values.js";

export type GitDeliveryMode =
  "local-only" | "branch-push" | "direct" | "review";

export interface GitDeliveryRequest {
  readonly mode: GitDeliveryMode;
  readonly destination: Option<DeliveryDestinationRef>;
  readonly reviewedProgress: FinalReviewProgress;
  readonly observedSourceSnapshot: SourceSnapshotDigest;
}

export type GitDeliveryAuthorization =
  | { readonly status: "authorized" }
  | {
      readonly status: "denied";
      readonly code:
        "TIBER_DELIVERY_DESTINATION_INVALID" | "TIBER_DELIVERY_REVIEW_REQUIRED";
    };

export function authorizeGitDelivery(
  request: GitDeliveryRequest,
): GitDeliveryAuthorization {
  if (
    decideReviewedCompletion(
      request.reviewedProgress,
      request.observedSourceSnapshot,
    ).status !== "authorized"
  )
    return { status: "denied", code: "TIBER_DELIVERY_REVIEW_REQUIRED" };
  const destinationRequired = request.mode !== "local-only";
  return (destinationRequired && request.destination.kind === "none") ||
    (!destinationRequired && request.destination.kind === "some")
    ? { status: "denied", code: "TIBER_DELIVERY_DESTINATION_INVALID" }
    : { status: "authorized" };
}

export interface GitDeliveryReceipt {
  readonly mode: GitDeliveryMode;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly commit: DeliveryCommitRevision;
  readonly tree: DeliveryTreeDigest;
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
  readonly destination: Option<DeliveryDestinationRef>;
  readonly observedRemoteCommit: Option<DeliveryCommitRevision>;
}

export function validateGitDeliveryReceipt(
  receipt: GitDeliveryReceipt,
): GitDeliveryAuthorization {
  if (receipt.mode === "local-only")
    return receipt.destination.kind === "none" &&
      receipt.observedRemoteCommit.kind === "none"
      ? { status: "authorized" }
      : { status: "denied", code: "TIBER_DELIVERY_DESTINATION_INVALID" };
  if (
    receipt.destination.kind === "none" ||
    // Stryker disable next-line ConditionalExpression, StringLiteral: Option.none exposes no value and the exact comparison that follows independently denies it; this check documents the receipt shape and preserves narrowing.
    receipt.observedRemoteCommit.kind === "none" ||
    receipt.observedRemoteCommit.value !== receipt.commit
  )
    return { status: "denied", code: "TIBER_DELIVERY_DESTINATION_INVALID" };
  return { status: "authorized" };
}
