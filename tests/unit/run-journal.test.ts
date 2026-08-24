import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { mkdirSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { FileRunJournal } from "../../src/adapters/runs/file-run-journal.js";

const taskId = "2424c876-6180-4c64-976e-9ea4bd540744";
const record = {
  schemaVersion: 1 as const,
  taskId,
  claimId: "00000000-0000-4000-8000-000000000001",
  baselineRevision: "a".repeat(40),
  workflowDigest: `sha256:${"b".repeat(64)}`,
  state: "red-accepted" as const,
  worktreePath: "/worktree",
  redReceipt: {
    scenarioName: "scenario",
    testMapping: "tests/scenario.test.ts",
    diagnosticDigest: `sha256:${"c".repeat(64)}`,
    missingPublicSurface: true,
  },
};

describe("durable RED run receipt", () => {
  it("round-trips an accepted RED receipt", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-run-"));
    const journal = new FileRunJournal(root);
    expect(journal.write(record)).toBe(true);
    expect(new FileRunJournal(root).read(taskId)).toEqual(record);
  });

  it("fails closed when accepted RED lacks its receipt", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-run-"));
    const path = join(root, "tiber", "runs", `${taskId}.json`);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify({ ...record, redReceipt: undefined }));
    expect(new FileRunJournal(root).read(taskId)).toBeUndefined();
  });
});
