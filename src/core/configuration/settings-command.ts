import type {
  SettingKey,
  SettingsFailure,
  SettingsResult,
} from "./settings.js";

export type SettingsScope = "global" | "project";

export type SettingsCommand =
  | { readonly kind: "show" }
  | {
      readonly kind: "set";
      readonly scope: SettingsScope;
      readonly key: SettingKey;
      readonly value: string;
    };

function invalid(message: string): SettingsResult<never> {
  const failure: SettingsFailure = {
    code: "TIBER_SETTINGS_INVALID_VALUE",
    message,
    retryable: false,
  };
  return { ok: false, failure };
}

function isScope(value: string): value is SettingsScope {
  return value === "global" || value === "project";
}

function isKey(value: string): value is SettingKey {
  return (
    value === "assuranceLevel" ||
    value === "outputPreviewBytes" ||
    value === "worktreeMode"
  );
}

const USAGE =
  "usage: /tiber:settings [show | set <global|project> <setting> <value|inherit>]";

export function parseSettingsCommand(
  argumentsText: string,
): SettingsResult<SettingsCommand> {
  const normalized = argumentsText.trim();
  if (normalized.length === 0 || normalized === "show") {
    return { ok: true, value: { kind: "show" } };
  }

  const parts = normalized.split(/\s+/u);
  if (parts.length !== 4) {
    return invalid(USAGE);
  }

  const [operation, scope, key, value] = parts as [
    string,
    string,
    string,
    string,
  ];
  if (operation !== "set") {
    return invalid(USAGE);
  }
  if (!isScope(scope)) {
    return invalid("settings scope must be global or project");
  }
  if (!isKey(key)) {
    return invalid(`unknown setting: ${key}`);
  }

  return {
    ok: true,
    value: { kind: "set", scope, key, value },
  };
}
