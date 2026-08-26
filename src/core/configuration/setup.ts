import { createHash } from "node:crypto";

import type { CompiledCommandCatalog } from "../commands/structured-command.js";
import { compileCommandCatalog } from "../commands/structured-command.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";
import { none, type Option } from "../types/option.js";
import {
  compileWorkflow,
  type CompiledWorkflow,
} from "../workflow/workflow.js";
import {
  parseSetupExpectedAuthorityDigest,
  parseSetupPlanDigest,
  type SetupExpectedAuthorityDigest,
  type SetupPlanDigest,
} from "./setup-values.js";
import {
  applyAssuranceCeiling,
  parseAuthorityDocument,
  type AuthorityDocument,
} from "./authority.js";
import {
  ASSURANCE_LEVELS,
  parseSettingsDocument,
  resolveSettings,
  type AssuranceLevel,
  type SettingsOverrides,
} from "./settings.js";

export type SetupCommandCatalogChoice =
  | { readonly kind: "keep" }
  | { readonly kind: "remove" }
  | {
      readonly kind: "replace";
      readonly catalog: CompiledCommandCatalog;
    };

export type SetupProjectWorkflowChoice =
  | { readonly kind: "keep" }
  | { readonly kind: "built-in" }
  | { readonly kind: "replace"; readonly workflow: CompiledWorkflow };

export const SETUP_PLAN_LIMITS = {
  maximumSecretReferences: 64,
} as const;

export interface SetupPlan {
  readonly globalSettings: SettingsOverrides;
  readonly projectSettings: SettingsOverrides;
  readonly authority: AuthorityDocument;
  readonly commandCatalog: SetupCommandCatalogChoice;
  readonly projectWorkflow: SetupProjectWorkflowChoice;
}

type SetupFailure = TiberFailure<
  "TIBER_SETUP_PLAN_INVALID",
  { readonly domain: "setup" },
  "corrected-input" | "state-change" | "retry-operation"
>;
export type SetupResult<Value> = TiberResult<Value, SetupFailure>;

function invalid(message: string): SetupResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      "TIBER_SETUP_PLAN_INVALID",
      "setup",
      message,
      "retry-after-input",
    ),
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const SETTING_KEYS = [
  "assuranceLevel",
  "outputPreviewBytes",
  "worktreeMode",
] as const;

function parseCompleteSettings(
  input: unknown,
  scope: "global" | "project",
): SetupResult<SettingsOverrides> {
  if (
    !record(input) ||
    Object.keys(input).sort().join(",") !==
      "assuranceLevel,outputPreviewBytes,worktreeMode"
  ) {
    return invalid(`setup plan must answer every ${scope} setting`);
  }

  const values = Object.fromEntries(
    SETTING_KEYS.flatMap((key) =>
      input[key] === "inherit" ? [] : [[key, input[key]]],
    ),
  );
  const parsed = parseSettingsDocument({ schemaVersion: 1, values });
  return parsed.ok
    ? { ok: true, value: parsed.value.values }
    : invalid(`setup plan contains invalid ${scope} settings`);
}

function parseAuthority(
  minimumAssuranceLevel: unknown,
  secretReferences: unknown,
): SetupResult<AuthorityDocument> {
  if (
    !record(secretReferences) ||
    Object.keys(secretReferences).length >
      SETUP_PLAN_LIMITS.maximumSecretReferences
  )
    return invalid("setup plan contains invalid authority settings");
  const parsed = parseAuthorityDocument({
    schemaVersion: 1,
    ceilings:
      minimumAssuranceLevel === "unlocked" ? {} : { minimumAssuranceLevel },
    secretReferences,
  });
  return parsed.ok
    ? parsed
    : invalid("setup plan contains invalid authority settings");
}

function parseCommandCatalog(
  input: unknown,
): SetupResult<SetupCommandCatalogChoice> {
  if (!record(input) || typeof input.action !== "string") {
    return invalid("setup plan must choose how to configure project commands");
  }
  if (input.action === "keep" && Object.keys(input).length === 1) {
    return { ok: true, value: { kind: "keep" } };
  }
  if (input.action === "remove" && Object.keys(input).length === 1) {
    return { ok: true, value: { kind: "remove" } };
  }
  if (
    input.action !== "replace" ||
    Object.keys(input).sort().join(",") !== "action,definition"
  ) {
    return invalid("setup command-catalog choice is invalid");
  }
  const catalog = compileCommandCatalog(input.definition);
  return catalog.ok
    ? { ok: true, value: { kind: "replace", catalog: catalog.value } }
    : invalid(catalog.failure.message);
}

function parseProjectWorkflow(
  input: unknown,
): SetupResult<SetupProjectWorkflowChoice> {
  if (!record(input) || typeof input.action !== "string") {
    return invalid("setup plan must choose how to configure project workflow");
  }
  if (input.action === "keep" && Object.keys(input).length === 1) {
    return { ok: true, value: { kind: "keep" } };
  }
  if (input.action === "built-in" && Object.keys(input).length === 1) {
    return { ok: true, value: { kind: "built-in" } };
  }
  if (
    input.action !== "replace" ||
    Object.keys(input).sort().join(",") !== "action,definition"
  ) {
    return invalid("setup project-workflow choice is invalid");
  }
  const workflow = compileWorkflow(input.definition);
  return workflow.ok
    ? { ok: true, value: { kind: "replace", workflow: workflow.value } }
    : invalid(workflow.failure.message);
}

