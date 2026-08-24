import type { TiberFailure } from "../failures/tiber-failure.js";
import { none, some, type Option } from "../types/option.js";
import {
  parseOutputPreviewBytes,
  type OutputPreviewBytes,
} from "./configuration-values.js";

export const ASSURANCE_LEVELS = [
  "host-trusted",
  "workspace-isolated",
  "workspace-and-network-isolated",
  "hermetic",
] as const;

export const WORKTREE_MODES = ["isolated", "current"] as const;

export type AssuranceLevel = (typeof ASSURANCE_LEVELS)[number];
export type WorktreeMode = (typeof WORKTREE_MODES)[number];
export type SettingKey =
  "assuranceLevel" | "outputPreviewBytes" | "worktreeMode";
export type SettingSource = "built-in" | "user-global" | "project";

export interface SettingsOverrides {
  readonly assuranceLevel: Option<AssuranceLevel>;
  readonly outputPreviewBytes: Option<OutputPreviewBytes>;
  readonly worktreeMode: Option<WorktreeMode>;
}

export interface EffectiveSetting<T> {
  readonly value: T;
  readonly source: SettingSource;
}

export interface EffectiveSettings {
  readonly assuranceLevel: EffectiveSetting<AssuranceLevel>;
  readonly outputPreviewBytes: EffectiveSetting<OutputPreviewBytes>;
  readonly worktreeMode: EffectiveSetting<WorktreeMode>;
}

export interface SettingsDocument {
  readonly schemaVersion: 1;
  readonly values: SettingsOverrides;
}

type SettingsFailureCode =
  | "TIBER_SETTINGS_INVALID_DOCUMENT"
  | "TIBER_SETTINGS_INVALID_KEY"
  | "TIBER_SETTINGS_INVALID_VALUE"
  | "TIBER_SETTINGS_IO"
  | "TIBER_SETTINGS_REPOSITORY_REQUIRED";

export type SettingsFailure = TiberFailure<
  SettingsFailureCode,
  { readonly domain: "settings" },
  "corrected-settings" | "repository-required" | "retry-operation"
>;

export function settingsFailure(
  code: SettingsFailureCode,
  message: string,
): SettingsFailure {
  const retryable = code === "TIBER_SETTINGS_IO";
  return {
    code,
    message,
    safeContext: { domain: "settings" },
    causes: [],
    retryability: retryable
      ? "transient"
      : code === "TIBER_SETTINGS_REPOSITORY_REQUIRED"
        ? "retry-after-state-change"
        : "retry-after-input",
    requiredRecoveryEvidence: retryable
      ? ["retry-operation"]
      : code === "TIBER_SETTINGS_REPOSITORY_REQUIRED"
        ? ["repository-required"]
        : ["corrected-settings"],
    redaction: "public",
  };
}

export type SettingsResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly failure: SettingsFailure };

const builtInPreview = parseOutputPreviewBytes(16_384);
// Stryker disable next-line ConditionalExpression, BooleanLiteral, BlockStatement: the literal built-in lies within the parser's bounds; rejection is an internal defect.
if (!builtInPreview.ok) {
  // Stryker disable next-line StringLiteral, CallExpression: the validated literal makes this defect throw unreachable.
  throw new Error("built-in output preview violated its invariant");
}

export const BUILT_IN_SETTINGS = {
  assuranceLevel: "host-trusted",
  outputPreviewBytes: builtInPreview.value,
  worktreeMode: "isolated",
} as const;

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: removing the typeof guard is behaviorally equivalent because every primitive subsequently fails the required document-shape check; the guard exists to establish the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAssuranceLevel(value: unknown): value is AssuranceLevel {
  return ASSURANCE_LEVELS.some((candidate) => candidate === value);
}

function isWorktreeMode(value: unknown): value is WorktreeMode {
  return WORKTREE_MODES.some((candidate) => candidate === value);
}

function invalidDocument(message: string): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure("TIBER_SETTINGS_INVALID_DOCUMENT", message),
  };
}

export function parseSettingsDocument(
  input: unknown,
): SettingsResult<SettingsDocument> {
  if (
    !isRecord(input) ||
    input.schemaVersion !== 1 ||
    !isRecord(input.values)
  ) {
    return invalidDocument(
      "settings must use schema version 1 and an object of values",
    );
  }

  const allowedKeys = new Set<string>([
    "assuranceLevel",
    "outputPreviewBytes",
    "worktreeMode",
  ]);
  const unknownKey = Object.keys(input.values).find(
    (key) => !allowedKeys.has(key),
  );
  if (unknownKey !== undefined) {
    return invalidDocument(`unknown setting: ${unknownKey}`);
  }

  const assuranceLevel = input.values.assuranceLevel;
  if (assuranceLevel !== undefined && !isAssuranceLevel(assuranceLevel)) {
    return invalidDocument("assuranceLevel is invalid");
  }

  const outputPreviewBytes = input.values.outputPreviewBytes;
  let semanticPreview: Option<OutputPreviewBytes> = none;
  if (outputPreviewBytes !== undefined) {
    const parsedPreview = parseOutputPreviewBytes(outputPreviewBytes);
    if (!parsedPreview.ok)
      return invalidDocument(
        "outputPreviewBytes must be an integer from 1024 to 1048576",
      );
    semanticPreview = some(parsedPreview.value);
  }

  const worktreeMode = input.values.worktreeMode;
  if (worktreeMode !== undefined && !isWorktreeMode(worktreeMode)) {
    return invalidDocument("worktreeMode is invalid");
  }

  return {
    ok: true,
    value: {
      schemaVersion: 1,
      values: {
        assuranceLevel:
          assuranceLevel === undefined ? none : some(assuranceLevel),
        outputPreviewBytes: semanticPreview,
        worktreeMode: worktreeMode === undefined ? none : some(worktreeMode),
      },
    },
  };
}

