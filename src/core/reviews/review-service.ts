import type { TiberFailure, TiberResult } from "../failures/tiber-failure.js";
import {
  parseReviewAuthorLogin,
  parseReviewBaseRef,
  parseReviewBody,
  parseReviewHeadRef,
  parseReviewNodeId,
  parseReviewNumber,
  parseReviewRepositoryName,
  parseReviewRepositoryOwner,
  parseReviewRevision,
  parseReviewTitle,
  parseReviewUrl,
  type ReviewAuthorLogin,
  type ReviewBaseRef,
  type ReviewBody,
  type ReviewHeadRef,
  type ReviewNodeId,
  type ReviewNumber,
  type ReviewRepositoryName,
  type ReviewRepositoryOwner,
  type ReviewRevision,
  type ReviewTitle,
  type ReviewUrl,
} from "./review-service-values.js";

export type ReviewKind = "ordinary" | "release";
export type ReviewApprovalStatus = "approved" | "pending" | "changes-requested";
export type ReviewConversationStatus = "resolved" | "unresolved";
export type ReviewCiStatus = "success" | "pending" | "failure";
export type ReviewMergePermission = "granted" | "missing";

export interface ReviewRequest {
  readonly repositoryOwner: ReviewRepositoryOwner;
  readonly repositoryName: ReviewRepositoryName;
  readonly headRef: ReviewHeadRef;
  readonly headRevision: ReviewRevision;
  readonly baseRef: ReviewBaseRef;
  readonly title: ReviewTitle;
  readonly body: ReviewBody;
}

export interface PullRequestReceipt {
  readonly number: ReviewNumber;
  readonly nodeId: ReviewNodeId;
  readonly url: ReviewUrl;
  readonly author: ReviewAuthorLogin;
  readonly headRevision: ReviewRevision;
}

export interface OpenedReview {
  readonly kind: ReviewKind;
  readonly request: ReviewRequest;
  readonly pullRequest: PullRequestReceipt;
}

export type ReviewDeliveryDisposition =
  | "merged"
  | "auto-merge-enabled"
  | "permission-missing"
  | "human-merge-required";
export type ReviewMergeStatus = "open" | "merged";

