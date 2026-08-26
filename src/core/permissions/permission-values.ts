import { createHash } from "node:crypto";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";
import type { AgentRole, PermissionEffect } from "./permission-policy.js";

declare const permissionValuePurpose: unique symbol;
type PermissionValue<Purpose extends string> = string & {
  readonly [permissionValuePurpose]: Purpose;
};

export type PermissionScope = PermissionValue<"permission-scope">;
export type PermissionDecisionAt = PermissionValue<"permission-decision-at">;

export interface PermissionScopeFacts {
  readonly role: AgentRole;
  readonly effect: PermissionEffect;
  readonly executable: string;
  readonly argv: readonly string[];
  readonly purpose: string;
  readonly cwd: "task-worktree" | "repository";
  readonly environment: Readonly<Record<string, string>>;
}

type PermissionValueFailure = TiberFailure<
  "TIBER_PERMISSION_VALUE_INVALID",
  { readonly field: "permissionScope" | "permissionDecisionAt" },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, PermissionValueFailure>;

function invalid(
  field: PermissionValueFailure["safeContext"]["field"],
): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_PERMISSION_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

export function permissionScope(facts: PermissionScopeFacts): PermissionScope {
  const environment = Object.entries(facts.environment).sort();
  const canonical = JSON.stringify({
    role: facts.role,
    effect: facts.effect,
    executable: facts.executable,
    argv: facts.argv,
    purpose: facts.purpose,
    cwd: facts.cwd,
    environment,
  });
  return `sha256:${createHash("sha256").update(canonical).digest("hex")}` as PermissionScope;
}

export function parsePermissionScope(input: unknown): Result<PermissionScope> {
  return typeof input === "string" && /^sha256:[0-9a-f]{64}$/u.test(input)
    ? { ok: true, value: input as PermissionScope }
    : invalid("permissionScope");
}

export function parsePermissionDecisionAt(
  input: unknown,
): Result<PermissionDecisionAt> {
  if (typeof input !== "string") return invalid("permissionDecisionAt");
  const milliseconds = Date.parse(input);
  return Number.isFinite(milliseconds) &&
    new Date(milliseconds).toISOString() === input
    ? { ok: true, value: input as PermissionDecisionAt }
    : invalid("permissionDecisionAt");
}
