import { randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

import {
  operationalFailure,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";
import {
  parseProcessGroupId,
  parseProcessId,
  parseProcessStartedAt,
  type ProcessGroupId,
  type ProcessId,
  type ProcessStartedAt,
} from "../../core/processes/process-values.js";
import {
  parseTaskClaimId,
  parseTaskId,
  type TaskClaimId,
  type TaskId,
} from "../../core/tasks/task-values.js";

export interface OwnedProcessGroup {
  readonly schemaVersion: 1;
  readonly taskId: TaskId;
  readonly claimId: TaskClaimId;
  readonly pid: ProcessId;
  readonly processGroupId: ProcessGroupId;
  readonly startedAt: ProcessStartedAt;
}

type ProcessFailureCode =
  | "TIBER_PROCESS_INVALID"
  | "TIBER_PROCESS_QUOTA"
  | "TIBER_PROCESS_REGISTRY_INVALID"
  | "TIBER_PROCESS_REGISTRY_IO"
  | "TIBER_PROCESS_TERMINATION_FAILED";
export type ProcessFailure = TiberFailure<
  ProcessFailureCode,
  { readonly domain: "process-registry" },
  "corrected-input" | "state-change" | "retry-operation"
>;

export type ProcessRegistryResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: ProcessFailure;
    };

function failure(
  code: ProcessFailureCode,
  message: string,
): ProcessRegistryResult<never> {
  const retryability =
    code === "TIBER_PROCESS_REGISTRY_IO" ||
    code === "TIBER_PROCESS_TERMINATION_FAILED"
      ? "transient"
      : code === "TIBER_PROCESS_INVALID" ||
          code === "TIBER_PROCESS_REGISTRY_INVALID"
        ? "retry-after-input"
        : "retry-after-state-change";
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "process-registry",
      message,
      retryability,
    ),
  };
}

function parseGroup(value: unknown): OwnedProcessGroup | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return undefined;
  if (
    Object.keys(value).sort().join(",") !==
      "claimId,pid,processGroupId,schemaVersion,startedAt,taskId" ||
    !("schemaVersion" in value) ||
    value.schemaVersion !== 1 ||
    !("taskId" in value) ||
    !("claimId" in value) ||
    !("pid" in value) ||
    !("processGroupId" in value) ||
    !("startedAt" in value)
  )
    return undefined;
  const taskId = parseTaskId(value.taskId);
  const claimId = parseTaskClaimId(value.claimId);
  const pid = parseProcessId(value.pid);
  const processGroupId = parseProcessGroupId(value.processGroupId);
  const startedAt = parseProcessStartedAt(value.startedAt);
  return taskId.ok &&
    claimId.ok &&
    pid.ok &&
    processGroupId.ok &&
    Number(processGroupId.value) === Number(pid.value) &&
    startedAt.ok
    ? {
        schemaVersion: 1,
        taskId: taskId.value,
        claimId: claimId.value,
        pid: pid.value,
        processGroupId: processGroupId.value,
        startedAt: startedAt.value,
      }
    : undefined;
}

export type ProcessUnregisterOutcome = "absent" | "unregistered";

export class FileProcessGroupRegistry {
  private readonly path: string;

  public constructor(agentDirectory: string) {
    this.path = join(agentDirectory, "tiber", "process-groups.v1.json");
  }

  public read(): ProcessRegistryResult<readonly OwnedProcessGroup[]> {
    if (!existsSync(this.path)) return { ok: true, value: [] };
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.path, "utf8"));
      if (!Array.isArray(parsed) || parsed.length > 16)
        return failure(
          "TIBER_PROCESS_REGISTRY_INVALID",
          "owned process-group registry is malformed",
        );
      const groups = parsed.map(parseGroup);
      if (
        groups.some((group) => group === undefined) ||
        new Set(groups.map((group) => group?.pid)).size !== groups.length
      )
        return failure(
          "TIBER_PROCESS_REGISTRY_INVALID",
          "owned process-group registry is malformed",
        );
      return {
        ok: true,
        value: groups.filter((group) => group !== undefined),
      };
    } catch (error) {
      return error instanceof SyntaxError
        ? failure(
            "TIBER_PROCESS_REGISTRY_INVALID",
            "owned process-group registry is malformed",
          )
        : failure(
            "TIBER_PROCESS_REGISTRY_IO",
            "owned process-group registry could not be read",
          );
    }
  }

  public register(
    group: OwnedProcessGroup,
  ): ProcessRegistryResult<OwnedProcessGroup> {
    if (parseGroup(group) === undefined)
      return failure("TIBER_PROCESS_INVALID", "process ownership is invalid");
    const groups = this.read();
    if (!groups.ok) return groups;
    if (groups.value.length >= 16)
      return failure("TIBER_PROCESS_QUOTA", "owned process quota is exhausted");
    if (!this.write([...groups.value, group]))
      return failure(
        "TIBER_PROCESS_REGISTRY_IO",
        "process ownership was not durable",
      );
    return { ok: true, value: group };
  }

  public unregister(
    pid: ProcessId,
  ): ProcessRegistryResult<ProcessUnregisterOutcome> {
    const groups = this.read();
    if (!groups.ok) return groups;
    const remaining = groups.value.filter((group) => group.pid !== pid);
    if (remaining.length === groups.value.length)
      return { ok: true, value: "absent" };
    if (!this.write(remaining))
      return failure(
        "TIBER_PROCESS_REGISTRY_IO",
        "process completion was not durable",
      );
    return { ok: true, value: "unregistered" };
  }

  public reconcile(): ProcessRegistryResult<readonly OwnedProcessGroup[]> {
    const groups = this.read();
    if (!groups.ok) return groups;
    const alive = groups.value.filter((group) => {
      try {
        process.kill(group.pid, 0);
        return true;
      } catch {
        return false;
      }
    });
    if (alive.length !== groups.value.length && !this.write(alive))
      return failure(
        "TIBER_PROCESS_REGISTRY_IO",
        "reconciliation was not durable",
      );
    return { ok: true, value: alive };
  }

  public terminateAll(): ProcessRegistryResult<readonly ProcessGroupId[]> {
    const groups = this.read();
    if (!groups.ok) return groups;
    if (groups.value.length === 0) return { ok: true, value: [] };
    const terminated: ProcessGroupId[] = [];
    for (const group of groups.value) {
      try {
        process.kill(
          process.platform === "win32"
            ? group.processGroupId
            : -Number(group.processGroupId),
          "SIGTERM",
        );
        terminated.push(group.processGroupId);
      } catch (error) {
        const code =
          typeof error === "object" &&
          error !== null &&
          "code" in error &&
          typeof error.code === "string"
            ? error.code
            : undefined;
        if (code !== "ESRCH")
          return failure(
            "TIBER_PROCESS_TERMINATION_FAILED",
            `owned process group ${String(group.processGroupId)} could not be terminated`,
          );
      }
    }
    if (!this.write([]))
      return failure(
        "TIBER_PROCESS_REGISTRY_IO",
        "termination receipt was not durable",
      );
    return { ok: true, value: terminated };
  }

  private write(groups: readonly OwnedProcessGroup[]): boolean {
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(groups, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, this.path);
      return true;
    } catch {
      rmSync(temporary, { force: true });
      return false;
    }
  }
}
