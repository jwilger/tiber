export type FailureRetryability =
  | "not-retryable"
  | "retry-after-input"
  | "retry-after-state-change"
  | "transient";

export type FailureRedaction = "public" | "sensitive" | "secret";

export interface FailureCause {
  readonly code: string;
  readonly safeSummary: string;
}

export interface TiberFailure<Code extends string, Context, RecoveryEvidence> {
  readonly code: Code;
  readonly message: string;
  readonly safeContext: Context;
  readonly causes: readonly FailureCause[];
  readonly retryability: FailureRetryability;
  readonly requiredRecoveryEvidence: readonly RecoveryEvidence[];
  readonly redaction: FailureRedaction;
}

export type Result<Value, Failure> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly failure: Failure };

export type TiberResult<
  Value,
  Failure extends TiberFailure<string, unknown, unknown>,
> = Result<Value, Failure>;

export function succeed<Value>(value: Value): Result<Value, never> {
  return { ok: true, value };
}

export function fail<Failure>(failure: Failure): Result<never, Failure> {
  return { ok: false, failure };
}

export function mapResult<Value, Failure, Mapped>(
  result: Result<Value, Failure>,
  map: (value: Value) => Mapped,
): Result<Mapped, Failure> {
  return result.ok ? succeed(map(result.value)) : result;
}

export function flatMapResult<Value, Failure, Mapped, NextFailure>(
  result: Result<Value, Failure>,
  map: (value: Value) => Result<Mapped, NextFailure>,
): Result<Mapped, Failure | NextFailure> {
  return result.ok ? map(result.value) : result;
}

export function semanticValueFailure<
  Code extends string,
  Field extends string,
  RecoveryEvidence extends string,
>(
  code: Code,
  field: Field,
  recoveryEvidence: RecoveryEvidence,
): TiberFailure<Code, { readonly field: Field }, RecoveryEvidence> {
  return {
    code,
    message: `Invalid ${field}`,
    safeContext: { field },
    causes: [],
    retryability: "retry-after-input",
    requiredRecoveryEvidence: [recoveryEvidence],
    redaction: "public",
  };
}

export function operationalFailure<Code extends string, Domain extends string>(
  code: Code,
  domain: Domain,
  message: string,
  retryability: FailureRetryability,
): TiberFailure<
  Code,
  { readonly domain: Domain },
  "corrected-input" | "state-change" | "retry-operation"
> {
  const requiredRecoveryEvidence =
    retryability === "retry-after-input"
      ? (["corrected-input"] as const)
      : retryability === "retry-after-state-change"
        ? (["state-change"] as const)
        : retryability === "transient"
          ? (["retry-operation"] as const)
          : [];
  return {
    code,
    message,
    safeContext: { domain },
    causes: [],
    retryability,
    requiredRecoveryEvidence,
    redaction: "public",
  };
}

export function mapFailure<Value, Failure, MappedFailure>(
  result: Result<Value, Failure>,
  map: (failure: Failure) => MappedFailure,
): Result<Value, MappedFailure> {
  return result.ok ? result : fail(map(result.failure));
}
