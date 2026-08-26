import type { Option } from "../types/option.js";

export type AutonomyLevel = "ask-first" | "routine" | "repository";
export type AgentRole =
  | "coordinator"
  | "planning"
  | "readiness"
  | "review"
  | "setup"
  | "classifier"
  | "implementation"
  | "delivery"
  | "ci";
export type PermissionEffect =
  | "repository-read"
  | "process"
  | "arbitrary-shell"
  | "git-read"
  | "git-mutate"
  | "github-read"
  | "github-mutate";
export type PermissionRisk =
  "routine" | "unfamiliar" | "destructive" | "publication" | "privileged";
export type PermissionBoundary = "repository" | "external";
export type RememberedPermission = "allow" | "deny";
export type PermissionChoice =
  "deny-once" | "deny-always" | "allow-once" | "allow-always";

export interface PermissionRequest {
  readonly autonomy: AutonomyLevel;
  readonly role: AgentRole;
  readonly effect: PermissionEffect;
  readonly risk: PermissionRisk;
  readonly boundary: PermissionBoundary;
  readonly workflow: "authorized" | "denied";
  readonly remembered: Option<RememberedPermission>;
  readonly interactive: boolean;
  readonly persistable: boolean;
}

export type PermissionDecision =
  | {
      readonly status: "allowed";
      readonly code:
        | "TIBER_PERMISSION_READ_ONLY"
        | "TIBER_PERMISSION_REMEMBERED"
        | "TIBER_PERMISSION_ROUTINE"
        | "TIBER_PERMISSION_REPOSITORY_AUTONOMY";
    }
  | {
      readonly status: "denied";
      readonly code:
        | "TIBER_PERMISSION_WORKFLOW_DENIED"
        | "TIBER_PERMISSION_ROLE_DENIED"
        | "TIBER_PERMISSION_ALWAYS_DENIED"
        | "TIBER_PERMISSION_INTERACTION_REQUIRED";
    }
  | {
      readonly status: "prompt";
      readonly code:
        | "TIBER_PERMISSION_REQUIRED"
        | "TIBER_PERMISSION_EXACT_APPROVAL_REQUIRED";
      readonly choices: readonly PermissionChoice[];
    };

function roleRestrictsProcess(role: AgentRole): boolean {
  return (
    role === "coordinator" ||
    role === "planning" ||
    role === "readiness" ||
    role === "review" ||
    role === "setup" ||
    role === "classifier"
  );
}

function riskRequiresExactApproval(risk: PermissionRisk): boolean {
  return (
    risk === "destructive" || risk === "publication" || risk === "privileged"
  );
}

function roleAllows(request: PermissionRequest): boolean {
  if (request.effect === "repository-read") return true;
  if (
    roleRestrictsProcess(request.role) &&
    (request.effect === "process" || request.effect === "arbitrary-shell")
  )
    return false;
  if (request.role === "delivery")
    return (
      request.effect === "git-read" ||
      request.effect === "git-mutate" ||
      request.effect === "github-read" ||
      request.effect === "github-mutate"
    );
  if (request.role === "ci") return request.effect === "github-read";
  return true;
}

function requiresExactApproval(request: PermissionRequest): boolean {
  return (
    request.effect === "arbitrary-shell" ||
    request.boundary === "external" ||
    riskRequiresExactApproval(request.risk) ||
    !request.persistable
  );
}

function prompt(exact: boolean): PermissionDecision {
  return exact
    ? {
        status: "prompt",
        code: "TIBER_PERMISSION_EXACT_APPROVAL_REQUIRED",
        choices: ["deny-once", "deny-always", "allow-once"],
      }
    : {
        status: "prompt",
        code: "TIBER_PERMISSION_REQUIRED",
        choices: ["deny-once", "deny-always", "allow-once", "allow-always"],
      };
}

export function decidePermission(
  request: PermissionRequest,
): PermissionDecision {
  if (request.workflow === "denied")
    return { status: "denied", code: "TIBER_PERMISSION_WORKFLOW_DENIED" };
  if (!roleAllows(request))
    return { status: "denied", code: "TIBER_PERMISSION_ROLE_DENIED" };
  if (request.effect === "repository-read")
    return { status: "allowed", code: "TIBER_PERMISSION_READ_ONLY" };
  const exact = requiresExactApproval(request);
  if (request.remembered.kind === "some") {
    if (request.remembered.value === "deny")
      return { status: "denied", code: "TIBER_PERMISSION_ALWAYS_DENIED" };
    if (!exact)
      return { status: "allowed", code: "TIBER_PERMISSION_REMEMBERED" };
  }
  if (exact)
    return request.interactive
      ? prompt(exact)
      : {
          status: "denied",
          code: "TIBER_PERMISSION_INTERACTION_REQUIRED",
        };
  if (request.autonomy === "repository")
    return {
      status: "allowed",
      code: "TIBER_PERMISSION_REPOSITORY_AUTONOMY",
    };
  if (request.autonomy === "routine" && request.risk === "routine")
    return { status: "allowed", code: "TIBER_PERMISSION_ROUTINE" };
  return request.interactive
    ? prompt(exact)
    : {
        status: "denied",
        code: "TIBER_PERMISSION_INTERACTION_REQUIRED",
      };
}
