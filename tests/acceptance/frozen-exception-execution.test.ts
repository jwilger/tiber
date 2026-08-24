import { execFile } from "node:child_process";
import { mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "vitest";
import { freezeExceptionClaim } from "../../src/adapters/exceptions/exception-state-observer.js";
import { executeFrozenException } from "../../src/adapters/exceptions/frozen-exception-executor.js";

const execute = promisify(execFile);

describe("frozen human exception execution", () => {
  it("executes only the frozen shell-free executable and rejects repository drift", async () => {
    const cwd = await mkdtemp(path.join(tmpdir(), "tiber-frozen-"));
    try {
      await execute("git", ["init", "-q", cwd]);
      await execute(
        "git",
        ["-C", cwd, "commit", "--allow-empty", "-m", "baseline"],
        {
          env: {
            ...process.env,
            GIT_AUTHOR_NAME: "Tiber",
            GIT_AUTHOR_EMAIL: "tiber@example.invalid",
            GIT_COMMITTER_NAME: "Tiber",
            GIT_COMMITTER_EMAIL: "tiber@example.invalid",
          },
        },
      );
      await writeFile(path.join(cwd, "state.txt"), "frozen");
      await symlink(tmpdir(), path.join(cwd, "escape"));
      const escaped = await freezeExceptionClaim({
        taskId: "task",
        runId: "escape",
        goal: "escape",
        denialCode: "DENIED",
        compliantAlternatives: [],
        operation: {
          kind: "structured-command",
          executable: process.execPath,
          arguments: [],
          environment: [],
          workingDirectory: cwd,
          timeoutMs: 5_000,
          maxOutputBytes: 1024,
          paths: ["escape/missing"],
        },
      });
      expect(escaped.ok).toBe(false);
      const claim = await freezeExceptionClaim({
        taskId: "task",
        runId: "run",
        goal: "exact output",
        denialCode: "DENIED",
        compliantAlternatives: [],
        operation: {
          kind: "structured-command",
          executable: process.execPath,
          arguments: ["-e", "process.stdout.write('exact-output')"],
          environment: [],
          workingDirectory: cwd,
          timeoutMs: 5_000,
          maxOutputBytes: 1024,
          paths: ["state.txt"],
        },
      });
      expect(claim.ok).toBe(true);
      if (!claim.ok) return;
      const result = await executeFrozenException({
        attemptId: "attempt",
        claim: claim.value,
      });
      expect(result).toMatchObject({
        ok: true,
        value: {
          attemptId: "attempt",
          exitCode: 0,
          stdoutDigest:
            "0ac28d8dcc122673c44c779e992de8e4f27d7197c5f040e44d3fab204fc7f13d",
          stderrDigest:
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
      });
      await writeFile(path.join(cwd, "state.txt"), "drifted");
      expect(
        (
          await executeFrozenException({
            attemptId: "preimage-drift",
            claim: claim.value,
          })
        ).ok,
      ).toBe(false);
      await writeFile(path.join(cwd, "state.txt"), "frozen");
      await execute(
        "git",
        ["-C", cwd, "commit", "--allow-empty", "-m", "drift"],
        {
          env: {
            ...process.env,
            GIT_AUTHOR_NAME: "Tiber",
            GIT_AUTHOR_EMAIL: "tiber@example.invalid",
            GIT_COMMITTER_NAME: "Tiber",
            GIT_COMMITTER_EMAIL: "tiber@example.invalid",
          },
        },
      );
      expect(
        (
          await executeFrozenException({
            attemptId: "drift",
            claim: claim.value,
          })
        ).ok,
      ).toBe(false);
    } finally {
      await rm(cwd, { recursive: true, force: true });
    }
  });
});
