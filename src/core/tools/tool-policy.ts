import { isAbsolute, relative, resolve } from "node:path";

export interface ToolDecision {
  readonly allowed: boolean;
  readonly code: string;
  readonly detail: string;
}

export const GOVERNED_TOOL_NAMES = ["bash", "edit", "read", "write"] as const;

function deny(code: string, detail: string): ToolDecision {
  return { allowed: false, code, detail };
}

function isWithin(root: string, candidate: string): boolean {
  const path = relative(root, candidate);
  // Stryker disable next-line ConditionalExpression, StringLiteral: the empty relative path also satisfies the general non-parent, non-absolute rule; the explicit root case documents intent.
  return path === "" || (!path.startsWith("..") && !isAbsolute(path));
}

export function authorizeReadPath(
  canonicalRoot: string,
  requestedPath: string,
  canonicalTarget: string,
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

export function authorizeMutation(hasPublishedClaim: boolean): ToolDecision {
  return hasPublishedClaim
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
): ToolDecision {
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
