import type {
  PermissionDecisionAt,
  PermissionScope,
} from "../core/permissions/permission-values.js";
import {
  decidePermission,
  type PermissionChoice,
  type PermissionDecision,
  type PermissionRequest,
  type RememberedPermission,
} from "../core/permissions/permission-policy.js";
import type { Option } from "../core/types/option.js";

export interface PermissionDecisionStore {
  readonly lookup: (
    scope: PermissionScope,
  ) =>
    | { readonly ok: true; readonly value: Option<RememberedPermission> }
    | { readonly ok: false };
  readonly remember: (
    scope: PermissionScope,
    decision: RememberedPermission,
    decidedAt: PermissionDecisionAt,
  ) => { readonly ok: true; readonly value: unknown } | { readonly ok: false };
}

export interface PermissionPrompt {
  readonly choose: (
    description: string,
    choices: readonly PermissionChoice[],
  ) => Promise<Option<PermissionChoice>>;
}

type CorePermissionDenialCode = Extract<
  PermissionDecision,
  { readonly status: "denied" }
>["code"];

export type EffectAuthorization =
  | { readonly status: "allowed"; readonly remembered: boolean }
  | {
      readonly status: "denied";
      readonly code:
        | CorePermissionDenialCode
        | "TIBER_PERMISSION_STATE_INVALID"
        | "TIBER_PERMISSION_PROMPT_CANCELLED";
    };

export async function authorizeRequestedEffect(
  request: Omit<PermissionRequest, "remembered">,
  scope: PermissionScope,
  description: string,
  store: PermissionDecisionStore,
  prompt: PermissionPrompt,
  decidedAt: PermissionDecisionAt,
): Promise<EffectAuthorization> {
  const remembered = store.lookup(scope);
  if (!remembered.ok)
    return { status: "denied", code: "TIBER_PERMISSION_STATE_INVALID" };
  const decision = decidePermission({
    ...request,
    remembered: remembered.value,
  });
  if (decision.status === "allowed")
    return { status: "allowed", remembered: false };
  if (decision.status === "denied") return decision;

  const selected = await prompt.choose(description, decision.choices);
  if (
    selected.kind === "none" ||
    !decision.choices.some((choice) => choice === selected.value)
  )
    return { status: "denied", code: "TIBER_PERMISSION_PROMPT_CANCELLED" };
  if (selected.value === "deny-once")
    return { status: "denied", code: "TIBER_PERMISSION_PROMPT_CANCELLED" };
  if (selected.value === "allow-once")
    return { status: "allowed", remembered: false };

  const persistentDecision =
    selected.value === "allow-always" ? "allow" : "deny";
  const persisted = store.remember(scope, persistentDecision, decidedAt);
  if (!persisted.ok)
    return { status: "denied", code: "TIBER_PERMISSION_STATE_INVALID" };
  return persistentDecision === "allow"
    ? { status: "allowed", remembered: true }
    : { status: "denied", code: "TIBER_PERMISSION_ALWAYS_DENIED" };
}
