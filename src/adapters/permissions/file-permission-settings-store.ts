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
import type { AutonomyLevel } from "../../core/permissions/permission-policy.js";

export interface PermissionSettings {
  readonly schemaVersion: 1;
  readonly autonomy: AutonomyLevel;
}

type PermissionSettingsFailure = TiberFailure<
  "TIBER_PERMISSION_SETTINGS_INVALID" | "TIBER_PERMISSION_SETTINGS_IO",
  { readonly domain: "permission-settings" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type Result<Value> = TiberResult<Value, PermissionSettingsFailure>;

const DEFAULT_PERMISSION_SETTINGS: PermissionSettings = {
  schemaVersion: 1,
  autonomy: "routine",
};

function failure(
  code: PermissionSettingsFailure["code"],
  message: string,
): Result<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "permission-settings",
      message,
      code === "TIBER_PERMISSION_SETTINGS_IO"
        ? "transient"
        : "retry-after-input",
    ),
  };
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parse(input: unknown): Result<PermissionSettings> {
  if (
    !isRecord(input) ||
    Object.keys(input).sort().join(",") !== "autonomy,schemaVersion"
  )
    return failure(
      "TIBER_PERMISSION_SETTINGS_INVALID",
      "permission settings document is invalid",
    );
  return input.schemaVersion === 1 &&
    (input.autonomy === "ask-first" ||
      input.autonomy === "routine" ||
      input.autonomy === "repository")
    ? {
        ok: true,
        value: { schemaVersion: 1, autonomy: input.autonomy },
      }
    : failure(
        "TIBER_PERMISSION_SETTINGS_INVALID",
        "permission settings values are invalid",
      );
}

export class FilePermissionSettingsStore {
  private readonly path: string;

  public constructor(agentDirectory: string, projectId: ProjectId) {
    this.path = join(
      agentDirectory,
      "tiber",
      "projects",
      projectId,
      "permission-settings.v1.json",
    );
  }

  public load(): Result<PermissionSettings> {
    if (!existsSync(this.path))
      return { ok: true, value: DEFAULT_PERMISSION_SETTINGS };
    let text: string;
    try {
      text = readFileSync(this.path, "utf8");
    } catch {
      return failure(
        "TIBER_PERMISSION_SETTINGS_IO",
        "permission settings could not be read",
      );
    }
    try {
      const input: unknown = JSON.parse(text);
      return parse(input);
    } catch {
      return failure(
        "TIBER_PERMISSION_SETTINGS_INVALID",
        "permission settings are not valid JSON",
      );
    }
  }

  public save(settings: PermissionSettings): Result<void> {
    const parsed = parse(settings);
    if (!parsed.ok) return parsed;
    const temporaryPath = `${this.path}.${String(process.pid)}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      writeFileSync(
        temporaryPath,
        `${JSON.stringify(parsed.value, null, 2)}\n`,
        { encoding: "utf8", mode: 0o600, flag: "wx" },
      );
      renameSync(temporaryPath, this.path);
      const observed: unknown = JSON.parse(readFileSync(this.path, "utf8"));
      return parse(observed).ok
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_PERMISSION_SETTINGS_INVALID",
            "permission settings observation is invalid",
          );
    } catch {
      rmSync(temporaryPath, { force: true });
      return failure(
        "TIBER_PERMISSION_SETTINGS_IO",
        "permission settings could not be persisted",
      );
    }
  }
}
