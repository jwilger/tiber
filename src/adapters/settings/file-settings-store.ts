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
  parseSettingsDocument,
  type SettingsDocument,
  type SettingsFailure,
  type SettingsOverrides,
  type SettingsResult,
} from "../../core/configuration/settings.js";

export interface SettingsSnapshot {
  readonly projectId: string;
  readonly globalValues: SettingsOverrides;
  readonly projectValues: SettingsOverrides;
}

function failed(
  code: SettingsFailure["code"],
  message: string,
  retryable = false,
): SettingsResult<never> {
  return {
    ok: false,
    failure: { code, message, retryable },
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
    return { ok: true, value: {} };
  }

  try {
    const document = parseJson(readFileSync(path, "utf8"), path);
    return document.ok ? { ok: true, value: document.value.values } : document;
  } catch {
    return failed(
      "TIBER_SETTINGS_IO",
      `could not read settings: ${path}`,
      true,
    );
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
  const document: SettingsDocument = { schemaVersion: 1, values };
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
    return failed(
      "TIBER_SETTINGS_IO",
      `could not write settings: ${path}`,
      true,
    );
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

function readIdentity(identityPath: string): SettingsResult<string> {
  try {
    const identity = readFileSync(identityPath, "utf8").trim();
    if (
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
        identity,
      )
    ) {
      return { ok: true, value: identity };
    }
    return failed(
      "TIBER_SETTINGS_INVALID_DOCUMENT",
      `repository identity is malformed: ${identityPath}`,
    );
  } catch {
    return failed(
      "TIBER_SETTINGS_IO",
      `could not read repository identity: ${identityPath}`,
      true,
    );
  }
}

function projectIdentity(cwd: string): SettingsResult<string> {
  const commonDirectory = gitCommonDirectory(cwd);
  if (!commonDirectory.ok) {
    return commonDirectory;
  }

  const identityPath = join(commonDirectory.value, "tiber", "project-id");
  if (existsSync(identityPath)) {
    return readIdentity(identityPath);
  }

  const identity = randomUUID();
  try {
    mkdirSync(dirname(identityPath), { recursive: true, mode: 0o700 });
    writeFileSync(identityPath, `${identity}\n`, {
      encoding: "utf8",
      mode: 0o600,
      flag: "wx",
    });
    return { ok: true, value: identity };
  } catch {
    return existsSync(identityPath)
      ? readIdentity(identityPath)
      : failed(
          "TIBER_SETTINGS_IO",
          `could not create repository identity: ${identityPath}`,
          true,
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

  private projectPath(projectId: string): string {
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
    projectId: string,
    values: SettingsOverrides,
  ): SettingsResult<void> {
    return writeValues(this.projectPath(projectId), values);
  }
}
