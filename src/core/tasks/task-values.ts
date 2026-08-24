import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const taskValuePurpose: unique symbol;
type TaskValue<Value, Purpose extends string> = Value & {
  readonly [taskValuePurpose]: Purpose;
};

export type ClaimPublicationStatus = "absent" | "published";
export type TaskId = TaskValue<string, "task-id">;
export type TaskEventId = TaskValue<string, "task-event-id">;
export type TaskClaimId = TaskValue<string, "task-claim-id">;
export type TaskEventOccurredAt = TaskValue<string, "task-event-occurred-at">;
export type TaskTitle = TaskValue<string, "task-title">;
export type TaskDescription = TaskValue<string, "task-description">;
export type ClaimOwnerIdentity = TaskValue<string, "claim-owner-identity">;
export type ClaimBaselineRevision = TaskValue<
  string,
  "claim-baseline-revision"
>;
export type SpecificationDigest = TaskValue<string, "specification-digest">;
export type ScenarioName = TaskValue<string, "scenario-name">;
export type ScenarioGivenStep = TaskValue<string, "scenario-given-step">;
export type ScenarioWhenStep = TaskValue<string, "scenario-when-step">;
export type ScenarioThenStep = TaskValue<string, "scenario-then-step">;
export type TestMappingPath = TaskValue<string, "test-mapping-path">;
export type SpecificationOutcome = TaskValue<string, "specification-outcome">;
export type AcceptanceCriterion = TaskValue<string, "acceptance-criterion">;
export type SpecificationExclusion = TaskValue<
  string,
  "specification-exclusion"
>;
export type SpecificationDependency = TaskValue<
  string,
  "specification-dependency"
>;
export type ArchitectureImplication = TaskValue<
  string,
  "architecture-implication"
>;
export type SpecificationReviewFindingCount = TaskValue<
  number,
  "specification-review-finding-count"
>;

type TaskValueField =
  | "taskId"
  | "taskEventId"
  | "taskClaimId"
  | "taskEventOccurredAt"
  | "taskTitle"
  | "taskDescription"
  | "claimOwnerIdentity"
  | "claimBaselineRevision"
  | "specificationDigest"
  | "scenarioName"
  | "scenarioGivenStep"
  | "scenarioWhenStep"
  | "scenarioThenStep"
  | "testMappingPath"
  | "specificationOutcome"
  | "acceptanceCriterion"
  | "specificationExclusion"
  | "specificationDependency"
  | "architectureImplication"
  | "specificationReviewFindingCount";

type TaskValueFailure = TiberFailure<
  "TIBER_TASK_VALUE_INVALID",
  { readonly field: TaskValueField },
  "corrected-value"
>;

type Result<Value> = TiberResult<Value, TaskValueFailure>;

function invalid(field: TaskValueField): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_TASK_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function valid<Value, Purpose extends string>(
  value: Value,
): TaskValue<Value, Purpose> {
  return value as TaskValue<Value, Purpose>;
}

function uuid<Purpose extends string>(
  value: unknown,
  field: TaskValueField,
): Result<TaskValue<string, Purpose>> {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
      value,
    )
    ? { ok: true, value: valid<string, Purpose>(value) }
    : invalid(field);
}

export const parseTaskId = (value: unknown): Result<TaskId> =>
  uuid(value, "taskId");
export const parseTaskEventId = (value: unknown): Result<TaskEventId> =>
  uuid(value, "taskEventId");
export const parseTaskClaimId = (value: unknown): Result<TaskClaimId> =>
  uuid(value, "taskClaimId");

export function parseTaskEventOccurredAt(
  value: unknown,
): Result<TaskEventOccurredAt> {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return invalid("taskEventOccurredAt");
  return new Date(value).toISOString() === value
    ? { ok: true, value: valid<string, "task-event-occurred-at">(value) }
    : invalid("taskEventOccurredAt");
}

function boundedText<Purpose extends string>(
  value: unknown,
  field: TaskValueField,
  maximum: number,
): Result<TaskValue<string, Purpose>> {
  return typeof value === "string" &&
    value.trim() === value &&
    value.length > 0 &&
    value.length <= maximum
    ? { ok: true, value: valid<string, Purpose>(value) }
    : invalid(field);
}

export const parseTaskTitle = (value: unknown): Result<TaskTitle> =>
  boundedText(value, "taskTitle", 200);
export function parseTaskDescription(value: unknown): Result<TaskDescription> {
  return typeof value === "string" && value.length <= 10_000
    ? { ok: true, value: value as TaskDescription }
    : invalid("taskDescription");
}
export const parseClaimOwnerIdentity = (
  value: unknown,
): Result<ClaimOwnerIdentity> => boundedText(value, "claimOwnerIdentity", 320);
export const parseScenarioName = (value: unknown): Result<ScenarioName> =>
  boundedText(value, "scenarioName", 200);
export const parseScenarioGivenStep = (
  value: unknown,
): Result<ScenarioGivenStep> => boundedText(value, "scenarioGivenStep", 1_000);
export const parseScenarioWhenStep = (
  value: unknown,
): Result<ScenarioWhenStep> => boundedText(value, "scenarioWhenStep", 1_000);
export const parseScenarioThenStep = (
  value: unknown,
): Result<ScenarioThenStep> => boundedText(value, "scenarioThenStep", 1_000);
export const parseSpecificationOutcome = (
  value: unknown,
): Result<SpecificationOutcome> =>
  boundedText(value, "specificationOutcome", 2_000);
export const parseAcceptanceCriterion = (
  value: unknown,
): Result<AcceptanceCriterion> =>
  boundedText(value, "acceptanceCriterion", 2_000);
export const parseSpecificationExclusion = (
  value: unknown,
): Result<SpecificationExclusion> =>
  boundedText(value, "specificationExclusion", 2_000);
export const parseSpecificationDependency = (
  value: unknown,
): Result<SpecificationDependency> =>
  boundedText(value, "specificationDependency", 2_000);
export const parseArchitectureImplication = (
  value: unknown,
): Result<ArchitectureImplication> =>
  boundedText(value, "architectureImplication", 4_000);

export function parseClaimBaselineRevision(
  value: unknown,
): Result<ClaimBaselineRevision> {
  return typeof value === "string" && /^[0-9a-f]{40,64}$/u.test(value)
    ? {
        ok: true,
        value: valid<string, "claim-baseline-revision">(value),
      }
    : invalid("claimBaselineRevision");
}

function digest<Purpose extends string>(
  value: unknown,
  field: TaskValueField,
): Result<TaskValue<string, Purpose>> {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/u.test(value)
    ? { ok: true, value: valid<string, Purpose>(value) }
    : invalid(field);
}

export const parseSpecificationDigest = (
  value: unknown,
): Result<SpecificationDigest> => digest(value, "specificationDigest");

export function parseTestMappingPath(value: unknown): Result<TestMappingPath> {
  return typeof value === "string" &&
    value.length > 0 &&
    value.length <= 500 &&
    !value.includes("\\") &&
    !value.startsWith("/") &&
    !value.startsWith("../") &&
    value !== ".." &&
    !value.includes("/../") &&
    !value.includes("//")
    ? { ok: true, value: valid<string, "test-mapping-path">(value) }
    : invalid("testMappingPath");
}

export function parseSpecificationReviewFindingCount(
  value: unknown,
): Result<SpecificationReviewFindingCount> {
  // Stryker disable next-line ConditionalExpression: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? {
        ok: true,
        value: valid<number, "specification-review-finding-count">(value),
      }
    : invalid("specificationReviewFindingCount");
}
