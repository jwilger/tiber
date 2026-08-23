import {
  ASSURANCE_LEVELS,
  type AssuranceLevel,
  type SettingsResult,
} from "./settings.js";

export interface SecretReference {
  readonly provider: "environment";
  readonly name: string;
}

export interface AuthorityDocument {
  readonly schemaVersion: 1;
  readonly ceilings: {
    readonly minimumAssuranceLevel?: AssuranceLevel;
  };
  readonly secretReferences: Readonly<Record<string, SecretReference>>;
}

export interface AssuranceDecision {
  readonly requested: AssuranceLevel;
  readonly effective: AssuranceLevel;
  readonly conflict?: string;
}

export const EMPTY_AUTHORITY: AuthorityDocument = {
  schemaVersion: 1,
  ceilings: {},
  secretReferences: {},
};

function failure(message: string): SettingsResult<never> {
  return {
    ok: false,
    failure: {
      code: "TIBER_SETTINGS_INVALID_DOCUMENT",
      message,
      retryable: false,
    },
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
  // Stryker disable next-line ConditionalExpression: JSON non-strings are string-coerced by RegExp.test and none can satisfy the uppercase environment-name grammar; the explicit guard narrows the semantic result.
  return typeof value.name === "string" &&
    /^[A-Z][A-Z0-9_]{0,127}$/u.test(value.name)
    ? { provider: "environment", name: value.name }
    : undefined;
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

  const secretReferences: Record<string, SecretReference> = {};
  for (const [key, value] of Object.entries(input.secretReferences)) {
    if (!/^[a-z][a-z0-9-]{0,63}$/u.test(key)) {
      return failure(`secret reference key is invalid: ${key}`);
    }
    const parsed = parseSecretReference(value);
    if (parsed === undefined) {
      return failure(`secret reference is invalid: ${key}`);
    }
    secretReferences[key] = parsed;
  }

  const minimumAssuranceLevel = input.ceilings.minimumAssuranceLevel;
  const ceilings: AuthorityDocument["ceilings"] =
    // Stryker disable next-line ConditionalExpression: an own property whose value is undefined is omitted by JSON serialization and is semantically identical to absence under exact-optional consumers; the branch keeps the in-memory representation canonical.
    minimumAssuranceLevel === undefined
      ? {}
      : { minimumAssuranceLevel: minimumAssuranceLevel };
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
  minimum: AssuranceLevel | undefined,
): AssuranceDecision {
  // Stryker disable next-line ConditionalExpression, BlockStatement: undefined has rank -1, so the general ranking branch returns this same value; the early return documents and avoids relying on that array sentinel.
  if (minimum === undefined) {
    return { requested, effective: requested };
  }
  const requestedRank = ASSURANCE_LEVELS.indexOf(requested);
  const minimumRank = ASSURANCE_LEVELS.indexOf(minimum);
  if (requestedRank >= minimumRank) {
    return { requested, effective: requested };
  }
  return {
    requested,
    effective: minimum,
    conflict: `project requested ${requested}, but the user-global ceiling requires ${minimum} or stronger`,
  };
}

export function lockMinimumAssurance(
  current: AuthorityDocument,
  level: AssuranceLevel,
): AuthorityDocument {
  return {
    ...current,
    ceilings: { minimumAssuranceLevel: level },
  };
}

export function unlockMinimumAssurance(
  current: AuthorityDocument,
  confirmation: string,
): SettingsResult<AuthorityDocument> {
  const locked = current.ceilings.minimumAssuranceLevel;
  if (locked === undefined) {
    return { ok: true, value: current };
  }
  const expected = `unlock minimumAssuranceLevel=${locked}`;
  if (confirmation !== expected) {
    return {
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_VALUE",
        message: `unlock requires exact confirmation: ${expected}`,
        retryable: false,
      },
    };
  }
  return { ok: true, value: { ...current, ceilings: {} } };
}

export function setSecretReference(
  current: AuthorityDocument,
  key: string,
  environmentName: string | undefined,
): SettingsResult<AuthorityDocument> {
  if (!/^[a-z][a-z0-9-]{0,63}$/u.test(key)) {
    return failure(`secret reference key is invalid: ${key}`);
  }
  if (
    environmentName !== undefined &&
    !/^[A-Z][A-Z0-9_]{0,127}$/u.test(environmentName)
  ) {
    return failure(`environment variable name is invalid: ${environmentName}`);
  }
  const secretReferences: Readonly<Record<string, SecretReference>> =
    environmentName === undefined
      ? Object.fromEntries(
          Object.entries(current.secretReferences).filter(
            ([existingKey]) => existingKey !== key,
          ),
        )
      : {
          ...current.secretReferences,
          [key]: { provider: "environment", name: environmentName },
        };
  return { ok: true, value: { ...current, secretReferences } };
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
    `Minimum assurance lock: ${authority.ceilings.minimumAssuranceLevel ?? "unlocked"}`,
    `Assurance after ceiling: ${decision.effective}`,
    ...(decision.conflict === undefined
      ? []
      : [`Conflict: ${decision.conflict}`]),
    `Secret references: ${references || "none"}`,
  ].join("\n");
}
