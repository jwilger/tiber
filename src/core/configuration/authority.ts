import { none, some, type Option } from "../types/option.js";
import {
  parseSecretEnvironmentVariableName,
  parseSecretReferenceName,
  type AuthorityUnlockConfirmation,
  type SecretEnvironmentVariableName,
  type SecretReferenceName,
} from "./configuration-values.js";
import {
  ASSURANCE_LEVELS,
  settingsFailure,
  type AssuranceLevel,
  type SettingsResult,
} from "./settings.js";

export interface SecretReference {
  readonly provider: "environment";
  readonly name: SecretEnvironmentVariableName;
}

export interface AuthorityDocument {
  readonly schemaVersion: 1;
  readonly ceilings: {
    readonly minimumAssuranceLevel: Option<AssuranceLevel>;
  };
  readonly secretReferences: Readonly<
    Record<SecretReferenceName, SecretReference>
  >;
}

export interface AssuranceDecision {
  readonly requested: AssuranceLevel;
  readonly effective: AssuranceLevel;
  readonly conflict: Option<string>;
}

export const EMPTY_AUTHORITY: AuthorityDocument = {
  schemaVersion: 1,
  ceilings: { minimumAssuranceLevel: none },
  secretReferences: {},
};

function failure(message: string): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure("TIBER_SETTINGS_INVALID_DOCUMENT", message),
  };
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: JSON primitives other than null safely yield undefined for every required property and are rejected by the document-shape check; typeof exists to establish the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAssuranceLevel(value: unknown): value is AssuranceLevel {
  return ASSURANCE_LEVELS.some((candidate) => candidate === value);
}

function parseSecretReference(value: unknown): SecretReference | undefined {
  if (!isRecord(value) || value.provider !== "environment") {
    return undefined;
  }
  const name = parseSecretEnvironmentVariableName(value.name);
  return name.ok ? { provider: "environment", name: name.value } : undefined;
}

export function parseAuthorityDocument(
  input: unknown,
): SettingsResult<AuthorityDocument> {
  if (
    !isRecord(input) ||
    input.schemaVersion !== 1 ||
    !isRecord(input.ceilings) ||
    !isRecord(input.secretReferences)
  ) {
    return failure("authority settings must use schema version 1");
  }
  const ceilingKeys = Object.keys(input.ceilings);
  if (
    ceilingKeys.some((key) => key !== "minimumAssuranceLevel") ||
    (input.ceilings.minimumAssuranceLevel !== undefined &&
      !isAssuranceLevel(input.ceilings.minimumAssuranceLevel))
  ) {
    return failure("minimum assurance ceiling is invalid");
  }

  const secretReferences: Record<SecretReferenceName, SecretReference> = {};
  for (const [key, value] of Object.entries(input.secretReferences)) {
    const parsedKey = parseSecretReferenceName(key);
    if (!parsedKey.ok) {
      return failure(`secret reference key is invalid: ${key}`);
    }
    const parsed = parseSecretReference(value);
    if (parsed === undefined) {
      return failure(`secret reference is invalid: ${key}`);
    }
    secretReferences[parsedKey.value] = parsed;
  }

  const minimumAssuranceLevel = input.ceilings.minimumAssuranceLevel;
  const ceilings: AuthorityDocument["ceilings"] = {
    minimumAssuranceLevel:
      minimumAssuranceLevel === undefined ? none : some(minimumAssuranceLevel),
  };
  return {
    ok: true,
    value: {
      schemaVersion: 1,
      ceilings,
      secretReferences,
    },
  };
}

export function applyAssuranceCeiling(
  requested: AssuranceLevel,
  minimum: Option<AssuranceLevel>,
): AssuranceDecision {
  // Stryker disable next-line ConditionalExpression, BlockStatement, StringLiteral: none has no value and therefore rank -1, so the general ranking branch returns this same value; the early return documents the Option rail.
  if (minimum.kind === "none") {
    return { requested, effective: requested, conflict: none };
  }
  const requestedRank = ASSURANCE_LEVELS.indexOf(requested);
  const minimumRank = ASSURANCE_LEVELS.indexOf(minimum.value);
  if (requestedRank >= minimumRank) {
    return { requested, effective: requested, conflict: none };
  }
  return {
    requested,
    effective: minimum.value,
    conflict: some(
      `project requested ${requested}, but the user-global ceiling requires ${minimum.value} or stronger`,
    ),
  };
}

export function lockMinimumAssurance(
  current: AuthorityDocument,
  level: AssuranceLevel,
): AuthorityDocument {
  return {
    ...current,
    ceilings: { minimumAssuranceLevel: some(level) },
  };
}

export function unlockMinimumAssurance(
  current: AuthorityDocument,
  confirmation: AuthorityUnlockConfirmation,
): SettingsResult<AuthorityDocument> {
  const locked = current.ceilings.minimumAssuranceLevel;
  if (locked.kind === "none") {
    return { ok: true, value: current };
  }
  const expected = `unlock minimumAssuranceLevel=${locked.value}`;
  if (confirmation !== expected) {
    return {
      ok: false,
      failure: settingsFailure(
        "TIBER_SETTINGS_INVALID_VALUE",
        `unlock requires exact confirmation: ${expected}`,
      ),
    };
  }
  return {
    ok: true,
    value: { ...current, ceilings: { minimumAssuranceLevel: none } },
  };
}

export function setSecretReference(
  current: AuthorityDocument,
  key: SecretReferenceName,
  environmentName: Option<SecretEnvironmentVariableName>,
): SettingsResult<AuthorityDocument> {
  if (environmentName.kind === "none") {
    const secretReferences = Object.fromEntries(
      Object.entries(current.secretReferences).filter(
        ([candidate]) => candidate !== key,
      ),
    );
    return { ok: true, value: { ...current, secretReferences } };
  }
  return {
    ok: true,
    value: {
      ...current,
      secretReferences: {
        ...current.secretReferences,
        [key]: {
          provider: "environment",
          name: environmentName.value,
        },
      },
    },
  };
}

export function formatAuthority(
  authority: AuthorityDocument,
  requested: AssuranceLevel,
): string {
  const decision = applyAssuranceCeiling(
    requested,
    authority.ceilings.minimumAssuranceLevel,
  );
  const references = Object.entries(authority.secretReferences)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, reference]) => `${key}=environment:${reference.name}`)
    .join(", ");
  return [
    `Minimum assurance lock: ${authority.ceilings.minimumAssuranceLevel.kind === "none" ? "unlocked" : authority.ceilings.minimumAssuranceLevel.value}`,
    `Assurance after ceiling: ${decision.effective}`,
    ...(decision.conflict.kind === "none"
      ? []
      : [`Conflict: ${decision.conflict.value}`]),
    `Secret references: ${references || "none"}`,
  ].join("\n");
}
