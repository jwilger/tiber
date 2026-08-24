import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { describe, expect, it } from "vitest";

import { FileRunJournal } from "../../src/adapters/runs/file-run-journal.js";
import { some } from "../../src/core/types/option.js";
import { parseOwnedWorktreePath } from "../../src/core/worktrees/worktree-values.js";
import {
  claimBaselineRevision,
  scenarioName,
  taskClaimId,
  taskId as semanticTaskId,
  testMappingPath,
} from "../fixtures/task-values.js";
import {
  compiledWorkflowDigest,
  redDiagnosticDigest,
} from "../fixtures/workflow-values.js";

const taskId = semanticTaskId("2424c876-6180-4c64-976e-9ea4bd540744");
const worktree = parseOwnedWorktreePath("/worktree");
if (!worktree.ok) throw new Error("invalid worktree fixture");
const record = {
  schemaVersion: 1 as const,
  taskId,
  claimId: taskClaimId("00000000-0000-4000-8000-000000000001"),
  baselineRevision: claimBaselineRevision("a".repeat(40)),
  workflowDigest: compiledWorkflowDigest(`sha256:${"b".repeat(64)}`),
  state: "red-accepted" as const,
  worktreePath: some(worktree.value),
  redReceipt: some({
    scenarioName: scenarioName("scenario"),
    testMapping: testMappingPath("tests/scenario.test.ts"),
    diagnosticDigest: redDiagnosticDigest(`sha256:${"c".repeat(64)}`),
    missingPublicSurface: true,
  }),
};

describe("durable RED run receipt", () => {
  it("round-trips an accepted RED receipt", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-run-"));
    const journal = new FileRunJournal(root);
    expect(journal.write(record)).toEqual({ ok: true, value: undefined });
    expect(new FileRunJournal(root).read(taskId)).toEqual({
      ok: true,
      value: some(record),
    });
  });

  it("fails closed when accepted RED lacks its receipt", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-run-"));
    const path = join(root, "tiber", "runs", `${taskId}.json`);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(
      path,
      JSON.stringify({
        schemaVersion: 1,
        taskId,
        claimId: record.claimId,
        baselineRevision: record.baselineRevision,
        workflowDigest: record.workflowDigest,
        state: "red-accepted",
        worktreePath: worktree.value,
      }),
    );
    expect(new FileRunJournal(root).read(taskId)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_RUN_JOURNAL_INVALID" },
    });
  });
});
