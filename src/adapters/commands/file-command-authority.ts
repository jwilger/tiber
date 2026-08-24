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
import { dirname, join } from "node:path";

import {
  compileCommandCatalog,
  type CommandCatalogResult,
} from "../../core/commands/structured-command.js";

const DIGEST = /^sha256:[0-9a-f]{64}$/u;

export class FileCommandAuthority {
  private readonly catalogPath: string;
  private readonly grantPath: string;

  public constructor(repository: string) {
    this.catalogPath = join(repository, ".tiber", "commands.json");
    const common = execFileSync(
      "git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      {
        cwd: repository,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      },
    ).trim();
    this.grantPath = join(common, "tiber", "command-grant.v1.json");
  }

  public loadCatalog(): CommandCatalogResult {
    try {
      const value: unknown = JSON.parse(readFileSync(this.catalogPath, "utf8"));
      return compileCommandCatalog(value);
    } catch {
      return {
        ok: false,
        failure: {
          code: "TIBER_COMMAND_CATALOG_INVALID",
          message: "project command catalog is missing or unreadable",
        },
      };
    }
  }

  public readGrant(): string | undefined {
    if (!existsSync(this.grantPath)) return undefined;
    try {
      const value: unknown = JSON.parse(readFileSync(this.grantPath, "utf8"));
      if (
        typeof value !== "object" ||
        value === null ||
        Array.isArray(value) ||
        !("schemaVersion" in value) ||
        value.schemaVersion !== 1 ||
        !("catalogDigest" in value) ||
        typeof value.catalogDigest !== "string" ||
        !DIGEST.test(value.catalogDigest) ||
        Object.keys(value).sort().join(",") !== "catalogDigest,schemaVersion"
      )
        return undefined;
      return value.catalogDigest;
    } catch {
      return undefined;
    }
  }

  public grant(catalogDigest: string): boolean {
    if (!DIGEST.test(catalogDigest)) return false;
    const temporary = `${this.grantPath}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.grantPath), { recursive: true, mode: 0o700 });
      writeFileSync(
        temporary,
        `${JSON.stringify({ schemaVersion: 1, catalogDigest }, null, 2)}\n`,
        { encoding: "utf8", mode: 0o600, flag: "wx" },
      );
      renameSync(temporary, this.grantPath);
      return this.readGrant() === catalogDigest;
    } catch {
      rmSync(temporary, { force: true });
      return false;
    }
  }
}