export interface ReviewGateReceipt {
  readonly observation: ReviewGateObservation;
  readonly mergeStatus: ReviewMergeStatus;
  readonly disposition: ReviewDeliveryDisposition;
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseOpenedReview(value: unknown): OpenedReview | undefined {
  if (!record(value) || !record(value.request) || !record(value.pullRequest))
    return undefined;
  const request = value.request;
  const repositoryOwner = parseReviewRepositoryOwner(request.repositoryOwner);
  const repositoryName = parseReviewRepositoryName(request.repositoryName);
  const headRef = parseReviewHeadRef(request.headRef);
  const headRevision = parseReviewRevision(request.headRevision);
  const baseRef = parseReviewBaseRef(request.baseRef);
  const title = parseReviewTitle(request.title);
  const body = parseReviewBody(request.body);
  const number = parseReviewNumber(value.pullRequest.number);
  const nodeId = parseReviewNodeId(value.pullRequest.nodeId);
  const url = parseReviewUrl(value.pullRequest.url);
  const author = parseReviewAuthorLogin(value.pullRequest.author);
  const pullRevision = parseReviewRevision(value.pullRequest.headRevision);
  if (
    // Stryker disable next-line ConditionalExpression: deterministic classification below independently rejects every kind outside the closed pair; this check preserves narrowing.
    (value.kind !== "ordinary" && value.kind !== "release") ||
    !repositoryOwner.ok ||
    !repositoryName.ok ||
    !headRef.ok ||
    !headRevision.ok ||
    !baseRef.ok ||
    !title.ok ||
    !body.ok ||
    !number.ok ||
    !nodeId.ok ||
    !url.ok ||
    !author.ok ||
    !pullRevision.ok ||
    pullRevision.value !== headRevision.value
  )
    return undefined;
  const parsedRequest = {
    repositoryOwner: repositoryOwner.value,
    repositoryName: repositoryName.value,
    headRef: headRef.value,
    headRevision: headRevision.value,
    baseRef: baseRef.value,
    title: title.value,
    body: body.value,
  };
  return classifyReviewKind(parsedRequest.headRef, parsedRequest.title) ===
    value.kind
    ? {
        kind: value.kind,
        request: parsedRequest,
        pullRequest: {
          number: number.value,
          nodeId: nodeId.value,
          url: url.value,
          author: author.value,
          headRevision: pullRevision.value,
        },
      }
    : undefined;
}

export function parseReviewGateReceipt(
  value: unknown,
): ReviewGateReceipt | undefined {
  if (!record(value) || !record(value.observation)) return undefined;
  const headRevision = parseReviewRevision(value.observation.headRevision);
  const reviewStatus = value.observation.reviewStatus;
  const conversationStatus = value.observation.conversationStatus;
  const ciStatus = value.observation.ciStatus;
  const authorMergePermission = value.observation.authorMergePermission;
  const disposition = value.disposition;
  return headRevision.ok &&
    (reviewStatus === "approved" ||
      reviewStatus === "pending" ||
      reviewStatus === "changes-requested") &&
    (conversationStatus === "resolved" ||
      conversationStatus === "unresolved") &&
    (ciStatus === "success" ||
      ciStatus === "pending" ||
      ciStatus === "failure") &&
    (authorMergePermission === "granted" ||
      authorMergePermission === "missing") &&
    (value.mergeStatus === "open" || value.mergeStatus === "merged") &&
    (disposition === "merged" ||
      disposition === "auto-merge-enabled" ||
      disposition === "permission-missing" ||
      disposition === "human-merge-required")
    ? {
        observation: {
          headRevision: headRevision.value,
          reviewStatus,
          conversationStatus,
          ciStatus,
          authorMergePermission,
        },
        mergeStatus: value.mergeStatus,
        disposition,
      }
    : undefined;
}

export interface ReviewGateObservation {
  readonly headRevision: ReviewRevision;
  readonly reviewStatus: ReviewApprovalStatus;
  readonly conversationStatus: ReviewConversationStatus;
  readonly ciStatus: ReviewCiStatus;
  readonly authorMergePermission: ReviewMergePermission;
}

export type ReviewServiceFailure = TiberFailure<
  | "TIBER_REVIEW_SERVICE_FAILED"
  | "TIBER_REVIEW_SERVICE_RESPONSE_INVALID"
  | "TIBER_REVIEW_SERVICE_PERMISSION_MISSING",
  { readonly domain: "review-service" },
  "corrected-input" | "state-change" | "retry-operation"
>;
export type ReviewServiceResult<T> = TiberResult<T, ReviewServiceFailure>;

export interface PullRequestService {
  create(
    request: ReviewRequest,
  ): Promise<ReviewServiceResult<PullRequestReceipt>>;
}

export interface ReviewObservationService {
  observe(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<
    ReviewServiceResult<
      Pick<
        ReviewGateObservation,
        "headRevision" | "reviewStatus" | "conversationStatus"
      >
    >
  >;
}

export interface ReviewCiService {
  observe(
    request: ReviewRequest,
  ): Promise<
    ReviewServiceResult<
      Pick<ReviewGateObservation, "headRevision" | "ciStatus">
    >
  >;
}

export interface ReviewMergeService {
  observeMerge(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<ReviewMergeStatus>>;
  observeAuthorPermission(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<ReviewMergePermission>>;
  enableAutoMerge(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<void>>;
}

export function classifyReviewKind(
  headRef: ReviewHeadRef,
  title: ReviewTitle,
): ReviewKind {
  return headRef.startsWith("refs/heads/release-please--") ||
    title.startsWith("chore(main): release ")
    ? "release"
    : "ordinary";
}

export type ReviewObservationAssembly =
  | {
      readonly status: "assembled";
      readonly observation: ReviewGateObservation;
    }
  | {
      readonly status: "denied";
      readonly code: "TIBER_REVIEW_REVISION_MISMATCH";
    };

export function assembleReviewGateObservation(input: {
  readonly deliveredRevision: ReviewRevision;
  readonly review: Pick<
    ReviewGateObservation,
    "headRevision" | "reviewStatus" | "conversationStatus"
  >;
  readonly ci: Pick<ReviewGateObservation, "headRevision" | "ciStatus">;
  readonly authorMergePermission: ReviewMergePermission;
}): ReviewObservationAssembly {
  return input.review.headRevision === input.deliveredRevision &&
    input.ci.headRevision === input.deliveredRevision
    ? {
        status: "assembled",
        observation: {
          headRevision: input.deliveredRevision,
          reviewStatus: input.review.reviewStatus,
          conversationStatus: input.review.conversationStatus,
          ciStatus: input.ci.ciStatus,
          authorMergePermission: input.authorMergePermission,
        },
      }
    : { status: "denied", code: "TIBER_REVIEW_REVISION_MISMATCH" };
}

export type ReviewAutoMergeDecision =
  | { readonly status: "authorized"; readonly effect: "enable-auto-merge" }
  | {
      readonly status: "waiting";
      readonly code: "TIBER_REVIEW_GATES_INCOMPLETE";
    }
  | {
      readonly status: "human-required";
      readonly code: "TIBER_RELEASE_HUMAN_MERGE_REQUIRED";
    }
  | {
      readonly status: "denied";
      readonly code:
        | "TIBER_REVIEW_REVISION_MISMATCH"
        | "TIBER_REVIEW_GATE_FAILED"
        | "TIBER_REVIEW_MERGE_PERMISSION_REQUIRED";
    };

export function reviewDisposition(
  decision: ReviewAutoMergeDecision,
  mergeStatus: ReviewMergeStatus,
): ReviewDeliveryDisposition | undefined {
  if (mergeStatus === "merged") return "merged";
  if (decision.status === "authorized") return "auto-merge-enabled";
  if (decision.status === "human-required") return "human-merge-required";
  // Stryker disable next-line ConditionalExpression: this code exists only on the denied variant; the status check preserves explicit closed-union narrowing.
  return decision.status === "denied" &&
    decision.code === "TIBER_REVIEW_MERGE_PERMISSION_REQUIRED"
    ? "permission-missing"
    : undefined;
}

export function authorizeReviewAutoMerge(input: {
  readonly kind: ReviewKind;
  readonly deliveredRevision: ReviewRevision;
  readonly baseRef: ReviewBaseRef;
  readonly observation: ReviewGateObservation;
}): ReviewAutoMergeDecision {
  if (input.observation.headRevision !== input.deliveredRevision)
    return { status: "denied", code: "TIBER_REVIEW_REVISION_MISMATCH" };
  if (input.kind === "release")
    return {
      status: "human-required",
      code: "TIBER_RELEASE_HUMAN_MERGE_REQUIRED",
    };
  if (
    input.observation.reviewStatus === "changes-requested" ||
    input.observation.ciStatus === "failure"
  )
    return { status: "denied", code: "TIBER_REVIEW_GATE_FAILED" };
  if (input.observation.authorMergePermission === "missing")
    return {
      status: "denied",
      code: "TIBER_REVIEW_MERGE_PERMISSION_REQUIRED",
    };
  if (
    input.observation.reviewStatus !== "approved" ||
    input.observation.conversationStatus !== "resolved" ||
    input.observation.ciStatus !== "success"
  )
    return { status: "waiting", code: "TIBER_REVIEW_GATES_INCOMPLETE" };
  return { status: "authorized", effect: "enable-auto-merge" };
}
