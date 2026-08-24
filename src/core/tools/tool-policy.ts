import { isAbsolute, relative, resolve } from "node:path";

import type {
  CanonicalReadTarget,
  ClaimedWorkspaceRoot,
  RequestedWorkspacePath,
} from "./tool-values.js";

export type ToolDecisionCode =
  | "TIBER_MUTATION_CLAIMED"
  | "TIBER_MUTATION_CLAIM_REQUIRED"
  | "TIBER_PATH_OUTSIDE_WORKSPACE"
  | "TIBER_PATH_SYMLINK_ESCAPE"
  | "TIBER_READ_ALLOWED"
  | "TIBER_REFACTOR_ALLOWED"
  | "TIBER_REFACTOR_REQUIRES_GREEN"
  | "TIBER_TOOL_INVENTORY_COMPLETE"
  | "TIBER_TOOL_INVENTORY_INCOMPLETE"
  | "TIBER_PRODUCTION_MUTATION_ALLOWED"
  | "TIBER_RED_REQUIRED"
  | "TIBER_TEST_MUTATION_ALLOWED"
  | "TIBER_MUTATION_PATH_INVALID";

export interface ToolDecision<
  Code extends ToolDecisionCode = ToolDecisionCode,
> {
  readonly allowed: boolean;
  readonly code: Code;
  readonly detail: string;
}

export const GOVERNED_TOOL_NAMES = [
  "bash",
  "edit",
  "read",
  "tiber_artifact_range",
  "tiber_artifact_search",
  "tiber_command",
  "write",
] as const;

function deny<Code extends ToolDecisionCode>(
  code: Code,
  detail: string,
): ToolDecision<Code> {
  return { allowed: false, code, detail };
}

function isWithin(root: ClaimedWorkspaceRoot, candidate: string): boolean {
  const path = relative(root, candidate);
  // Stryker disable next-line ConditionalExpression, StringLiteral: the empty relative path also satisfies the general non-parent, non-absolute rule; the explicit root case documents intent.
  return path === "" || (!path.startsWith("..") && !isAbsolute(path));
}

export function authorizeReadPath(
  canonicalRoot: ClaimedWorkspaceRoot,
  requestedPath: RequestedWorkspacePath,
  canonicalTarget: CanonicalReadTarget,
): ToolDecision {
  const lexicalTarget = resolve(canonicalRoot, requestedPath);
  if (!isWithin(canonicalRoot, lexicalTarget)) {
    return deny(
      "TIBER_PATH_OUTSIDE_WORKSPACE",
      "requested path escapes the workspace",
    );
  }
  if (!isWithin(canonicalRoot, canonicalTarget)) {
    return deny(
      "TIBER_PATH_SYMLINK_ESCAPE",
      "canonical target escapes through a symlink",
    );
  }
  return {
    allowed: true,
    code: "TIBER_READ_ALLOWED",
    detail: "read-only workspace inspection allowed",
  };
}

export function authorizeMutation(
  claimStatus: "absent" | "published",
): ToolDecision {
  return claimStatus === "published"
    ? {
        allowed: true,
        code: "TIBER_MUTATION_CLAIMED",
        detail: "published task claim authorizes governed mutation",
      }
    : deny(
        "TIBER_MUTATION_CLAIM_REQUIRED",
        "repository mutation requires a remotely published exclusive task claim",
      );
}

export function verifyToolInventory(
  toolNames: readonly string[],
): ToolDecision<
  "TIBER_TOOL_INVENTORY_COMPLETE" | "TIBER_TOOL_INVENTORY_INCOMPLETE"
> {
  const unexpected = [...new Set(toolNames)]
    .filter((name) => !GOVERNED_TOOL_NAMES.some((known) => known === name))
    .sort();
  return unexpected.length === 0
    ? {
        allowed: true,
        code: "TIBER_TOOL_INVENTORY_COMPLETE",
        detail: "all executable tools are governed",
      }
    : deny(
        "TIBER_TOOL_INVENTORY_INCOMPLETE",
        `ungoverned executable tools: ${unexpected.join(", ")}`,
      );
}