function repositoryAssurance(plan: SetupPlan): AssuranceLevel {
  const requested = resolveSettings(plan.globalSettings, plan.projectSettings)
    .assuranceLevel.value;
  return applyAssuranceCeiling(
    requested,
    plan.authority.ceilings.minimumAssuranceLevel,
  ).effective;
}

function globalAssurance(plan: SetupPlan): AssuranceLevel {
  const inheritedProject: SettingsOverrides = {
    assuranceLevel: none,
    outputPreviewBytes: none,
    worktreeMode: none,
  };
  return resolveSettings(plan.globalSettings, inheritedProject).assuranceLevel
    .value;
}

function minimumAssuranceRank(
  minimum: AuthorityDocument["ceilings"]["minimumAssuranceLevel"],
): number {
  // Stryker disable next-line ConditionalExpression: Option.none has no value and Array.indexOf(undefined) would also produce the explicit absent rank -1; the branch preserves semantic Option access.
  return minimum.kind === "some" ? ASSURANCE_LEVELS.indexOf(minimum.value) : -1;
}

function weakened(current: AssuranceLevel, proposed: AssuranceLevel): boolean {
  return ASSURANCE_LEVELS.indexOf(proposed) < ASSURANCE_LEVELS.indexOf(current);
}

export function requiredSetupConfirmations(
  current: SetupPlan,
  proposed: SetupPlan,
): readonly string[] {
  const confirmations: string[] = [];
  const currentMinimum = current.authority.ceilings.minimumAssuranceLevel;
  const proposedMinimum = proposed.authority.ceilings.minimumAssuranceLevel;
  if (
    minimumAssuranceRank(proposedMinimum) < minimumAssuranceRank(currentMinimum)
  ) {
    // Stryker disable next-line ConditionalExpression: a lower proposed rank proves the current minimum is present; this guard carries that Option proof without a cast.
    if (currentMinimum.kind === "some")
      confirmations.push(
        `unlock minimumAssuranceLevel=${currentMinimum.value}`,
      );
  }

  const currentGlobal = globalAssurance(current);
  const proposedGlobal = globalAssurance(proposed);
  const globalWeakened = weakened(currentGlobal, proposedGlobal);
  if (globalWeakened) {
    confirmations.push(
      `apply weaker global assurance current=${currentGlobal} proposed=${proposedGlobal}`,
    );
  }

  const currentRepository = repositoryAssurance(current);
  const proposedRepository = repositoryAssurance(proposed);
  if (
    weakened(currentRepository, proposedRepository) &&
    (!globalWeakened ||
      currentRepository !== currentGlobal ||
      proposedRepository !== proposedGlobal)
  ) {
    confirmations.push(
      `apply weaker project assurance current=${currentRepository} proposed=${proposedRepository}`,
    );
  }
  return confirmations;
}

function sameSetting<Value>(
  left:
    | { readonly kind: "none" }
    | { readonly kind: "some"; readonly value: Value },
  right:
    | { readonly kind: "none" }
    | { readonly kind: "some"; readonly value: Value },
): boolean {
  if (left.kind === "none") return right.kind === "none";
  // Stryker disable next-line ConditionalExpression, StringLiteral: Option.none has no value, so comparing the present left value to its absent runtime value also returns false; the branch preserves semantic Option access.
  if (right.kind === "none") return false;
  return left.value === right.value;
}

function sameSettings(left: SettingsOverrides, right: SettingsOverrides) {
  return (
    sameSetting(left.assuranceLevel, right.assuranceLevel) &&
    sameSetting(left.outputPreviewBytes, right.outputPreviewBytes) &&
    sameSetting(left.worktreeMode, right.worktreeMode)
  );
}

function sameSecretReferences(
  left: AuthorityDocument,
  right: AuthorityDocument,
): boolean {
  const leftReferences = Object.entries(left.secretReferences);
  const rightReferences = Object.entries(right.secretReferences);
  return (
    leftReferences.length === rightReferences.length &&
    leftReferences.every(([leftName, leftReference]) =>
      rightReferences.some(
        ([rightName, rightReference]) =>
          leftName === rightName && leftReference.name === rightReference.name,
      ),
    )
  );
}

function sameAuthority(
  left: AuthorityDocument,
  right: AuthorityDocument,
): boolean {
  return (
    sameSetting(
      left.ceilings.minimumAssuranceLevel,
      right.ceilings.minimumAssuranceLevel,
    ) && sameSecretReferences(left, right)
  );
}

export function sameSetupAuthorityState(
  left: SetupPlan,
  right: SetupPlan,
): boolean {
  return (
    sameSettings(left.globalSettings, right.globalSettings) &&
    sameSettings(left.projectSettings, right.projectSettings) &&
    sameAuthority(left.authority, right.authority)
  );
}

