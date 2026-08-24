import {
  parseCanonicalReadTarget,
  parseClaimedWorkspaceRoot,
  parseRequestedWorkspacePath,
} from "../../src/core/tools/tool-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid tool value fixture");
  return result.value;
}

export const claimedWorkspaceRoot = (value: string) =>
  required(parseClaimedWorkspaceRoot(value));
export const requestedWorkspacePath = (value: string) =>
  required(parseRequestedWorkspacePath(value));
export const canonicalReadTarget = (value: string) =>
  required(parseCanonicalReadTarget(value));
