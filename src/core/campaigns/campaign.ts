import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

const campaignValueBrand: unique symbol = Symbol("CampaignValue");
type CampaignValue<Name extends string, Value> = Value & {
  readonly [campaignValueBrand]: Name;
};

export type CampaignTaskLimit = CampaignValue<"CampaignTaskLimit", number>;
export type CampaignInitiativeTaskLimit = CampaignValue<
  "CampaignInitiativeTaskLimit",
  number
>;
export type CampaignDurationLimit = CampaignValue<
  "CampaignDurationLimit",
  number
>;
export type CampaignCostLimit = CampaignValue<"CampaignCostLimit", number>;
export type CampaignTokenLimit = CampaignValue<"CampaignTokenLimit", number>;
export type CampaignConcurrencyLimit = CampaignValue<
  "CampaignConcurrencyLimit",
  number
>;
export type CampaignTaskId = CampaignValue<"CampaignTaskId", string>;
export type CampaignInitiativeId = CampaignValue<
  "CampaignInitiativeId",
  string
>;
export type CampaignGoal = CampaignValue<"CampaignGoal", string>;
export type CampaignId = CampaignValue<"CampaignId", string>;
export type CampaignCheckpointTime = CampaignValue<
  "CampaignCheckpointTime",
  string
>;

export interface CampaignBounds {
  readonly taskLimit: CampaignTaskLimit;
  readonly initiativeTaskLimit: CampaignInitiativeTaskLimit;
  readonly durationLimitMs: CampaignDurationLimit;
  readonly costLimitMicros: CampaignCostLimit;
  readonly tokenLimit: CampaignTokenLimit;
  readonly concurrencyLimit: CampaignConcurrencyLimit;
}

export interface CampaignConsumption {
  readonly startedTasks: number;
  readonly elapsedMs: number;
  readonly costMicros: number;
  readonly tokens: number;
  readonly activeTasks: number;
  readonly startedTaskIds: readonly CampaignTaskId[];
  readonly activeTaskIds: readonly CampaignTaskId[];
  readonly initiativeStarts: Readonly<Record<string, number>>;
}

export interface CampaignCandidate {
  readonly taskId: CampaignTaskId;
  readonly initiativeId: CampaignInitiativeId;
  readonly rank: number;
  readonly blockerPhase: "none" | "pre-mutation" | "post-mutation";
  readonly estimatedCostMicros: number;
  readonly estimatedTokens: number;
}

type CampaignFailure = TiberFailure<
  "TIBER_CAMPAIGN_VALUE_INVALID",
  { readonly field: "campaign" },
  "corrected-input"
>;
type CampaignResult<Value> = TiberResult<Value, CampaignFailure>;

function invalid(): CampaignResult<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_CAMPAIGN_VALUE_INVALID",
      "campaign",
      "corrected-input",
    ),
  };
}

