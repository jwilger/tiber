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
    }
  | { readonly kind: "lock"; readonly value: string }
  | { readonly kind: "unlock"; readonly confirmation: string }
  | {
      readonly kind: "secret";
      readonly key: string;
      readonly environmentName?: string;
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
  "usage: /tiber:settings [show | set <global|project> <setting> <value|inherit> | lock assuranceLevel <value> | unlock assuranceLevel <exact-confirmation> | secret <key> <environment NAME|inherit>]";

export function parseSettingsCommand(
  argumentsText: string,
): SettingsResult<SettingsCommand> {
  const normalized = argumentsText.trim();
  if (normalized.length === 0 || normalized === "show") {
    return { ok: true, value: { kind: "show" } };
  }

  const parts = normalized.split(/\s+/u);
  const operation = parts[0];
  if (
    operation === "lock" &&
    parts.length === 3 &&
    parts[1] === "assuranceLevel"
  ) {
    const [, , value] = parts as [string, string, string];
    return { ok: true, value: { kind: "lock", value } };
  }
  if (
    operation === "unlock" &&
    parts.length === 4 &&
    parts[1] === "assuranceLevel"
  ) {
    return {
      ok: true,
      value: { kind: "unlock", confirmation: parts.slice(2).join(" ") },
    };
  }
  if (operation === "secret" && parts.length === 3 && parts[2] === "inherit") {
    const [, key] = parts as [string, string, string];
    return { ok: true, value: { kind: "secret", key } };
  }
  if (
    operation === "secret" &&
    parts.length === 4 &&
    parts[2] === "environment"
  ) {
    const [, key, , environmentName] = parts as [
      string,
      string,
      string,
      string,
    ];
    return {
      ok: true,
      value: { kind: "secret", key, environmentName },
    };
  }
  if (parts.length !== 4 || operation !== "set") {
    return invalid(USAGE);
  }

  const [, scope, key, value] = parts as [string, string, string, string];
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
