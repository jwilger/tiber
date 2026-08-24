import {
  semanticValueFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const deliveryValuePurpose: unique symbol;
type DeliveryValue<Purpose extends string> = string & {
  readonly [deliveryValuePurpose]: Purpose;
};

export type DeliveryCommitRevision = DeliveryValue<"delivery-commit-revision">;
export type DeliveryTreeDigest = DeliveryValue<"delivery-tree-digest">;
export type DeliveryDestinationRef = DeliveryValue<"delivery-destination-ref">;
export type DeliveryCommitSubject = DeliveryValue<"delivery-commit-subject">;
export type DeliveryCommitBody = DeliveryValue<"delivery-commit-body">;

type Field =
  | "commitRevision"
  | "treeDigest"
  | "destinationRef"
  | "commitSubject"
  | "commitBody";
type Result<Value> = TiberResult<Value, ReturnType<typeof failure>>;
function failure(field: Field) {
  return semanticValueFailure(
    "TIBER_DELIVERY_VALUE_INVALID",
    field,
    "corrected-value",
  );
}
function value<Purpose extends string>(
  input: unknown,
  field: Field,
  pattern: RegExp,
): Result<DeliveryValue<Purpose>> {
  return typeof input === "string" && pattern.test(input)
    ? { ok: true, value: input as DeliveryValue<Purpose> }
    : { ok: false, failure: failure(field) };
}
export const parseDeliveryCommitRevision = (
  input: unknown,
): Result<DeliveryCommitRevision> =>
  value(input, "commitRevision", /^[0-9a-f]{40}$/u);
export const parseDeliveryTreeDigest = (
  input: unknown,
): Result<DeliveryTreeDigest> => value(input, "treeDigest", /^[0-9a-f]{40}$/u);
export function parseDeliveryDestinationRef(
  input: unknown,
): Result<DeliveryDestinationRef> {
  return typeof input === "string" &&
    /^refs\/heads\/[a-zA-Z0-9][a-zA-Z0-9._/-]{0,199}$/u.test(input) &&
    !input.includes("..") &&
    !input.includes("//") &&
    !input.endsWith("/") &&
    !input.endsWith(".") &&
    !input.split("/").some((segment) => segment.startsWith(".")) &&
    !input.split("/").some((segment) => segment.endsWith(".lock"))
    ? { ok: true, value: input as DeliveryDestinationRef }
    : { ok: false, failure: failure("destinationRef") };
}
export const parseDeliveryCommitSubject = (
  input: unknown,
): Result<DeliveryCommitSubject> =>
  value(
    input,
    "commitSubject",
    /^(?:build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)(?:\([a-z0-9-]+\))?!?: [^\n]{1,100}$/u,
  );
export function parseDeliveryCommitBody(
  input: unknown,
): Result<DeliveryCommitBody> {
  return typeof input === "string" &&
    input.trim() === input &&
    input.length >= 12 &&
    !input.includes("\u0000")
    ? { ok: true, value: input as DeliveryCommitBody }
    : { ok: false, failure: failure("commitBody") };
}
