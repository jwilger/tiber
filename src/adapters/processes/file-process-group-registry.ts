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

export interface OwnedProcessGroup {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly claimId: string;
  readonly pid: number;
  readonly processGroupId: number;
  readonly startedAt: string;
}

export type ProcessRegistryResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: { readonly code: string; readonly message: string };
    };

const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

function valid(value: unknown): value is OwnedProcessGroup {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return false;
  return (
    Object.keys(value).sort().join(",") ===
      "claimId,pid,processGroupId,schemaVersion,startedAt,taskId" &&
    "schemaVersion" in value &&
    value.schemaVersion === 1 &&
    "taskId" in value &&
    typeof value.taskId === "string" &&
    UUID.test(value.taskId) &&
    "claimId" in value &&
    typeof value.claimId === "string" &&
    UUID.test(value.claimId) &&
    "pid" in value &&
    typeof value.pid === "number" &&
    Number.isSafeInteger(value.pid) &&
    value.pid > 1 &&
    "processGroupId" in value &&
    value.processGroupId === value.pid &&
    "startedAt" in value &&
    typeof value.startedAt === "string" &&
    Number.isFinite(Date.parse(value.startedAt))
  );
}

export class FileProcessGroupRegistry {
  private readonly path: string;

  public constructor(agentDirectory: string) {
    this.path = join(agentDirectory, "tiber", "process-groups.v1.json");
  }

  public read(): ProcessRegistryResult<readonly OwnedProcessGroup[]> {
    if (!existsSync(this.path)) return { ok: true, value: [] };
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.path, "utf8"));
      if (
        !Array.isArray(parsed) ||
        parsed.length > 16 ||
        !parsed.every(valid) ||
        new Set(parsed.map((item) => item.pid)).size !== parsed.length
      )
        throw new Error("invalid process registry");
      return { ok: true, value: parsed };
    } catch {
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_REGISTRY_INVALID",
          message: "owned process-group registry is malformed",
        },
      };
    }
  }

  public register(
    group: OwnedProcessGroup,
  ): ProcessRegistryResult<OwnedProcessGroup> {
    if (!valid(group))
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_INVALID",
          message: "process ownership is invalid",
        },
      };
    const groups = this.read();
    if (!groups.ok) return groups;
    if (groups.value.length >= 16)
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_QUOTA",
          message: "owned process quota is exhausted",
        },
      };
    if (!this.write([...groups.value, group]))
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_REGISTRY_IO",
          message: "process ownership was not durable",
        },
      };
    return { ok: true, value: group };
  }

  public unregister(pid: number): ProcessRegistryResult<boolean> {
    const groups = this.read();
    if (!groups.ok) return groups;
    const remaining = groups.value.filter((group) => group.pid !== pid);
    if (remaining.length === groups.value.length)
      return { ok: true, value: false };
    if (!this.write(remaining))
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_REGISTRY_IO",
          message: "process completion was not durable",
        },
      };
    return { ok: true, value: true };
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
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_REGISTRY_IO",
          message: "reconciliation was not durable",
        },
      };
    return { ok: true, value: alive };
  }

  public terminateAll(): ProcessRegistryResult<readonly number[]> {
    const groups = this.read();
    if (!groups.ok) return groups;
    if (groups.value.length === 0) return { ok: true, value: [] };
    const terminated: number[] = [];
    for (const group of groups.value) {
      try {
        process.kill(
          process.platform === "win32"
            ? group.processGroupId
            : -group.processGroupId,
          "SIGTERM",
        );
        terminated.push(group.processGroupId);
      } catch (error) {
        const code = (error as NodeJS.ErrnoException).code;
        if (code !== "ESRCH")
          return {
            ok: false,
            failure: {
              code: "TIBER_PROCESS_TERMINATION_FAILED",
              message: `owned process group ${String(group.processGroupId)} could not be terminated`,
            },
          };
      }
    }
    if (!this.write([]))
      return {
        ok: false,
        failure: {
          code: "TIBER_PROCESS_REGISTRY_IO",
          message: "termination receipt was not durable",
        },
      };
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