function effective<T>(
  builtIn: T,
  globalValue: Option<T>,
  projectValue: Option<T>,
): EffectiveSetting<T> {
  if (projectValue.kind === "some") {
    return { value: projectValue.value, source: "project" };
  }
  if (globalValue.kind === "some") {
    return { value: globalValue.value, source: "user-global" };
  }
  return { value: builtIn, source: "built-in" };
}

export function resolveSettings(
  globalValues: SettingsOverrides,
  projectValues: SettingsOverrides,
): EffectiveSettings {
  return {
    assuranceLevel: effective(
      BUILT_IN_SETTINGS.assuranceLevel,
      globalValues.assuranceLevel,
      projectValues.assuranceLevel,
    ),
    outputPreviewBytes: effective(
      BUILT_IN_SETTINGS.outputPreviewBytes,
      globalValues.outputPreviewBytes,
      projectValues.outputPreviewBytes,
    ),
    worktreeMode: effective(
      BUILT_IN_SETTINGS.worktreeMode,
      globalValues.worktreeMode,
      projectValues.worktreeMode,
    ),
  };
}

function invalidKey(key: string): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure(
      "TIBER_SETTINGS_INVALID_KEY",
      `unknown setting: ${key}`,
    ),
  };
}

function invalidValue(key: SettingKey, value: string): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure(
      "TIBER_SETTINGS_INVALID_VALUE",
      `invalid value for ${key}: ${value}`,
    ),
  };
}

function withoutSetting(
  current: SettingsOverrides,
  key: SettingKey,
): SettingsOverrides {
  return {
    assuranceLevel: key === "assuranceLevel" ? none : current.assuranceLevel,
    outputPreviewBytes:
      key === "outputPreviewBytes" ? none : current.outputPreviewBytes,
    worktreeMode: key === "worktreeMode" ? none : current.worktreeMode,
  };
}

export function setSetting(
  current: SettingsOverrides,
  key: string,
  rawValue: string,
): SettingsResult<SettingsOverrides> {
  if (key === "assuranceLevel") {
    if (rawValue === "inherit") {
      return { ok: true, value: withoutSetting(current, key) };
    }
    if (!isAssuranceLevel(rawValue)) {
      return invalidValue(key, rawValue);
    }
    return {
      ok: true,
      value: { ...current, assuranceLevel: some(rawValue) },
    };
  }

  if (key === "outputPreviewBytes") {
    if (rawValue === "inherit") {
      return { ok: true, value: withoutSetting(current, key) };
    }
    const parsed = parseOutputPreviewBytes(Number(rawValue));
    if (!parsed.ok) return invalidValue(key, rawValue);
    return {
      ok: true,
      value: { ...current, outputPreviewBytes: some(parsed.value) },
    };
  }

  if (key === "worktreeMode") {
    if (rawValue === "inherit") {
      return { ok: true, value: withoutSetting(current, key) };
    }
    if (!isWorktreeMode(rawValue)) {
      return invalidValue(key, rawValue);
    }
    return {
      ok: true,
      value: { ...current, worktreeMode: some(rawValue) },
    };
  }

  return invalidKey(key);
}

export function formatSettingsTable(
  globalValues: SettingsOverrides,
  projectValues: SettingsOverrides,
): string {
  const resolved = resolveSettings(globalValues, projectValues);
  const display = <Value extends string | number>(
    value: Option<Value>,
  ): string => (value.kind === "none" ? "inherit" : String(value.value));

  return [
    "Setting | Built-in | User global | Project | Effective (source)",
    `assuranceLevel | ${BUILT_IN_SETTINGS.assuranceLevel} | ${display(globalValues.assuranceLevel)} | ${display(projectValues.assuranceLevel)} | ${resolved.assuranceLevel.value} (${resolved.assuranceLevel.source})`,
    `outputPreviewBytes | ${String(BUILT_IN_SETTINGS.outputPreviewBytes)} | ${display(globalValues.outputPreviewBytes)} | ${display(projectValues.outputPreviewBytes)} | ${String(resolved.outputPreviewBytes.value)} (${resolved.outputPreviewBytes.source})`,
    `worktreeMode | ${BUILT_IN_SETTINGS.worktreeMode} | ${display(globalValues.worktreeMode)} | ${display(projectValues.worktreeMode)} | ${resolved.worktreeMode.value} (${resolved.worktreeMode.source})`,
  ].join("\n");
}
