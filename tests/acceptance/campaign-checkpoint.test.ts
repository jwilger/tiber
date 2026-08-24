import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCampaignStore } from "../../src/adapters/campaigns/file-campaign-store.js";
import {
  decideCampaignSchedule,
  parseCampaignBounds,
  parseCampaignCandidate,
  parseCampaignCheckpointTime,
  parseCampaignConsumption,
  parseCampaignId,
} from "../../src/core/campaigns/campaign.js";

const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0))
    rmSync(root, { recursive: true, force: true });
});
function parsed<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid fixture");
  return result.value;
}

describe("durable autonomous campaign checkpoints", () => {
  it("persists every scheduling decision and records a restart-safe shutdown boundary", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-campaign-"));
    roots.push(root);
    const repository = join(root, "repository");
    const store = new FileCampaignStore(join(root, "agent"), repository);
    const bounds = parsed(
      parseCampaignBounds({
        taskLimit: 1,
        initiativeTaskLimit: 1,
        durationLimitMs: 10_000,
        costLimitMicros: 1_000,
        tokenLimit: 1_000,
        concurrencyLimit: 1,
      }),
    );
    const consumption = parsed(
      parseCampaignConsumption({
        startedTasks: 0,
        elapsedMs: 0,
        costMicros: 0,
        tokens: 0,
        activeTasks: 0,
        startedTaskIds: [],
        activeTaskIds: [],
        initiativeStarts: {},
      }),
    );
    const candidates = [
      parsed(
        parseCampaignCandidate({
          taskId: "task-1",
          initiativeId: "initiative-1",
          rank: 1,
          blockerPhase: "none",
          estimatedCostMicros: 100,
          estimatedTokens: 100,
        }),
      ),
    ];
    const decision = decideCampaignSchedule({
      bounds,
      consumption,
      candidates,
    });
    const written = store.write({
      schemaVersion: 1,
      campaignId: parsed(parseCampaignId("campaign-1")),
      repositoryPath: repository,
      status: "active",
      startedAt: parsed(
        parseCampaignCheckpointTime("2026-01-01T00:00:00.000Z"),
      ),
      updatedAt: parsed(
        parseCampaignCheckpointTime("2026-01-01T00:00:01.000Z"),
      ),
      bounds,
      consumption: decision.checkpoint.consumption,
      candidates,
      attention: decision.attention,
      reason: decision.checkpoint.reason,
    });
    expect(written.ok).toBe(true);
    const shutdown = store.shutdown(
      parsed(parseCampaignCheckpointTime("2026-01-01T00:00:02.000Z")),
    );
    expect(shutdown).toMatchObject({
      ok: true,
      value: {
        kind: "some",
        value: { status: "shutdown", reason: "session-shutdown" },
      },
    });
    const reread = new FileCampaignStore(
      join(root, "agent"),
      repository,
    ).read();
    expect(reread).toMatchObject({
      ok: true,
      value: {
        kind: "some",
        value: {
          consumption: { startedTasks: 1, activeTasks: 1 },
          status: "shutdown",
        },
      },
    });
    const identity = createHash("sha256").update(repository).digest("hex");
    expect(
      readFileSync(
        join(root, "agent", "tiber", "campaigns", `${identity}.json`),
        "utf8",
      ),
    ).toContain('"session-shutdown"');
  });
});
