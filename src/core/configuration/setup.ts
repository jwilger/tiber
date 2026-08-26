import type { CompiledCommandCatalog } from "../commands/structured-command.js";
import { compileCommandCatalog } from "../commands/structured-command.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";
import { none, some, type Option } from "../types/option.js";
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

export interface SetupPlan {
  readonly globalSettings: SettingsOverrides;
  readonly projectSettings: SettingsOverrides;
  readonly authority: AuthorityDocument;
  readonly commandCatalog: Option<CompiledCommandCatalog>;
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
): SetupResult<Option<CompiledCommandCatalog>> {
  if (!record(input) || typeof input.action !== "string") {
    return invalid("setup plan must choose how to configure project commands");
  }
  if (input.action === "keep" && Object.keys(input).length === 1) {
    return { ok: true, value: none };
  }
  if (
    input.action !== "replace" ||
    Object.keys(input).sort().join(",") !== "action,definition"
  ) {
    return invalid("setup command-catalog choice is invalid");
  }
  const catalog = compileCommandCatalog(input.definition);
  return catalog.ok
    ? { ok: true, value: some(catalog.value) }
    : invalid(catalog.failure.message);
}

function effectiveAssurance(plan: SetupPlan) {
  const requested = resolveSettings(plan.globalSettings, plan.projectSettings)
    .assuranceLevel.value;
  return applyAssuranceCeiling(
    requested,
    plan.authority.ceilings.minimumAssuranceLevel,
  ).effective;
}

function minimumAssuranceRank(minimum: Option<AssuranceLevel>): number {
  // Stryker disable next-line ConditionalExpression: Option.none has no value and Array.indexOf(undefined) would also produce the explicit absent rank -1; the branch preserves semantic Option access.
  return minimum.kind === "some" ? ASSURANCE_LEVELS.indexOf(minimum.value) : -1;
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

  const currentEffective = effectiveAssurance(current);
  const proposedEffective = effectiveAssurance(proposed);
  if (
    ASSURANCE_LEVELS.indexOf(proposedEffective) <
    ASSURANCE_LEVELS.indexOf(currentEffective)
  ) {
    confirmations.push(
      `apply weaker assurance current=${currentEffective} proposed=${proposedEffective}`,
    );
  }
  return confirmations;
}

export function parseSetupPlan(input: unknown): SetupResult<SetupPlan> {
  if (
    !record(input) ||
    Object.keys(input).sort().join(",") !==
      "commandCatalog,globalSettings,minimumAssuranceLevel,projectSettings,schemaVersion,secretReferences" ||
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

  return {
    ok: true,
    value: {
      globalSettings: globalSettings.value,
      projectSettings: projectSettings.value,
      authority: authority.value,
      commandCatalog: commandCatalog.value,
    },
  };
}
