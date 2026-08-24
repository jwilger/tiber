import type {
  DeliveryCommitRevision,
  DeliveryDestinationRef,
} from "../delivery/git-delivery-values.js";
import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

export type ReviewRevision = DeliveryCommitRevision;
export type ReviewHeadRef = DeliveryDestinationRef;
export type ReviewBaseRef = string & { readonly __brand: "ReviewBaseRef" };
export type ReviewTitle = string & { readonly __brand: "ReviewTitle" };
export type ReviewBody = string & { readonly __brand: "ReviewBody" };
export type ReviewRepositoryOwner = string & {
  readonly __brand: "ReviewRepositoryOwner";
};
export type ReviewRepositoryName = string & {
  readonly __brand: "ReviewRepositoryName";
};
export type ReviewNumber = number & { readonly __brand: "ReviewNumber" };
export type ReviewNodeId = string & { readonly __brand: "ReviewNodeId" };
export type ReviewUrl = string & { readonly __brand: "ReviewUrl" };
export type ReviewAuthorLogin = string & {
  readonly __brand: "ReviewAuthorLogin";
};

export type ReviewValueFailure = TiberFailure<
  "TIBER_REVIEW_VALUE_INVALID",
  { readonly field: "review" },
  string
>;
export type ReviewValueResult<T> = TiberResult<T, ReviewValueFailure>;

function invalid(name: string): ReviewValueResult<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_REVIEW_VALUE_INVALID",
      "review",
      `${name} is invalid`,
    ),
  };
}

export function parseReviewRevision(
  input: unknown,
): ReviewValueResult<ReviewRevision> {
  return typeof input === "string" && /^[0-9a-f]{40}$/.test(input)
    ? { ok: true, value: input as DeliveryCommitRevision }
    : invalid("review revision");
}

export function parseReviewHeadRef(
  input: unknown,
): ReviewValueResult<ReviewHeadRef> {
  return typeof input === "string" &&
    /^refs\/heads\/[A-Za-z0-9][A-Za-z0-9._/-]{0,254}$/.test(input) &&
    !input.includes("..") &&
    !input.endsWith(".") &&
    !input.endsWith("/")
    ? { ok: true, value: input as DeliveryDestinationRef }
    : invalid("review head ref");
}

export function parseReviewBaseRef(
  input: unknown,
): ReviewValueResult<ReviewBaseRef> {
  return typeof input === "string" &&
    /^[A-Za-z0-9][A-Za-z0-9._/-]{0,254}$/.test(input) &&
    !input.includes("..") &&
    !input.endsWith(".") &&
    !input.endsWith("/")
    ? { ok: true, value: input as ReviewBaseRef }
    : invalid("review base ref");
}

export function parseReviewTitle(
  input: unknown,
): ReviewValueResult<ReviewTitle> {
  return typeof input === "string" &&
    input.trim() === input &&
    input.length <= 200 &&
    /^[a-z]+(?:\([a-z0-9-]+\))?!?: .+$/u.test(input)
    ? { ok: true, value: input as ReviewTitle }
    : invalid("review title");
}

export function parseReviewBody(input: unknown): ReviewValueResult<ReviewBody> {
  return typeof input === "string" &&
    input.trim() === input &&
    input.length >= 1 &&
    input.length <= 20_000
    ? { ok: true, value: input as ReviewBody }
    : invalid("review body");
}

function identifier(input: unknown): input is string {
  return (
    typeof input === "string" &&
    /^[A-Za-z0-9](?:[A-Za-z0-9_.-]{0,98}[A-Za-z0-9])?$/.test(input)
  );
}

export function parseReviewRepositoryOwner(
  input: unknown,
): ReviewValueResult<ReviewRepositoryOwner> {
  return identifier(input)
    ? { ok: true, value: input as ReviewRepositoryOwner }
    : invalid("review repository owner");
}

export function parseReviewRepositoryName(
  input: unknown,
): ReviewValueResult<ReviewRepositoryName> {
  return identifier(input)
    ? { ok: true, value: input as ReviewRepositoryName }
    : invalid("review repository name");
}

export function parseReviewNumber(
  input: unknown,
): ReviewValueResult<ReviewNumber> {
  return Number.isSafeInteger(input) && Number(input) > 0
    ? { ok: true, value: input as ReviewNumber }
    : invalid("review number");
}

function boundedText(input: unknown, maximum: number): input is string {
  return (
    typeof input === "string" &&
    input.length > 0 &&
    input.length <= maximum &&
    !input.includes("\0")
  );
}

export function parseReviewNodeId(
  input: unknown,
): ReviewValueResult<ReviewNodeId> {
  return boundedText(input, 512)
    ? { ok: true, value: input as ReviewNodeId }
    : invalid("review node id");
}

export function parseReviewUrl(input: unknown): ReviewValueResult<ReviewUrl> {
  if (typeof input !== "string" || input.length > 2_048)
    return invalid("review URL");
  try {
    const url = new URL(input);
    return url.protocol === "https:" &&
      url.username === "" &&
      url.password === ""
      ? { ok: true, value: input as ReviewUrl }
      : invalid("review URL");
  } catch {
    return invalid("review URL");
  }
}

export function parseReviewAuthorLogin(
  input: unknown,
): ReviewValueResult<ReviewAuthorLogin> {
  return boundedText(input, 100)
    ? { ok: true, value: input as ReviewAuthorLogin }
    : invalid("review author login");
}
