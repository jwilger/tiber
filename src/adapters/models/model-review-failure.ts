import {
  operationalFailure,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";

export type ModelReviewFailureCode =
  | "TIBER_FINAL_REVIEW_INVALID"
  | "TIBER_INCREMENT_REVIEW_INVALID"
  | "TIBER_READINESS_REVIEW_INVALID"
  | "TIBER_RED_REVIEW_INVALID"
  | "TIBER_REVIEW_EXECUTION_FAILED"
  | "TIBER_REVIEW_RESPONSE_MISSING";

export type ModelReviewFailure = TiberFailure<
  ModelReviewFailureCode,
  { readonly domain: "model-review" },
  "corrected-input" | "state-change" | "retry-operation"
>;

export function modelReviewFailure(
  code: ModelReviewFailureCode,
  message: string,
): ModelReviewFailure {
  return operationalFailure(
    code,
    "model-review",
    message,
    code === "TIBER_REVIEW_RESPONSE_MISSING" ||
      code === "TIBER_REVIEW_EXECUTION_FAILED"
      ? "transient"
      : "retry-after-input",
  );
}
