import { spawn } from "node:child_process";

import type { StructuredCommand } from "../../core/commands/structured-command.js";
import type { CommandOutput } from "../../core/artifacts/output-virtualization.js";
import type { FileProcessGroupRegistry } from "../processes/file-process-group-registry.js";

export type CommandRunResult =
  | { readonly ok: true; readonly output: CommandOutput }
  | {
      readonly ok: false;
      readonly failure: { readonly code: string; readonly message: string };
    };

export class StructuredCommandRunner {
  public constructor(private readonly processes: FileProcessGroupRegistry) {}

  public run(
    command: StructuredCommand,
    worktree: string,
    ownership: { readonly taskId: string; readonly claimId: string },
    signal?: AbortSignal,
  ): Promise<CommandRunResult> {
    return new Promise((resolve) => {
      const started = Date.now();
      const child = spawn(command.executable, [...command.argv], {
        cwd: worktree,
        env: { ...command.environment },
        detached: process.platform !== "win32",
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
      });
      const pid = child.pid;
      if (pid === undefined) {
        resolve({
          ok: false,
          failure: {
            code: "TIBER_COMMAND_START_FAILED",
            message: "process id is unavailable",
          },
        });
        return;
      }
      const registration = this.processes.register({
        schemaVersion: 1,
        taskId: ownership.taskId,
        claimId: ownership.claimId,
        pid,
        processGroupId: pid,
        startedAt: new Date(started).toISOString(),
      });
      if (!registration.ok) {
        try {
          process.kill(process.platform === "win32" ? pid : -pid, "SIGKILL");
        } catch {
          // The process may already have exited.
        }
        resolve({ ok: false, failure: registration.failure });
        return;
      }

      const stdout: Buffer[] = [];
      const stderr: Buffer[] = [];
      let byteLength = 0;
      let exceeded = false;
      let settled = false;
      const maximumArtifactBytes = 10 * 1024 * 1024;
      const capture = (target: Buffer[], chunk: Buffer): void => {
        if (exceeded) return;
        byteLength += chunk.length;
        if (byteLength > maximumArtifactBytes) {
          exceeded = true;
          try {
            process.kill(process.platform === "win32" ? pid : -pid, "SIGKILL");
          } catch {
            // The process may already have exited.
          }
          return;
        }
        target.push(chunk);
      };
      child.stdout.on("data", (chunk: Buffer) => {
        capture(stdout, chunk);
      });
      child.stderr.on("data", (chunk: Buffer) => {
        capture(stderr, chunk);
      });
      let forcedKill: NodeJS.Timeout | undefined;
      const timer = setTimeout(() => {
        try {
          process.kill(process.platform === "win32" ? pid : -pid, "SIGTERM");
          forcedKill = setTimeout(() => {
            try {
              process.kill(
                process.platform === "win32" ? pid : -pid,
                "SIGKILL",
              );
            } catch {
              // The process may already have exited.
            }
          }, 1_000);
          forcedKill.unref();
        } catch {
          // The process may already have exited.
        }
      }, command.timeoutMs);
      timer.unref();
      const abort = (): void => {
        try {
          process.kill(process.platform === "win32" ? pid : -pid, "SIGTERM");
        } catch {
          // The process may already have exited.
        }
      };
      signal?.addEventListener("abort", abort, { once: true });

      child.on("error", (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (forcedKill !== undefined) clearTimeout(forcedKill);
        signal?.removeEventListener("abort", abort);
        this.processes.unregister(pid);
        resolve({
          ok: false,
          failure: {
            code: "TIBER_COMMAND_START_FAILED",
            message: error.message,
          },
        });
      });
      child.on("close", (code) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (forcedKill !== undefined) clearTimeout(forcedKill);
        signal?.removeEventListener("abort", abort);
        const receipt = this.processes.unregister(pid);
        if (!receipt.ok) {
          resolve({ ok: false, failure: receipt.failure });
          return;
        }
        resolve({
          ok: true,
          output: {
            stdout: Buffer.concat(stdout).toString("utf8"),
            stderr: exceeded
              ? `${Buffer.concat(stderr).toString("utf8")}\nTIBER_COMMAND_OUTPUT_LIMIT: output exceeded 10485760 bytes`
              : Buffer.concat(stderr).toString("utf8"),
            exitCode: code,
            durationMs: Date.now() - started,
          },
        });
      });
    });
  }
}
