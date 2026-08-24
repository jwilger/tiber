import { none, some, type Option } from "../types/option.js";
import {
  parseAuthorityUnlockConfirmation,
  parseSecretEnvironmentVariableName,
  parseSecretReferenceName,
  parseSettingsCommandValue,
  type AuthorityUnlockConfirmation,
  type SecretEnvironmentVariableName,
  type SecretReferenceName,
  type SettingsCommandValue,
} from "./configuration-values.js";
import {
  settingsFailure,
  type SettingKey,
  type SettingsResult,
} from "./settings.js";

export type SettingsScope = "global" | "project";

export type SettingsCommand =
  | { readonly kind: "show" }
  | {
      readonly kind: "set";
      readonly scope: SettingsScope;
      readonly key: SettingKey;
      readonly value: SettingsCommandValue;
    }
  | { readonly kind: "lock"; readonly value: SettingsCommandValue }
  | {
      readonly kind: "unlock";
      readonly confirmation: AuthorityUnlockConfirmation;
    }
  | {
      readonly kind: "secret";
      readonly key: SecretReferenceName;
      readonly environmentName: Option<SecretEnvironmentVariableName>;
    };

function invalid(message: string): SettingsResult<never> {
  const failure = settingsFailure("TIBER_SETTINGS_INVALID_VALUE", message);
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
    const value = parseSettingsCommandValue(parts[2]);
    return value.ok
      ? { ok: true, value: { kind: "lock", value: value.value } }
      : invalid(USAGE);
  }
  if (
    operation === "unlock" &&
    parts.length === 4 &&
    parts[1] === "assuranceLevel"
  ) {
    const confirmation = parseAuthorityUnlockConfirmation(
      parts.slice(2).join(" "),
    );
    return confirmation.ok
      ? {
          ok: true,
          value: { kind: "unlock", confirmation: confirmation.value },
        }
      : invalid(USAGE);
  }
  if (operation === "secret" && parts.length === 3 && parts[2] === "inherit") {
    const key = parseSecretReferenceName(parts[1]);
    return key.ok
      ? {
          ok: true,
          value: { kind: "secret", key: key.value, environmentName: none },
        }
      : invalid(USAGE);
  }
  if (
    operation === "secret" &&
    parts.length === 4 &&
    parts[2] === "environment"
  ) {
    const key = parseSecretReferenceName(parts[1]);
    const environmentName = parseSecretEnvironmentVariableName(parts[3]);
    return key.ok && environmentName.ok
      ? {
          ok: true,
          value: {
            kind: "secret",
            key: key.value,
            environmentName: some(environmentName.value),
          },
        }
      : invalid(USAGE);
  }
  if (parts.length !== 4 || operation !== "set") {
    return invalid(USAGE);
  }

  const scope = parts[1];
  const key = parts[2];
  const value = parseSettingsCommandValue(parts[3]);
  // Stryker disable next-line ConditionalExpression: the exact four-part shape check establishes this index; undefined is also rejected by isScope.
  if (scope === undefined || !isScope(scope)) {
    return invalid("settings scope must be global or project");
  }
  // Stryker disable next-line ConditionalExpression, StringLiteral: the exact four-part shape check establishes this index; undefined is also rejected by isKey, so the missing-label fallback cannot be observed.
  if (key === undefined || !isKey(key)) {
    // Stryker disable next-line StringLiteral: exact four-part parsing makes the missing-key label unreachable; malformed present keys are observed verbatim.
    return invalid(`unknown setting: ${key ?? "(missing)"}`);
  }

  return value.ok
    ? {
        ok: true,
        value: { kind: "set", scope, key, value: value.value },
      }
    : invalid(USAGE);
}
