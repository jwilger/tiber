import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { FileProcessGroupRegistry } from "../../src/adapters/processes/file-process-group-registry.js";
import {
  processGroupId,
  processId,
  processStartedAt,
} from "../fixtures/process-values.js";
import { taskClaimId, taskId } from "../fixtures/task-values.js";

function detachedChild() {
  const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    detached: true,
    stdio: "ignore",
  });
  if (child.pid === undefined) throw new Error("child process did not start");
  child.unref();
  return { child, pid: child.pid };
}

describe("owned process-group recovery", () => {
  it.skipIf(process.platform === "win32")(
    "reconciles a live detached group after restart and terminates it on shutdown",
    async () => {
      const root = mkdtempSync(join(tmpdir(), "tiber-process-"));
      const child = spawn(
        process.execPath,
        ["-e", "setInterval(() => {}, 1000)"],
        {
          detached: true,
          stdio: "ignore",
        },
      );
      if (child.pid === undefined)
        throw new Error("child process did not start");
      child.unref();
      const registry = new FileProcessGroupRegistry(root);
      expect(
        registry.register({
          schemaVersion: 1,
          taskId: taskId("2424c876-6180-4c64-976e-9ea4bd540744"),
          claimId: taskClaimId("00000000-0000-4000-8000-000000000001"),
          pid: processId(child.pid),
          processGroupId: processGroupId(child.pid),
          startedAt: processStartedAt("2026-08-23T16:00:00.000Z"),
        }),
      ).toMatchObject({ ok: true });

      expect(new FileProcessGroupRegistry(root).reconcile()).toMatchObject({
        ok: true,
        value: [{ pid: child.pid, processGroupId: child.pid }],
      });
      expect(new FileProcessGroupRegistry(root).terminateAll()).toEqual({
        ok: true,
        value: [child.pid],
      });
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(new FileProcessGroupRegistry(root).read()).toEqual({
        ok: true,
        value: [],
      });
      expect(() => process.kill(child.pid ?? 0, 0)).toThrow();
    },
  );

  it.skipIf(process.platform === "win32")(
    "terminates only the exact task claim during completion cleanup",
    async () => {
      const root = mkdtempSync(join(tmpdir(), "tiber-process-task-"));
      const first = detachedChild();
      const second = detachedChild();
      const registry = new FileProcessGroupRegistry(root);
      const firstTask = taskId("2424c876-6180-4c64-976e-9ea4bd540744");
      const firstClaim = taskClaimId("00000000-0000-4000-8000-000000000001");
      const secondTask = taskId("3434c876-6180-4c64-976e-9ea4bd540744");
      const secondClaim = taskClaimId("00000000-0000-4000-8000-000000000002");
      for (const owned of [
        {
          schemaVersion: 1 as const,
          taskId: firstTask,
          claimId: firstClaim,
          pid: processId(first.pid),
          processGroupId: processGroupId(first.pid),
          startedAt: processStartedAt("2026-08-23T16:00:00.000Z"),
        },
        {
          schemaVersion: 1 as const,
          taskId: secondTask,
          claimId: secondClaim,
          pid: processId(second.pid),
          processGroupId: processGroupId(second.pid),
          startedAt: processStartedAt("2026-08-23T16:00:01.000Z"),
        },
      ])
        expect(registry.register(owned)).toMatchObject({ ok: true });
      expect(registry.terminateTask(firstTask, firstClaim)).toEqual({
        ok: true,
        value: [first.pid],
      });
      expect(registry.read()).toMatchObject({
        ok: true,
        value: [{ taskId: secondTask, claimId: secondClaim }],
      });
      expect(registry.terminateAll()).toMatchObject({ ok: true });
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(() => process.kill(first.pid, 0)).toThrow();
      expect(() => process.kill(second.pid, 0)).toThrow();
    },
  );
});
