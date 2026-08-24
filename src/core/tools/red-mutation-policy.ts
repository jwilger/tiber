import { isAbsolute, posix } from "node:path";

import type {
  ClaimPublicationStatus,
  TestMappingPath,
} from "../tasks/task-values.js";
import type { ToolDecision, ToolDecisionCode } from "./tool-policy.js";

function deny(code: ToolDecisionCode, detail: string): ToolDecision {
  return { allowed: false, code, detail };
}

export function authorizeWorkflowMutation(
  requestedPath: string,
  authority: {
    readonly claimStatus: ClaimPublicationStatus;
    readonly redStatus: "accepted" | "required";
    readonly testMappings: readonly TestMappingPath[];
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
  if (authority.claimStatus !== "published")
    return deny(
      "TIBER_MUTATION_CLAIM_REQUIRED",
      "mutation requires an exact active remote claim",
    );
  if (authority.redStatus === "accepted")
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
