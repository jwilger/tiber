import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import {
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignCheckpointTime,
  parseCampaignConsumption,
  parseCampaignId,
  type CampaignAttentionItem,
  type CampaignBounds,
  type CampaignCandidate,
  type CampaignCheckpointTime,
  type CampaignConsumption,
  type CampaignId,
  type CampaignDecision,
} from "../../core/campaigns/campaign.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";

export interface CampaignCheckpoint {
  readonly schemaVersion: 1;
  readonly campaignId: CampaignId;
  readonly repositoryPath: string;
  readonly status: "active" | "completed" | "shutdown";
  readonly startedAt: CampaignCheckpointTime;
  readonly updatedAt: CampaignCheckpointTime;
  readonly bounds: CampaignBounds;
  readonly consumption: CampaignConsumption;
  readonly candidates: readonly CampaignCandidate[];
  readonly attention: readonly CampaignAttentionItem[];
  readonly reason:
    CampaignDecision["checkpoint"]["reason"] | "session-shutdown";
}

type StoreFailure = TiberFailure<
  "TIBER_CAMPAIGN_CHECKPOINT_INVALID" | "TIBER_CAMPAIGN_CHECKPOINT_IO",
  { readonly domain: "campaign-checkpoint" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type StoreResult<Value> = TiberResult<Value, StoreFailure>;

function failure(code: StoreFailure["code"]): StoreResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "campaign-checkpoint",
      code === "TIBER_CAMPAIGN_CHECKPOINT_IO"
        ? "Campaign checkpoint I/O failed"
        : "Campaign checkpoint is invalid",
      code === "TIBER_CAMPAIGN_CHECKPOINT_IO"
        ? "transient"
        : "retry-after-input",
    ),
  };
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class FileCampaignStore {
  private readonly path: string;

  public constructor(
    agentDirectory: string,
    private readonly repositoryPath: string,
  ) {
    const identity = createHash("sha256").update(repositoryPath).digest("hex");
    this.path = join(agentDirectory, "tiber", "campaigns", `${identity}.json`);
  }

  public read(): StoreResult<Option<CampaignCheckpoint>> {
    let value: unknown;
    try {
      value = JSON.parse(readFileSync(this.path, "utf8"));
    } catch (error) {
      if (object(error) && error.code === "ENOENT")
        return { ok: true, value: none };
      return failure("TIBER_CAMPAIGN_CHECKPOINT_IO");
    }
    if (
      !object(value) ||
      value.schemaVersion !== 1 ||
      typeof value.campaignId !== "string" ||
      value.repositoryPath !== this.repositoryPath ||
      (value.status !== "active" &&
        value.status !== "completed" &&
        value.status !== "shutdown") ||
      typeof value.startedAt !== "string" ||
      typeof value.updatedAt !== "string" ||
      !Array.isArray(value.candidates) ||
      !Array.isArray(value.attention) ||
      typeof value.reason !== "string"
    )
      return failure("TIBER_CAMPAIGN_CHECKPOINT_INVALID");
    const campaignId = parseCampaignId(value.campaignId);
    const startedAt = parseCampaignCheckpointTime(value.startedAt);
    const updatedAt = parseCampaignCheckpointTime(value.updatedAt);
    const bounds = parseCampaignBounds(value.bounds);
    const consumption = parseCampaignConsumption(value.consumption);
    const candidates = value.candidates.map(parseCampaignCandidate);
    const parsedCandidates = candidates.flatMap((candidate) =>
      candidate.ok ? [candidate.value] : [],
    );
    const attention: CampaignAttentionItem[] = [];
    for (const item of value.attention) {
      if (!object(item)) continue;
      const candidate = parsedCandidates.find(
        (entry) =>
          entry.taskId === item.taskId &&
          entry.initiativeId === item.initiativeId,
      );
      if (candidate === undefined) continue;
      if (item.kind === "pre-mutation-blocker")
        attention.push({
          taskId: candidate.taskId,
          initiativeId: candidate.initiativeId,
          kind: "pre-mutation-blocker",
        });
      if (item.kind === "post-mutation-blocker")
        attention.push({
          taskId: candidate.taskId,
          initiativeId: candidate.initiativeId,
          kind: "post-mutation-blocker",
        });
    }
    const reasons: readonly CampaignCheckpoint["reason"][] = [
      "work-scheduled",
      "task-bound",
      "time-bound",
      "cost-bound",
      "token-bound",
      "concurrency-bound",
      "no-eligible-work",
      "session-shutdown",
    ];
    const reason = reasons.find((candidate) => candidate === value.reason);
    if (
      !campaignId.ok ||
      !startedAt.ok ||
      !updatedAt.ok ||
      !bounds.ok ||
      !consumption.ok ||
      parsedCandidates.length !== candidates.length ||
      attention.length !== value.attention.length ||
      reason === undefined
    )
      return failure("TIBER_CAMPAIGN_CHECKPOINT_INVALID");
    return {
      ok: true,
      value: some({
        schemaVersion: 1,
        campaignId: campaignId.value,
        repositoryPath: this.repositoryPath,
        status: value.status,
        startedAt: startedAt.value,
        updatedAt: updatedAt.value,
        bounds: bounds.value,
        consumption: consumption.value,
        candidates: parsedCandidates,
        attention,
        reason,
      }),
    };
  }

  public write(
    checkpoint: CampaignCheckpoint,
  ): StoreResult<CampaignCheckpoint> {
    try {
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      const temporary = `${this.path}.tmp`;
      writeFileSync(
        temporary,
        `${JSON.stringify(checkpoint, undefined, 2)}\n`,
        { mode: 0o600 },
      );
      renameSync(temporary, this.path);
      return { ok: true, value: checkpoint };
    } catch {
      return failure("TIBER_CAMPAIGN_CHECKPOINT_IO");
    }
  }

  public shutdown(
    now: CampaignCheckpointTime,
  ): StoreResult<Option<CampaignCheckpoint>> {
    const current = this.read();
    if (!current.ok || current.value.kind === "none") return current;
    if (current.value.value.status === "shutdown") return current;
    const checkpoint: CampaignCheckpoint = {
      ...current.value.value,
      status: "shutdown",
      updatedAt: now,
      reason: "session-shutdown",
    };
    const written = this.write(checkpoint);
    return written.ok ? { ok: true, value: some(written.value) } : written;
  }
}
