import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { FileProcessGroupRegistry } from "../../src/adapters/processes/file-process-group-registry.js";

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
          taskId: "2424c876-6180-4c64-976e-9ea4bd540744",
          claimId: "00000000-0000-4000-8000-000000000001",
          pid: child.pid,
          processGroupId: child.pid,
          startedAt: "2026-08-23T16:00:00.000Z",
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
});
