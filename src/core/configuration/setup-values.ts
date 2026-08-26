import { isAbsolute } from "node:path";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const setupValuePurpose: unique symbol;
type SetupValue<Purpose extends string> = string & {
  readonly [setupValuePurpose]: Purpose;
};

export type SetupRepositoryPath = SetupValue<"setup-repository-path">;
export type SetupAgentDirectoryPath = SetupValue<"setup-agent-directory-path">;
export type SetupPlanDigest = SetupValue<"setup-plan-digest">;
export type SetupExpectedAuthorityDigest =
  SetupValue<"setup-expected-authority-digest">;

type SetupPathField =
  | "setupRepositoryPath"
  | "setupAgentDirectoryPath"
  | "setupPlanDigest"
  | "setupExpectedAuthorityDigest";
type SetupPathFailure = TiberFailure<
  "TIBER_SETUP_VALUE_INVALID",
  { readonly field: SetupPathField },
  "corrected-value"
>;
type SetupPathResult<Value> = TiberResult<Value, SetupPathFailure>;

function validPath(value: unknown): value is string {
  return (
    typeof value === "string" &&
    isAbsolute(value) &&
    value.length <= 4_096 &&
    !value.includes("\0")
  );
}

function invalid<Value>(field: SetupPathField): SetupPathResult<Value> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_SETUP_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

export function parseSetupRepositoryPath(
  value: unknown,
): SetupPathResult<SetupRepositoryPath> {
  return validPath(value)
    ? { ok: true, value: value as SetupRepositoryPath }
    : invalid("setupRepositoryPath");
}

export function parseSetupAgentDirectoryPath(
  value: unknown,
): SetupPathResult<SetupAgentDirectoryPath> {
  return validPath(value)
    ? { ok: true, value: value as SetupAgentDirectoryPath }
    : invalid("setupAgentDirectoryPath");
}

function digest(value: unknown): value is string {
  // Stryker disable next-line ConditionalExpression: the anchored digest grammar rejects every non-string after RegExp coercion; typeof establishes trust-boundary narrowing.
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/u.test(value);
}

export function parseSetupPlanDigest(
  value: unknown,
): SetupPathResult<SetupPlanDigest> {
  return digest(value)
    ? { ok: true, value: value as SetupPlanDigest }
    : invalid("setupPlanDigest");
}

export function parseSetupExpectedAuthorityDigest(
  value: unknown,
): SetupPathResult<SetupExpectedAuthorityDigest> {
  return digest(value)
    ? { ok: true, value: value as SetupExpectedAuthorityDigest }
    : invalid("setupExpectedAuthorityDigest");
}
