import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const configurationValuePurpose: unique symbol;
type ConfigurationValue<Value, Purpose extends string> = Value & {
  readonly [configurationValuePurpose]: Purpose;
};

export type ProjectId = ConfigurationValue<string, "project-id">;
export type OutputPreviewBytes = ConfigurationValue<
  number,
  "output-preview-bytes"
>;
export type SecretReferenceName = ConfigurationValue<
  string,
  "secret-reference-name"
>;
export type SettingsCommandValue = ConfigurationValue<
  string,
  "settings-command-value"
>;
export type AuthorityUnlockConfirmation = ConfigurationValue<
  string,
  "authority-unlock-confirmation"
>;
export type SecretEnvironmentVariableName = ConfigurationValue<
  string,
  "secret-environment-variable-name"
>;

export const OUTPUT_PREVIEW_BYTES_RANGE = {
  minimum: 1_024,
  maximum: 1_048_576,
} as const;

type Field =
  | "projectId"
  | "outputPreviewBytes"
  | "settingsCommandValue"
  | "authorityUnlockConfirmation"
  | "secretReferenceName"
  | "secretEnvironmentVariableName";
type Failure = TiberFailure<
  "TIBER_CONFIGURATION_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_CONFIGURATION_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

export function parseProjectId(value: unknown): Result<ProjectId> {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
      value,
    )
    ? { ok: true, value: value as ProjectId }
    : invalid("projectId");
}

export function parseOutputPreviewBytes(
  value: unknown,
): Result<OutputPreviewBytes> {
  // Stryker disable next-line ConditionalExpression, LogicalOperator: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= OUTPUT_PREVIEW_BYTES_RANGE.minimum &&
    value <= OUTPUT_PREVIEW_BYTES_RANGE.maximum
    ? { ok: true, value: value as OutputPreviewBytes }
    : invalid("outputPreviewBytes");
}

export function parseSettingsCommandValue(
  value: unknown,
): Result<SettingsCommandValue> {
  return typeof value === "string" && value.length > 0 && value.length <= 256
    ? { ok: true, value: value as SettingsCommandValue }
    : invalid("settingsCommandValue");
}

export function parseAuthorityUnlockConfirmation(
  value: unknown,
): Result<AuthorityUnlockConfirmation> {
  return typeof value === "string" && value.length > 0 && value.length <= 512
    ? { ok: true, value: value as AuthorityUnlockConfirmation }
    : invalid("authorityUnlockConfirmation");
}

export function parseSecretReferenceName(
  value: unknown,
): Result<SecretReferenceName> {
  return typeof value === "string" && /^[a-z][a-z0-9-]{0,63}$/u.test(value)
    ? { ok: true, value: value as SecretReferenceName }
    : invalid("secretReferenceName");
}

export function parseSecretEnvironmentVariableName(
  value: unknown,
): Result<SecretEnvironmentVariableName> {
  return typeof value === "string" && /^[A-Z][A-Z0-9_]{0,127}$/u.test(value)
    ? { ok: true, value: value as SecretEnvironmentVariableName }
    : invalid("secretEnvironmentVariableName");
}
