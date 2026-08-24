import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import type {
  ExceptionExecutionAttempt,
  ExceptionExecutionObservation,
} from "../../core/exceptions/human-exception.js";
import { verifyFrozenExceptionClaim } from "./exception-state-observer.js";
import {
  succeed,
  type Result,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";

type ExecutionFailure = TiberFailure<string, unknown, unknown>;

export async function executeFrozenException(
  attempt: ExceptionExecutionAttempt,
): Promise<Result<ExceptionExecutionObservation, ExecutionFailure>> {
  const verified = await verifyFrozenExceptionClaim(attempt.claim);
  if (!verified.ok) return verified;
  const operation = verified.value.operation;
  return new Promise((resolve) => {
    const stdout = createHash("sha256");
    const stderr = createHash("sha256");
    let outputBytes = 0;
    let terminal = false;
    const child = spawn(operation.executable, operation.arguments, {
      cwd: operation.workingDirectory,
      env: Object.fromEntries(
        operation.environment.map((entry) => [entry.name, entry.value]),
      ),
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const timeout = setTimeout(
      () => child.kill("SIGKILL"),
      operation.timeoutMs,
    );
    const consume = (
      hash: ReturnType<typeof createHash>,
      chunk: Buffer,
    ): void => {
      outputBytes += chunk.length;
      hash.update(chunk);
      if (outputBytes > operation.maxOutputBytes) child.kill("SIGKILL");
    };
    child.stdout.on("data", (chunk: Buffer) => {
      consume(stdout, chunk);
    });
    child.stderr.on("data", (chunk: Buffer) => {
      consume(stderr, chunk);
    });
    child.once("error", () => {
      if (terminal) return;
      terminal = true;
      clearTimeout(timeout);
      resolve(
        succeed({
          attemptId: attempt.attemptId,
          exitCode: -1,
          stdoutDigest: stdout.digest("hex"),
          stderrDigest: stderr.digest("hex"),
          observedAt: new Date().toISOString(),
        }),
      );
    });
    child.once("close", (code) => {
      if (terminal) return;
      terminal = true;
      clearTimeout(timeout);
      resolve(
        succeed({
          attemptId: attempt.attemptId,
          exitCode: code ?? -1,
          stdoutDigest: stdout.digest("hex"),
          stderrDigest: stderr.digest("hex"),
          observedAt: new Date().toISOString(),
        }),
      );
    });
  });
}
