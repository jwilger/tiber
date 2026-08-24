import { spawn } from "node:child_process";

import {
  parseCommandDurationMilliseconds,
  parseCommandExitCode,
  parseCommandStandardError,
  parseCommandStandardOutput,
  type CommandExitCode,
} from "../../core/artifacts/artifact-values.js";
import type { CommandOutput } from "../../core/artifacts/output-virtualization.js";
import type { StructuredCommand } from "../../core/commands/structured-command.js";
import {
  parseProcessGroupId,
  parseProcessId,
  parseProcessStartedAt,
} from "../../core/processes/process-values.js";
import type { TaskClaimId, TaskId } from "../../core/tasks/task-values.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  operationalFailure,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";
import type {
  FileProcessGroupRegistry,
  ProcessFailure,
} from "../processes/file-process-group-registry.js";

type CommandRunFailure = TiberFailure<
  | "TIBER_COMMAND_RECEIPT_INVALID"
  | "TIBER_COMMAND_START_FAILED"
  | "TIBER_PROCESS_INVALID",
  { readonly domain: "command-runner" },
  "corrected-input" | "state-change" | "retry-operation"
>;

export type CommandRunResult =
  | { readonly ok: true; readonly output: CommandOutput }
  | {
      readonly ok: false;
      readonly failure: CommandRunFailure | ProcessFailure;
    };

function failure(
  code: CommandRunFailure["code"],
  message: string,
): CommandRunResult {
  return {
    ok: false,
    failure: operationalFailure(code, "command-runner", message, "transient"),
  };
}

export class StructuredCommandRunner {
  public constructor(private readonly processes: FileProcessGroupRegistry) {}

  public run(
    command: StructuredCommand,
    worktree: string,
    ownership: { readonly taskId: TaskId; readonly claimId: TaskClaimId },
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
        resolve(
          failure("TIBER_COMMAND_START_FAILED", "process id is unavailable"),
        );
        return;
      }
      const processId = parseProcessId(pid);
      const processGroupId = parseProcessGroupId(pid);
      const startedAt = parseProcessStartedAt(new Date(started).toISOString());
      if (!processId.ok || !processGroupId.ok || !startedAt.ok) {
        resolve(
          failure(
            "TIBER_PROCESS_INVALID",
            "spawned process values violated semantic invariants",
          ),
        );
        return;
      }
      const registration = this.processes.register({
        schemaVersion: 1,
        taskId: ownership.taskId,
        claimId: ownership.claimId,
        pid: processId.value,
        processGroupId: processGroupId.value,
        startedAt: startedAt.value,
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
        this.processes.unregister(processId.value);
        resolve(failure("TIBER_COMMAND_START_FAILED", error.message));
      });
      child.on("close", (code) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (forcedKill !== undefined) clearTimeout(forcedKill);
        signal?.removeEventListener("abort", abort);
        const receipt = this.processes.unregister(processId.value);
        if (!receipt.ok) {
          resolve({ ok: false, failure: receipt.failure });
          return;
        }
        const parsedStdout = parseCommandStandardOutput(
          Buffer.concat(stdout).toString("utf8"),
        );
        const parsedStderr = parseCommandStandardError(
          exceeded
            ? `${Buffer.concat(stderr).toString("utf8")}\nTIBER_COMMAND_OUTPUT_LIMIT: output exceeded 10485760 bytes`
            : Buffer.concat(stderr).toString("utf8"),
        );
        let exitCode: Option<CommandExitCode> = none;
        if (code !== null) {
          const parsedExitCode = parseCommandExitCode(code);
          if (parsedExitCode.ok) exitCode = some(parsedExitCode.value);
          else {
            resolve(
              failure(
                "TIBER_COMMAND_RECEIPT_INVALID",
                "process exit code violated its semantic invariant",
              ),
            );
            return;
          }
        }
        const duration = parseCommandDurationMilliseconds(Date.now() - started);
        if (!parsedStdout.ok || !parsedStderr.ok || !duration.ok) {
          resolve(
            failure(
              "TIBER_COMMAND_RECEIPT_INVALID",
              "process completion values violated semantic invariants",
            ),
          );
          return;
        }
        resolve({
          ok: true,
          output: {
            stdout: parsedStdout.value,
            stderr: parsedStderr.value,
            exitCode,
            durationMs: duration.value,
          },
        });
      });
    });
  }
}
