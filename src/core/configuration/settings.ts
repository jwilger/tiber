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
  readonly assuranceLevel?: AssuranceLevel;
  readonly outputPreviewBytes?: number;
  readonly worktreeMode?: WorktreeMode;
}

export interface EffectiveSetting<T> {
  readonly value: T;
  readonly source: SettingSource;
}

export interface EffectiveSettings {
  readonly assuranceLevel: EffectiveSetting<AssuranceLevel>;
  readonly outputPreviewBytes: EffectiveSetting<number>;
  readonly worktreeMode: EffectiveSetting<WorktreeMode>;
}

export interface SettingsDocument {
  readonly schemaVersion: 1;
  readonly values: SettingsOverrides;
}

export interface SettingsFailure {
  readonly code:
    | "TIBER_SETTINGS_INVALID_DOCUMENT"
    | "TIBER_SETTINGS_INVALID_KEY"
    | "TIBER_SETTINGS_INVALID_VALUE"
    | "TIBER_SETTINGS_IO"
    | "TIBER_SETTINGS_REPOSITORY_REQUIRED";
  readonly message: string;
  readonly retryable: boolean;
}

export type SettingsResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly failure: SettingsFailure };

export const BUILT_IN_SETTINGS = {
  assuranceLevel: "host-trusted",
  outputPreviewBytes: 16_384,
  worktreeMode: "isolated",
} as const satisfies Required<SettingsOverrides>;

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
    failure: {
      code: "TIBER_SETTINGS_INVALID_DOCUMENT",
      message,
      retryable: false,
    },
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
  if (
    outputPreviewBytes !== undefined &&
    // Stryker disable next-line ConditionalExpression: Number.isInteger also rejects every non-number, so removing only the typeof term is an equivalent mutant; the explicit guard narrows unknown before numeric comparisons.
    (typeof outputPreviewBytes !== "number" ||
      !Number.isInteger(outputPreviewBytes) ||
      outputPreviewBytes < 1024 ||
      outputPreviewBytes > 1_048_576)
  ) {
    return invalidDocument(
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    );
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
        ...(assuranceLevel === undefined ? {} : { assuranceLevel }),
        ...(outputPreviewBytes === undefined ? {} : { outputPreviewBytes }),
        ...(worktreeMode === undefined ? {} : { worktreeMode }),
      },
    },
  };
}

function effective<T>(
  builtIn: T,
  globalValue: T | undefined,
  projectValue: T | undefined,
): EffectiveSetting<T> {
  if (projectValue !== undefined) {
    return { value: projectValue, source: "project" };
  }
  if (globalValue !== undefined) {
    return { value: globalValue, source: "user-global" };
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
    failure: {
      code: "TIBER_SETTINGS_INVALID_KEY",
      message: `unknown setting: ${key}`,
      retryable: false,
    },
  };
}

function invalidValue(key: SettingKey, value: string): SettingsResult<never> {
  return {
    ok: false,
    failure: {
      code: "TIBER_SETTINGS_INVALID_VALUE",
      message: `invalid value for ${key}: ${value}`,
      retryable: false,
    },
  };
}

function withoutSetting(
  current: SettingsOverrides,
  key: SettingKey,
): SettingsOverrides {
  return {
    ...(key === "assuranceLevel" || current.assuranceLevel === undefined
      ? {}
      : { assuranceLevel: current.assuranceLevel }),
    ...(key === "outputPreviewBytes" || current.outputPreviewBytes === undefined
      ? {}
      : { outputPreviewBytes: current.outputPreviewBytes }),
    ...(key === "worktreeMode" || current.worktreeMode === undefined
      ? {}
      : { worktreeMode: current.worktreeMode }),
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
    return { ok: true, value: { ...current, assuranceLevel: rawValue } };
  }

  if (key === "outputPreviewBytes") {
    if (rawValue === "inherit") {
      return { ok: true, value: withoutSetting(current, key) };
    }
    const parsed = Number(rawValue);
    if (!Number.isSafeInteger(parsed) || parsed < 1024 || parsed > 1_048_576) {
      return invalidValue(key, rawValue);
    }
    return { ok: true, value: { ...current, outputPreviewBytes: parsed } };
  }

  if (key === "worktreeMode") {
    if (rawValue === "inherit") {
      return { ok: true, value: withoutSetting(current, key) };
    }
    if (!isWorktreeMode(rawValue)) {
      return invalidValue(key, rawValue);
    }
    return { ok: true, value: { ...current, worktreeMode: rawValue } };
  }

  return invalidKey(key);
}

export function formatSettingsTable(
  globalValues: SettingsOverrides,
  projectValues: SettingsOverrides,
): string {
  const resolved = resolveSettings(globalValues, projectValues);
  const display = (value: string | number | undefined): string =>
    value === undefined ? "inherit" : String(value);

  return [
    "Setting | Built-in | User global | Project | Effective (source)",
    `assuranceLevel | ${BUILT_IN_SETTINGS.assuranceLevel} | ${display(globalValues.assuranceLevel)} | ${display(projectValues.assuranceLevel)} | ${resolved.assuranceLevel.value} (${resolved.assuranceLevel.source})`,
    `outputPreviewBytes | ${display(BUILT_IN_SETTINGS.outputPreviewBytes)} | ${display(globalValues.outputPreviewBytes)} | ${display(projectValues.outputPreviewBytes)} | ${display(resolved.outputPreviewBytes.value)} (${resolved.outputPreviewBytes.source})`,
    `worktreeMode | ${BUILT_IN_SETTINGS.worktreeMode} | ${display(globalValues.worktreeMode)} | ${display(projectValues.worktreeMode)} | ${resolved.worktreeMode.value} (${resolved.worktreeMode.source})`,
  ].join("\n");
}
