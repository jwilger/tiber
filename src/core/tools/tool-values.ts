import { isAbsolute } from "node:path";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const toolValuePurpose: unique symbol;
type ToolValue<Purpose extends string> = string & {
  readonly [toolValuePurpose]: Purpose;
};

export type ClaimedWorkspaceRoot = ToolValue<"claimed-workspace-root">;
export type RequestedWorkspacePath = ToolValue<"requested-workspace-path">;
export type CanonicalReadTarget = ToolValue<"canonical-read-target">;

type Field =
  "canonicalReadTarget" | "claimedWorkspaceRoot" | "requestedWorkspacePath";
type Failure = TiberFailure<
  "TIBER_TOOL_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_TOOL_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function absolute<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<ToolValue<Purpose>> {
  return typeof value === "string" &&
    isAbsolute(value) &&
    value.length <= 4_096 &&
    !value.includes("\0")
    ? { ok: true, value: value as ToolValue<Purpose> }
    : invalid(field);
}

export const parseClaimedWorkspaceRoot = (
  value: unknown,
): Result<ClaimedWorkspaceRoot> => absolute(value, "claimedWorkspaceRoot");
export const parseCanonicalReadTarget = (
  value: unknown,
): Result<CanonicalReadTarget> => absolute(value, "canonicalReadTarget");
export function parseRequestedWorkspacePath(
  value: unknown,
): Result<RequestedWorkspacePath> {
  return typeof value === "string" &&
    value.length > 0 &&
    value.length <= 4_096 &&
    !value.includes("\0")
    ? { ok: true, value: value as RequestedWorkspacePath }
    : invalid("requestedWorkspacePath");
}
