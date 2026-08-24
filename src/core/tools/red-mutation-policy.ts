import { isAbsolute, posix } from "node:path";

import type { ToolDecision } from "./tool-policy.js";

function deny(code: string, detail: string): ToolDecision {
  return { allowed: false, code, detail };
}

export function authorizeWorkflowMutation(
  requestedPath: string,
  authority: {
    readonly activeClaim: boolean;
    readonly redAccepted: boolean;
    readonly testMappings: readonly string[];
  },
): ToolDecision {
  if (
    // Stryker disable next-line ConditionalExpression: posix.normalize maps the empty path to '.', so the canonical-equality check also rejects it; this branch documents the empty-path denial.
    requestedPath.length === 0 ||
    requestedPath.includes("\0") ||
    requestedPath.includes("\\") ||
    isAbsolute(requestedPath) ||
    posix.normalize(requestedPath) !== requestedPath ||
    requestedPath === ".." ||
    requestedPath.startsWith("../") ||
    requestedPath.startsWith(".git/") ||
    requestedPath === ".git"
  )
    return deny(
      "TIBER_MUTATION_PATH_INVALID",
      "mutation path must be canonical, repository-relative, and outside Git metadata",
    );
  if (!authority.activeClaim)
    return deny(
      "TIBER_MUTATION_CLAIM_REQUIRED",
      "mutation requires an exact active remote claim",
    );
  if (authority.redAccepted)
    return {
      allowed: true,
      code: "TIBER_PRODUCTION_MUTATION_ALLOWED",
      detail:
        "accepted scenario RED authorizes a diagnostic production micro-step",
    };
  return authority.testMappings.some((mapping) => mapping === requestedPath)
    ? {
        allowed: true,
        code: "TIBER_TEST_MUTATION_ALLOWED",
        detail: "mapped test mutation is allowed before RED",
      }
    : deny(
        "TIBER_RED_REQUIRED",
        "production mutation requires an accepted exact scenario RED",
      );
}
