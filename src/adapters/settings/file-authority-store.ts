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
  EMPTY_AUTHORITY,
  parseAuthorityDocument,
  type AuthorityDocument,
} from "../../core/configuration/authority.js";
import {
  settingsFailure,
  type SettingsResult,
} from "../../core/configuration/settings.js";

function ioFailure(message: string): SettingsResult<never> {
  return {
    ok: false,
    failure: settingsFailure("TIBER_SETTINGS_IO", message),
  };
}

function cleanup(path: string): void {
  try {
    rmSync(path, { force: true });
  } catch {
    return;
  }
}

export class FileAuthorityStore {
  public constructor(private readonly agentDirectory: string) {}

  private path(): string {
    return join(this.agentDirectory, "tiber", "authority.json");
  }

  public load(): SettingsResult<AuthorityDocument> {
    const path = this.path();
    if (!existsSync(path)) {
      return { ok: true, value: EMPTY_AUTHORITY };
    }
    try {
      const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
      return parseAuthorityDocument(parsed);
    } catch {
      return ioFailure(`could not read authority settings: ${path}`);
    }
  }

  public save(value: AuthorityDocument): SettingsResult<void> {
    const path = this.path();
    const temporaryPath = `${path}.${String(process.pid)}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      const document = {
        schemaVersion: 1,
        ceilings:
          value.ceilings.minimumAssuranceLevel.kind === "none"
            ? {}
            : {
                minimumAssuranceLevel:
                  value.ceilings.minimumAssuranceLevel.value,
              },
        secretReferences: value.secretReferences,
      };
      writeFileSync(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporaryPath, path);
      return { ok: true, value: undefined };
    } catch {
      cleanup(temporaryPath);
      return ioFailure(`could not write authority settings: ${path}`);
    }
  }
}