function record(value: unknown): value is Record<string, unknown> {
  // Stryker disable next-line ConditionalExpression: null and array exclusions plus subsequent required-field validation already reject every non-object primitive; the typeof guard preserves trust-boundary narrowing.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function positiveInteger(value: unknown): value is number {
  // Stryker disable next-line ConditionalExpression: Number.isSafeInteger independently rejects every non-number; the typeof guard preserves explicit boundary intent.
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function nonnegativeInteger(value: unknown): value is number {
  // Stryker disable next-line ConditionalExpression: Number.isSafeInteger independently rejects every non-number; the typeof guard preserves explicit boundary intent.
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function activeTasksWereStarted(
  active: readonly unknown[],
  started: readonly unknown[],
): boolean {
  return active.every((taskId) => started.includes(taskId));
}

function identifier(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(value)
  );
}

export function parseCampaignBounds(
  value: unknown,
): CampaignResult<CampaignBounds> {
  if (
    !record(value) ||
    !positiveInteger(value.taskLimit) ||
    !positiveInteger(value.initiativeTaskLimit) ||
    !positiveInteger(value.durationLimitMs) ||
    !positiveInteger(value.costLimitMicros) ||
    !positiveInteger(value.tokenLimit) ||
    !positiveInteger(value.concurrencyLimit)
  )
    return invalid();
  return {
    ok: true,
    value: {
      taskLimit: value.taskLimit as CampaignTaskLimit,
      initiativeTaskLimit:
        value.initiativeTaskLimit as CampaignInitiativeTaskLimit,
      durationLimitMs: value.durationLimitMs as CampaignDurationLimit,
      costLimitMicros: value.costLimitMicros as CampaignCostLimit,
      tokenLimit: value.tokenLimit as CampaignTokenLimit,
      concurrencyLimit: value.concurrencyLimit as CampaignConcurrencyLimit,
    },
  };
}

export function parseCampaignConsumption(
  value: unknown,
): CampaignResult<CampaignConsumption> {
  if (
    !record(value) ||
    !nonnegativeInteger(value.startedTasks) ||
    !nonnegativeInteger(value.elapsedMs) ||
    !nonnegativeInteger(value.costMicros) ||
    !nonnegativeInteger(value.tokens) ||
    !nonnegativeInteger(value.activeTasks) ||
    !Array.isArray(value.startedTaskIds) ||
    !Array.isArray(value.activeTaskIds) ||
    !value.startedTaskIds.every(identifier) ||
    !value.activeTaskIds.every(identifier) ||
    new Set(value.startedTaskIds).size !== value.startedTaskIds.length ||
    new Set(value.activeTaskIds).size !== value.activeTaskIds.length ||
    value.startedTaskIds.length !== value.startedTasks ||
    value.activeTaskIds.length !== value.activeTasks ||
    !activeTasksWereStarted(value.activeTaskIds, value.startedTaskIds) ||
    !record(value.initiativeStarts) ||
    !Object.entries(value.initiativeStarts).every(
      ([key, count]) => identifier(key) && nonnegativeInteger(count),
    )
  )
    return invalid();
  return {
    ok: true,
    value: {
      startedTasks: value.startedTasks,
      elapsedMs: value.elapsedMs,
      costMicros: value.costMicros,
      tokens: value.tokens,
      activeTasks: value.activeTasks,
      startedTaskIds: value.startedTaskIds as CampaignTaskId[],
      activeTaskIds: value.activeTaskIds as CampaignTaskId[],
      initiativeStarts: value.initiativeStarts as Readonly<
        Record<string, number>
      >,
    },
  };
}

export function parseCampaignCandidate(
  value: unknown,
): CampaignResult<CampaignCandidate> {
  if (
    !record(value) ||
    !identifier(value.taskId) ||
    !identifier(value.initiativeId) ||
    !nonnegativeInteger(value.rank) ||
    (value.blockerPhase !== "none" &&
      value.blockerPhase !== "pre-mutation" &&
      value.blockerPhase !== "post-mutation") ||
    !nonnegativeInteger(value.estimatedCostMicros) ||
    !nonnegativeInteger(value.estimatedTokens)
  )
    return invalid();
  return {
    ok: true,
    value: {
      taskId: value.taskId as CampaignTaskId,
      initiativeId: value.initiativeId as CampaignInitiativeId,
      rank: value.rank,
      blockerPhase: value.blockerPhase,
      estimatedCostMicros: value.estimatedCostMicros,
      estimatedTokens: value.estimatedTokens,
    },
  };
}

export function parseCampaignTaskId(
  value: unknown,
): CampaignResult<CampaignTaskId> {
  return identifier(value)
    ? { ok: true, value: value as CampaignTaskId }
    : invalid();
}

export function parseCampaignId(value: unknown): CampaignResult<CampaignId> {
  return identifier(value)
    ? { ok: true, value: value as CampaignId }
    : invalid();
}

export function parseCampaignCheckpointTime(
  value: unknown,
): CampaignResult<CampaignCheckpointTime> {
  if (typeof value !== "string" || Number.isNaN(Date.parse(value)))
    return invalid();
  return { ok: true, value: value as CampaignCheckpointTime };
}

export function parseCampaignGoal(
  value: unknown,
): CampaignResult<CampaignGoal> {
  if (
    typeof value !== "string" ||
    value.trim().length === 0 ||
    value.length > 200
  )
    return invalid();
  return { ok: true, value: value.trim() as CampaignGoal };
}

export type CampaignEffect =
  | {
      readonly kind: "start-task";
      readonly taskId: CampaignTaskId;
      readonly initiativeId: CampaignInitiativeId;
    }
  | {
      readonly kind: "release-and-defer";
      readonly taskId: CampaignTaskId;
      readonly initiativeId: CampaignInitiativeId;
    }
  | {
      readonly kind: "retain-blocked-work";
      readonly taskId: CampaignTaskId;
      readonly initiativeId: CampaignInitiativeId;
    };

export interface CampaignAttentionItem {
  readonly taskId: CampaignTaskId;
  readonly initiativeId: CampaignInitiativeId;
  readonly kind: "pre-mutation-blocker" | "post-mutation-blocker";
}

export type CampaignCheckpointReason =
  | "work-scheduled"
  | "task-bound"
  | "time-bound"
  | "cost-bound"
  | "token-bound"
  | "concurrency-bound"
  | "no-eligible-work";

export function mergeCampaignAttention(
  retained: readonly CampaignAttentionItem[],
  observed: readonly CampaignAttentionItem[],
): readonly CampaignAttentionItem[] {
  const byIdentity = new Map<string, CampaignAttentionItem>();
  for (const item of [...retained, ...observed])
    byIdentity.set(`${item.kind}:${item.taskId}`, item);
  return [...byIdentity.values()].sort(
    (left, right) =>
      left.taskId.localeCompare(right.taskId) ||
      left.kind.localeCompare(right.kind),
  );
}

export interface CampaignDecision {
  readonly effects: readonly CampaignEffect[];
  readonly attention: readonly CampaignAttentionItem[];
  readonly checkpoint: {
    readonly reason: CampaignCheckpointReason;
    readonly consumption: CampaignConsumption;
  };
}

function boundReason(
  bounds: CampaignBounds,
  consumption: CampaignConsumption,
): CampaignCheckpointReason | undefined {
  if (consumption.startedTasks >= bounds.taskLimit) return "task-bound";
  if (consumption.elapsedMs >= bounds.durationLimitMs) return "time-bound";
  if (consumption.costMicros >= bounds.costLimitMicros) return "cost-bound";
  if (consumption.tokens >= bounds.tokenLimit) return "token-bound";
  if (consumption.activeTasks >= bounds.concurrencyLimit)
    return "concurrency-bound";
  return undefined;
}

export function decideCampaignSchedule(input: {
  readonly bounds: CampaignBounds;
  readonly consumption: CampaignConsumption;
  readonly candidates: readonly CampaignCandidate[];
}): CampaignDecision {
  let consumption = input.consumption;
  const effects: CampaignEffect[] = [];
  const attention: CampaignAttentionItem[] = [];
  let reason = boundReason(input.bounds, consumption);

  const candidates = [...input.candidates].sort(
    (left, right) =>
      left.rank - right.rank || left.taskId.localeCompare(right.taskId),
  );
  for (const candidate of candidates) {
    if (candidate.blockerPhase === "pre-mutation") {
      effects.push({
        kind: "release-and-defer",
        taskId: candidate.taskId,
        initiativeId: candidate.initiativeId,
      });
      attention.push({
        taskId: candidate.taskId,
        initiativeId: candidate.initiativeId,
        kind: "pre-mutation-blocker",
      });
      if (consumption.activeTaskIds.includes(candidate.taskId))
        consumption = {
          ...consumption,
          activeTasks: consumption.activeTasks - 1,
          activeTaskIds: consumption.activeTaskIds.filter(
            (taskId) => taskId !== candidate.taskId,
          ),
        };
      reason = boundReason(input.bounds, consumption);
      continue;
    }
    if (candidate.blockerPhase === "post-mutation") {
      effects.push({
        kind: "retain-blocked-work",
        taskId: candidate.taskId,
        initiativeId: candidate.initiativeId,
      });
      attention.push({
        taskId: candidate.taskId,
        initiativeId: candidate.initiativeId,
        kind: "post-mutation-blocker",
      });
      continue;
    }
    if (
      reason !== undefined ||
      consumption.startedTaskIds.includes(candidate.taskId)
    )
      continue;
    const initiativeCount =
      consumption.initiativeStarts[candidate.initiativeId] ?? 0;
    if (initiativeCount >= input.bounds.initiativeTaskLimit) continue;
    if (
      consumption.costMicros + candidate.estimatedCostMicros >
      input.bounds.costLimitMicros
    ) {
      reason = "cost-bound";
      continue;
    }
    if (
      consumption.tokens + candidate.estimatedTokens >
      input.bounds.tokenLimit
    ) {
      reason = "token-bound";
      continue;
    }
    effects.push({
      kind: "start-task",
      taskId: candidate.taskId,
      initiativeId: candidate.initiativeId,
    });
    consumption = {
      ...consumption,
      startedTasks: consumption.startedTasks + 1,
      activeTasks: consumption.activeTasks + 1,
      startedTaskIds: [...consumption.startedTaskIds, candidate.taskId],
      activeTaskIds: [...consumption.activeTaskIds, candidate.taskId],
      costMicros: consumption.costMicros + candidate.estimatedCostMicros,
      tokens: consumption.tokens + candidate.estimatedTokens,
      initiativeStarts: {
        ...consumption.initiativeStarts,
        [candidate.initiativeId]: initiativeCount + 1,
      },
    };
    reason = boundReason(input.bounds, consumption);
    // Later blocker observations remain eligible even after a start bound.
  }
  return {
    effects,
    attention,
    checkpoint: {
      reason:
        reason ??
        (effects.some((effect) => effect.kind === "start-task")
          ? "work-scheduled"
          : "no-eligible-work"),
      consumption,
    },
  };
}

export function createAdHocCampaignTask(
  goal: CampaignGoal,
  campaignId: CampaignId,
) {
  return {
    title: goal,
    description: `Ad-hoc campaign goal: ${goal}`,
    provenance: { kind: "campaign-goal" as const, campaignId },
    initialState: "Backlog" as const,
  };
}
