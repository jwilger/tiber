import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { createReadStream } from "node:fs";
import { lstat, realpath } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import {
  parseExceptionBlockerClaim,
  type ExceptionBlockerClaim,
} from "../../core/exceptions/human-exception.js";
import {
  fail,
  operationalFailure,
  type Result,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";

const executeFile = promisify(execFile);
type ObserverFailure = TiberFailure<string, unknown, unknown>;
export interface ExceptionClaimDraft {
  readonly taskId: string;
  readonly runId: string;
  readonly goal: string;
  readonly denialCode: string;
  readonly compliantAlternatives: readonly string[];
  readonly operation: Omit<ExceptionBlockerClaim["operation"], "preimages">;
}
const denied = (message: string) =>
  operationalFailure(
    "TIBER_EXCEPTION_STATE_INVALID",
    "exception-state",
    message,
    "not-retryable",
  );
const digest = (value: string): string =>
  createHash("sha256").update(value).digest("hex");

async function nearestExistingParentIsContained(
  root: string,
  target: string,
): Promise<boolean> {
  let candidate = path.dirname(target);
  while (candidate !== path.dirname(candidate)) {
    try {
      const physical = await realpath(candidate);
      return physical === root || physical.startsWith(`${root}${path.sep}`);
    } catch (cause) {
      if ((cause as NodeJS.ErrnoException).code !== "ENOENT") return false;
      candidate = path.dirname(candidate);
    }
  }
  return false;
}

async function fileDigest(
  root: string,
  relativePath: string,
): Promise<Result<string, ObserverFailure>> {
  if (relativePath.length === 0 || path.isAbsolute(relativePath))
    return fail(denied("exception path is not repository-relative"));
  const resolved = path.resolve(root, relativePath);
  if (resolved !== root && !resolved.startsWith(`${root}${path.sep}`))
    return fail(denied("exception path escapes the frozen working directory"));
  if (!(await nearestExistingParentIsContained(root, resolved)))
    return fail(denied("exception path parent escapes through a symlink"));
  try {
    const status = await lstat(resolved);
    if (!status.isFile() || status.isSymbolicLink())
      return fail(denied("exception path is not a regular non-symlink file"));
    const physical = await realpath(resolved);
    if (physical !== root && !physical.startsWith(`${root}${path.sep}`))
      return fail(denied("exception path escapes through a symlink"));
    const hash = createHash("sha256");
    await new Promise<void>((resolve, reject) => {
      const stream = createReadStream(physical);
      stream.on("data", (chunk: Buffer | string) => {
        hash.update(chunk);
      });
      stream.once("error", reject);
      stream.once("end", resolve);
    });
    return { ok: true, value: hash.digest("hex") };
  } catch (cause) {
    if ((cause as NodeJS.ErrnoException).code === "ENOENT")
      return { ok: true, value: digest("tiber:absent") };
    return fail(denied("exception preimage could not be observed"));
  }
}

export async function freezeExceptionClaim(
  draft: ExceptionClaimDraft,
): Promise<Result<ExceptionBlockerClaim, ObserverFailure>> {
  try {
    const root = await realpath(draft.operation.workingDirectory);
    const revisionOutput = await executeFile(
      "git",
      ["-C", root, "rev-parse", "HEAD"],
      { encoding: "utf8", timeout: 10_000, maxBuffer: 1024 },
    );
    const revision = revisionOutput.stdout.trim();
    const preimages: { path: string; digest: string }[] = [];
    for (const frozenPath of draft.operation.paths) {
      const observed = await fileDigest(root, frozenPath);
      if (!observed.ok) return observed;
      preimages.push({ path: frozenPath, digest: observed.value });
    }
    const stateDigest = digest(JSON.stringify({ revision, preimages }));
    const claim = parseExceptionBlockerClaim({
      schemaVersion: 1,
      taskId: draft.taskId,
      runId: draft.runId,
      revision,
      goal: draft.goal,
      denialCode: draft.denialCode,
      compliantAlternatives: draft.compliantAlternatives,
      operation: { ...draft.operation, workingDirectory: root, preimages },
      stateDigest,
    });
    return claim.ok
      ? claim
      : fail(denied("observed exception claim is invalid"));
  } catch {
    return fail(denied("exception repository state could not be observed"));
  }
}

export async function verifyFrozenExceptionClaim(
  expected: ExceptionBlockerClaim,
): Promise<Result<ExceptionBlockerClaim, ObserverFailure>> {
  const observed = await freezeExceptionClaim({
    taskId: expected.taskId,
    runId: expected.runId,
    goal: expected.goal,
    denialCode: expected.denialCode,
    compliantAlternatives: expected.compliantAlternatives,
    operation: {
      kind: expected.operation.kind,
      executable: expected.operation.executable,
      arguments: expected.operation.arguments,
      environment: expected.operation.environment,
      workingDirectory: expected.operation.workingDirectory,
      timeoutMs: expected.operation.timeoutMs,
      maxOutputBytes: expected.operation.maxOutputBytes,
      paths: expected.operation.paths,
    },
  });
  return observed.ok &&
    JSON.stringify(observed.value) === JSON.stringify(expected)
    ? observed
    : fail(denied("frozen exception state has drifted"));
}
