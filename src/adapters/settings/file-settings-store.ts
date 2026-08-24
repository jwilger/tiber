import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";

import {
  parseProjectId,
  type ProjectId,
} from "../../core/configuration/configuration-values.js";
import { none } from "../../core/types/option.js";
import {
  parseSettingsDocument,
  settingsFailure,
  type SettingsDocument,
  type SettingsFailure,
  type SettingsOverrides,
  type SettingsResult,
} from "../../core/configuration/settings.js";

export interface SettingsSnapshot {
  readonly projectId: ProjectId;
  readonly globalValues: SettingsOverrides;
  readonly projectValues: SettingsOverrides;
}

function failed(
  code: SettingsFailure["code"],
  message: string,
): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure(code, message),
  };
}

function parseJson(
  text: string,
  path: string,
): SettingsResult<SettingsDocument> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return failed(
      "TIBER_SETTINGS_INVALID_DOCUMENT",
      `settings file is not valid JSON: ${path}`,
    );
  }
  return parseSettingsDocument(parsed);
}

function readValues(path: string): SettingsResult<SettingsOverrides> {
  if (!existsSync(path)) {
    return {
      ok: true,
      value: {
        assuranceLevel: none,
        outputPreviewBytes: none,
        worktreeMode: none,
      },
    };
  }

  try {
    const document = parseJson(readFileSync(path, "utf8"), path);
    return document.ok ? { ok: true, value: document.value.values } : document;
  } catch {
    return failed("TIBER_SETTINGS_IO", `could not read settings: ${path}`);
  }
}

function removeTemporaryFile(path: string): void {
  try {
    rmSync(path, { force: true });
  } catch {
    return;
  }
}

function writeValues(
  path: string,
  values: SettingsOverrides,
): SettingsResult<void> {
  const document = {
    schemaVersion: 1,
    values: {
      ...(values.assuranceLevel.kind === "some"
        ? { assuranceLevel: values.assuranceLevel.value }
        : {}),
      ...(values.outputPreviewBytes.kind === "some"
        ? { outputPreviewBytes: values.outputPreviewBytes.value }
        : {}),
      ...(values.worktreeMode.kind === "some"
        ? { worktreeMode: values.worktreeMode.value }
        : {}),
    },
  };
  const temporaryPath = `${path}.${String(process.pid)}.${randomUUID()}.tmp`;

  try {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    writeFileSync(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, {
      encoding: "utf8",
      mode: 0o600,
      flag: "wx",
    });
    renameSync(temporaryPath, path);
    return { ok: true, value: undefined };
  } catch {
    removeTemporaryFile(temporaryPath);
    return failed("TIBER_SETTINGS_IO", `could not write settings: ${path}`);
  }
}

function gitCommonDirectory(cwd: string): SettingsResult<string> {
  try {
    const output = execFileSync(
      "git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    ).trim();
    const absolute = isAbsolute(output) ? output : resolve(cwd, output);
    return { ok: true, value: absolute };
  } catch {
    return failed(
      "TIBER_SETTINGS_REPOSITORY_REQUIRED",
      "project settings require a Git repository",
    );
  }
}

function readIdentity(identityPath: string): SettingsResult<ProjectId> {
  try {
    const identity = readFileSync(identityPath, "utf8").trim();
    const parsed = parseProjectId(identity);
    if (parsed.ok) return parsed;
    return failed(
      "TIBER_SETTINGS_INVALID_DOCUMENT",
      `repository identity is malformed: ${identityPath}`,
    );
  } catch {
    return failed(
      "TIBER_SETTINGS_IO",
      `could not read repository identity: ${identityPath}`,
    );
  }
}

function projectIdentity(cwd: string): SettingsResult<ProjectId> {
  const commonDirectory = gitCommonDirectory(cwd);
  if (!commonDirectory.ok) {
    return commonDirectory;
  }

  const identityPath = join(commonDirectory.value, "tiber", "project-id");
  if (existsSync(identityPath)) {
    return readIdentity(identityPath);
  }

  const identity = parseProjectId(randomUUID());
  if (!identity.ok)
    return failed(
      "TIBER_SETTINGS_INVALID_DOCUMENT",
      "generated repository identity is invalid",
    );
  try {
    mkdirSync(dirname(identityPath), { recursive: true, mode: 0o700 });
    writeFileSync(identityPath, `${identity.value}\n`, {
      encoding: "utf8",
      mode: 0o600,
      flag: "wx",
    });
    return identity;
  } catch {
    return existsSync(identityPath)
      ? readIdentity(identityPath)
      : failed(
          "TIBER_SETTINGS_IO",
          `could not create repository identity: ${identityPath}`,
        );
  }
}

export class FileSettingsStore {
  public constructor(
    private readonly agentDirectory: string,
    private readonly cwd: string,
  ) {}

  private globalPath(): string {
    return join(this.agentDirectory, "tiber", "settings.json");
  }

  private projectPath(projectId: ProjectId): string {
    return join(
      this.agentDirectory,
      "tiber",
      "projects",
      projectId,
      "settings.json",
    );
  }

  public load(): SettingsResult<SettingsSnapshot> {
    const identity = projectIdentity(this.cwd);
    if (!identity.ok) {
      return identity;
    }

    const globalValues = readValues(this.globalPath());
    if (!globalValues.ok) {
      return globalValues;
    }

    const projectValues = readValues(this.projectPath(identity.value));
    if (!projectValues.ok) {
      return projectValues;
    }

    return {
      ok: true,
      value: {
        projectId: identity.value,
        globalValues: globalValues.value,
        projectValues: projectValues.value,
      },
    };
  }

  public saveGlobal(values: SettingsOverrides): SettingsResult<void> {
    return writeValues(this.globalPath(), values);
  }

  public saveProject(
    projectId: ProjectId,
    values: SettingsOverrides,
  ): SettingsResult<void> {
    return writeValues(this.projectPath(projectId), values);
  }
}