export function setupAuthorityStateCanReconcile(
  expected: SetupPlan,
  intended: SetupPlan,
  observed: SetupPlan,
): boolean {
  return (
    (sameSettings(observed.globalSettings, expected.globalSettings) ||
      sameSettings(observed.globalSettings, intended.globalSettings)) &&
    (sameSettings(observed.projectSettings, expected.projectSettings) ||
      sameSettings(observed.projectSettings, intended.projectSettings)) &&
    (sameAuthority(observed.authority, expected.authority) ||
      sameAuthority(observed.authority, intended.authority))
  );
}

function formatSetting<Value extends string | number>(
  value: Option<Value>,
): Value | "inherit" {
  return value.kind === "some" ? value.value : "inherit";
}

export function formatSetupPlan(plan: SetupPlan) {
  const commandCatalog =
    plan.commandCatalog.kind === "replace"
      ? {
          action: "replace" as const,
          definition: {
            schemaVersion: plan.commandCatalog.catalog.schemaVersion,
            commands: plan.commandCatalog.catalog.commands,
          },
        }
      : { action: plan.commandCatalog.kind };
  const projectWorkflow =
    plan.projectWorkflow.kind === "replace"
      ? {
          action: "replace" as const,
          definition: plan.projectWorkflow.workflow.definition,
        }
      : { action: plan.projectWorkflow.kind };
  return {
    schemaVersion: 1 as const,
    globalSettings: {
      assuranceLevel: formatSetting(plan.globalSettings.assuranceLevel),
      outputPreviewBytes: formatSetting(plan.globalSettings.outputPreviewBytes),
      worktreeMode: formatSetting(plan.globalSettings.worktreeMode),
    },
    projectSettings: {
      assuranceLevel: formatSetting(plan.projectSettings.assuranceLevel),
      outputPreviewBytes: formatSetting(
        plan.projectSettings.outputPreviewBytes,
      ),
      worktreeMode: formatSetting(plan.projectSettings.worktreeMode),
    },
    minimumAssuranceLevel:
      plan.authority.ceilings.minimumAssuranceLevel.kind === "some"
        ? plan.authority.ceilings.minimumAssuranceLevel.value
        : ("unlocked" as const),
    secretReferences: Object.fromEntries(
      Object.entries(plan.authority.secretReferences).sort(([left], [right]) =>
        left.localeCompare(right),
      ),
    ),
    commandCatalog,
    projectWorkflow,
  };
}

function setupPlanHash(plan: SetupPlan): string {
  return `sha256:${createHash("sha256")
    .update(JSON.stringify(formatSetupPlan(plan)))
    .digest("hex")}`;
}

export function digestSetupPlan(plan: SetupPlan): SetupPlanDigest {
  const digest = parseSetupPlanDigest(setupPlanHash(plan));
  // Stryker disable next-line ConditionalExpression, BlockStatement: SHA-256 output always satisfies the setup-plan digest parser; rejection is an internal defect.
  if (!digest.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: validated SHA-256 generation makes this defect throw unreachable.
    throw new Error("generated setup plan digest is invalid");
  }
  return digest.value;
}

export function digestSetupExpectedAuthority(
  plan: SetupPlan,
): SetupExpectedAuthorityDigest {
  const digest = parseSetupExpectedAuthorityDigest(setupPlanHash(plan));
  // Stryker disable next-line ConditionalExpression, BlockStatement: SHA-256 output always satisfies the expected-authority digest parser; rejection is an internal defect.
  if (!digest.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: validated SHA-256 generation makes this defect throw unreachable.
    throw new Error("generated setup expected-authority digest is invalid");
  }
  return digest.value;
}

export function parseSetupPlan(input: unknown): SetupResult<SetupPlan> {
  if (
    !record(input) ||
    Object.keys(input).sort().join(",") !==
      "commandCatalog,globalSettings,minimumAssuranceLevel,projectSettings,projectWorkflow,schemaVersion,secretReferences" ||
    input.schemaVersion !== 1
  ) {
    return invalid("setup plan must use the complete schema version 1 shape");
  }

  const globalSettings = parseCompleteSettings(input.globalSettings, "global");
  if (!globalSettings.ok) return globalSettings;
  const projectSettings = parseCompleteSettings(
    input.projectSettings,
    "project",
  );
  if (!projectSettings.ok) return projectSettings;
  const authority = parseAuthority(
    input.minimumAssuranceLevel,
    input.secretReferences,
  );
  if (!authority.ok) return authority;
  const commandCatalog = parseCommandCatalog(input.commandCatalog);
  if (!commandCatalog.ok) return commandCatalog;
  const projectWorkflow = parseProjectWorkflow(input.projectWorkflow);
  if (!projectWorkflow.ok) return projectWorkflow;

  return {
    ok: true,
    value: {
      globalSettings: globalSettings.value,
      projectSettings: projectSettings.value,
      authority: authority.value,
      commandCatalog: commandCatalog.value,
      projectWorkflow: projectWorkflow.value,
    },
  };
}
