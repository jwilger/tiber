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

import type { ProjectId } from "../../core/configuration/configuration-values.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import {
  parsePermissionDecisionAt,
  parsePermissionScope,
  type PermissionDecisionAt,
  type PermissionScope,
} from "../../core/permissions/permission-values.js";
import type { RememberedPermission } from "../../core/permissions/permission-policy.js";
import { none, some, type Option } from "../../core/types/option.js";

type PermissionStoreFailure = TiberFailure<
  "TIBER_PERMISSION_STATE_INVALID" | "TIBER_PERMISSION_STATE_IO",
  { readonly domain: "permission-state" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type Result<Value> = TiberResult<Value, PermissionStoreFailure>;

interface PermissionRecord {
  readonly scope: PermissionScope;
  readonly decision: RememberedPermission;
  readonly decidedAt: PermissionDecisionAt;
}

function failure(
  code: PermissionStoreFailure["code"],
  message: string,
): Result<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "permission-state",
      message,
      code === "TIBER_PERMISSION_STATE_IO" ? "transient" : "retry-after-input",
    ),
  };
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(
  value: Readonly<Record<string, unknown>>,
  keys: readonly string[],
): boolean {
  return Object.keys(value).sort().join(",") === [...keys].sort().join(",");
}

function parseDocument(value: unknown): Result<readonly PermissionRecord[]> {
  if (
    !isRecord(value) ||
    !exactKeys(value, ["schemaVersion", "records"]) ||
    value.schemaVersion !== 1 ||
    !Array.isArray(value.records) ||
    value.records.length > 10_000
  )
    return failure(
      "TIBER_PERMISSION_STATE_INVALID",
      "permission state document is invalid",
    );
  const records: PermissionRecord[] = [];
  for (const input of value.records) {
    if (
      !isRecord(input) ||
      !exactKeys(input, ["scope", "decision", "decidedAt"]) ||
      (input.decision !== "allow" && input.decision !== "deny")
    )
      return failure(
        "TIBER_PERMISSION_STATE_INVALID",
        "permission state record is invalid",
      );
    const scope = parsePermissionScope(input.scope);
    const decidedAt = parsePermissionDecisionAt(input.decidedAt);
    if (!scope.ok || !decidedAt.ok)
      return failure(
        "TIBER_PERMISSION_STATE_INVALID",
        "permission state values are invalid",
      );
    records.push({
      scope: scope.value,
      decision: input.decision,
      decidedAt: decidedAt.value,
    });
  }
  return { ok: true, value: records };
}

export class FilePermissionStore {
  private readonly path: string;

  public constructor(agentDirectory: string, projectId: ProjectId) {
    this.path = join(
      agentDirectory,
      "tiber",
      "projects",
      projectId,
      "permissions.v1.json",
    );
  }

  private load(): Result<readonly PermissionRecord[]> {
    if (!existsSync(this.path)) return { ok: true, value: [] };
    let text: string;
    try {
      text = readFileSync(this.path, "utf8");
    } catch {
      return failure(
        "TIBER_PERMISSION_STATE_IO",
        "permission state could not be read",
      );
    }
    try {
      const input: unknown = JSON.parse(text);
      return parseDocument(input);
    } catch {
      return failure(
        "TIBER_PERMISSION_STATE_INVALID",
        "permission state is not valid JSON",
      );
    }
  }

  public lookup(scope: PermissionScope): Result<Option<RememberedPermission>> {
    const loaded = this.load();
    if (!loaded.ok) return loaded;
    const record = [...loaded.value]
      .reverse()
      .find((candidate) => candidate.scope === scope);
    return {
      ok: true,
      value: record === undefined ? none : some(record.decision),
    };
  }

  public remember(
    scope: PermissionScope,
    decision: RememberedPermission,
    decidedAt: PermissionDecisionAt,
  ): Result<void> {
    const loaded = this.load();
    if (!loaded.ok) return loaded;
    if (loaded.value.length >= 10_000)
      return failure(
        "TIBER_PERMISSION_STATE_INVALID",
        "permission state record limit was reached",
      );
    const document = {
      schemaVersion: 1,
      records: [...loaded.value, { scope, decision, decidedAt }],
    };
    const temporaryPath = `${this.path}.${String(process.pid)}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      writeFileSync(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporaryPath, this.path);
      const observed: unknown = JSON.parse(readFileSync(this.path, "utf8"));
      return parseDocument(observed).ok
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_PERMISSION_STATE_INVALID",
            "permission state observation is invalid",
          );
    } catch {
      rmSync(temporaryPath, { force: true });
      return failure(
        "TIBER_PERMISSION_STATE_IO",
        "permission state could not be persisted",
      );
    }
  }
}
