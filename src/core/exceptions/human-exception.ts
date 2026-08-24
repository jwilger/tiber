import {
  semanticValueFailure,
  type Result,
  type TiberFailure,
  succeed,
  fail,
} from "../failures/tiber-failure.js";

type ExceptionFailure = TiberFailure<string, unknown, unknown>;

export interface FrozenStructuredCommand {
  readonly kind: "structured-command";
  readonly executable: string;
  readonly arguments: readonly string[];
  readonly environment: readonly {
    readonly name: string;
    readonly value: string;
  }[];
  readonly workingDirectory: string;
  readonly timeoutMs: number;
  readonly maxOutputBytes: number;
  readonly paths: readonly string[];
  readonly preimages: readonly {
    readonly path: string;
    readonly digest: string;
  }[];
}

export interface ExceptionBlockerClaim {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly runId: string;
  readonly revision: string;
  readonly goal: string;
  readonly denialCode: string;
  readonly compliantAlternatives: readonly string[];
  readonly operation: FrozenStructuredCommand;
  readonly stateDigest: string;
}

export interface ExceptionNecessityReview {
  readonly disposition: "necessary" | "compliant-route-available";
  readonly rationale: string;
  readonly reviewerIdentity: string;
}

export interface ExceptionAttention {
  readonly attentionId: string;
  readonly claimDigest: string;
  readonly taskId: string;
  readonly runId: string;
  readonly goal: string;
  readonly denialCode: string;
  readonly rationale: string;
}

export interface HumanExceptionApproval {
  readonly attentionId: string;
  readonly approvedAt: string;
  readonly expiresAt: string;
  readonly humanIdentity: string;
}

export interface ExceptionExecutionAttempt {
  readonly attemptId: string;
  readonly claim: ExceptionBlockerClaim;
}

declare const exceptionExecutionTimeBrand: unique symbol;
export type ExceptionExecutionTime = string & {
  readonly [exceptionExecutionTimeBrand]: "ExceptionExecutionTime";
};

export interface ExceptionExecutionObservation {
  readonly attemptId: string;
  readonly exitCode: number;
  readonly stdoutDigest: string;
  readonly stderrDigest: string;
  readonly observedAt: string;
}

const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const nonempty = (value: unknown): value is string => {
  if (typeof value !== "string") return false;
  return value.length > 0;
};
const digest = (value: unknown): value is string => {
  if (typeof value !== "string") return false;
  return /^[a-f0-9]{64}$/u.test(value);
};
const revision = (value: unknown): value is string => {
  if (typeof value !== "string") return false;
  return /^[a-f0-9]{40}$/u.test(value);
};
const iso = (value: unknown): value is string => {
  // Stryker disable next-line ConditionalExpression: a non-string cannot strictly equal the canonical ISO string below.
  if (typeof value !== "string") return false;
  const milliseconds = Date.parse(value);
  return (
    Number.isFinite(milliseconds) &&
    new Date(milliseconds).toISOString() === value
  );
};
const strings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === "string");

function invalid(context: string): ReturnType<typeof semanticValueFailure> {
  return semanticValueFailure(
    "TIBER_EXCEPTION_VALUE_INVALID",
    "human exception values are invalid",
    context,
  );
}

function parseOperation(value: unknown): FrozenStructuredCommand | undefined {
  if (
    !record(value) ||
    value.kind !== "structured-command" ||
    !nonempty(value.executable) ||
    !value.executable.startsWith("/") ||
    !strings(value.arguments) ||
    !nonempty(value.workingDirectory) ||
    !value.workingDirectory.startsWith("/") ||
    !Number.isSafeInteger(value.timeoutMs) ||
    (value.timeoutMs as number) < 1 ||
    (value.timeoutMs as number) > 60_000 ||
    !Number.isSafeInteger(value.maxOutputBytes) ||
    (value.maxOutputBytes as number) < 1 ||
    (value.maxOutputBytes as number) > 1_048_576 ||
    !strings(value.paths) ||
    !Array.isArray(value.environment) ||
    !Array.isArray(value.preimages)
  )
    return undefined;
  const environment: { name: string; value: string }[] = [];
  for (const item of value.environment) {
    if (!record(item) || !nonempty(item.name) || typeof item.value !== "string")
      return undefined;
    environment.push({ name: item.name, value: item.value });
  }
  const preimages: { path: string; digest: string }[] = [];
  for (const item of value.preimages) {
    if (!record(item) || !nonempty(item.path) || !digest(item.digest))
      return undefined;
    preimages.push({ path: item.path, digest: item.digest });
  }
  return {
    kind: "structured-command",
    executable: value.executable,
    arguments: value.arguments,
    environment,
    workingDirectory: value.workingDirectory,
    timeoutMs: value.timeoutMs as number,
    maxOutputBytes: value.maxOutputBytes as number,
    paths: value.paths,
    preimages,
  };
}

export function parseExceptionBlockerClaim(
  value: unknown,
): Result<ExceptionBlockerClaim, ExceptionFailure> {
  if (!record(value)) return fail(invalid("exception blocker claim"));
  const operation = parseOperation(value.operation);
  if (
    value.schemaVersion !== 1 ||
    !nonempty(value.taskId) ||
    !nonempty(value.runId) ||
    !revision(value.revision) ||
    !nonempty(value.goal) ||
    !nonempty(value.denialCode) ||
    !strings(value.compliantAlternatives) ||
    operation === undefined ||
    !digest(value.stateDigest)
  )
    return fail(invalid("exception blocker claim"));
  return succeed({
    schemaVersion: 1,
    taskId: value.taskId,
    runId: value.runId,
    revision: value.revision,
    goal: value.goal,
    denialCode: value.denialCode,
    compliantAlternatives: value.compliantAlternatives,
    operation,
    stateDigest: value.stateDigest,
  });
}

export function parseHumanExceptionApproval(
  value: unknown,
): Result<HumanExceptionApproval, ExceptionFailure> {
  if (
    !record(value) ||
    !nonempty(value.attentionId) ||
    !iso(value.approvedAt) ||
    !iso(value.expiresAt) ||
    !nonempty(value.humanIdentity)
  )
    return fail(invalid("human exception approval"));
  const lifetime = Date.parse(value.expiresAt) - Date.parse(value.approvedAt);
  if (lifetime <= 0 || lifetime > 15 * 60 * 1000)
    return fail(invalid("human exception approval lifetime"));
  return succeed({
    attentionId: value.attentionId,
    approvedAt: value.approvedAt,
    expiresAt: value.expiresAt,
    humanIdentity: value.humanIdentity,
  });
}

export function parseExceptionExecutionTime(
  value: unknown,
): Result<ExceptionExecutionTime, ExceptionFailure> {
  return iso(value)
    ? succeed(value as ExceptionExecutionTime)
    : fail(invalid("exception execution time"));
}

export function parseExceptionExecutionObservation(
  value: unknown,
): Result<ExceptionExecutionObservation, ExceptionFailure> {
  if (
    !record(value) ||
    !nonempty(value.attemptId) ||
    !Number.isSafeInteger(value.exitCode) ||
    !digest(value.stdoutDigest) ||
    !digest(value.stderrDigest) ||
    !iso(value.observedAt)
  )
    return fail(invalid("exception execution observation"));
  return succeed({
    attemptId: value.attemptId,
    exitCode: value.exitCode as number,
    stdoutDigest: value.stdoutDigest,
    stderrDigest: value.stderrDigest,
    observedAt: value.observedAt,
  });
}
