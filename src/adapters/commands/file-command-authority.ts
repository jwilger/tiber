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

import { canonicalRepositoryDeclarationPath } from "../configuration/repository-declaration-path.js";

import {
  parseCommandCatalogDigest,
  type CommandCatalogDigest,
} from "../../core/commands/command-values.js";
import {
  compileCommandCatalog,
  type CompiledCommandCatalog,
} from "../../core/commands/structured-command.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";

type CommandCatalogLoadFailure = TiberFailure<
  "TIBER_COMMAND_CATALOG_INVALID" | "TIBER_COMMAND_CATALOG_IO",
  { readonly domain: "command-catalog" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type CommandCatalogLoadResult = TiberResult<
  CompiledCommandCatalog,
  CommandCatalogLoadFailure
>;

type CommandAuthorityFailure = TiberFailure<
  "TIBER_COMMAND_GRANT_INVALID" | "TIBER_COMMAND_GRANT_IO",
  { readonly domain: "command-authority" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type CommandAuthorityResult<Value> = TiberResult<
  Value,
  CommandAuthorityFailure
>;

function catalogFailure(
  code: CommandCatalogLoadFailure["code"],
  message: string,
): CommandCatalogLoadResult {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "command-catalog",
      message,
      code === "TIBER_COMMAND_CATALOG_IO" ? "transient" : "retry-after-input",
    ),
  };
}

function authorityFailure(
  code: CommandAuthorityFailure["code"],
  message: string,
): CommandAuthorityResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "command-authority",
      message,
      code === "TIBER_COMMAND_GRANT_IO" ? "transient" : "retry-after-input",
    ),
  };
}

export class FileCommandAuthority {
  private readonly catalogPath: string;
  private readonly grantPath: string;

  public constructor(private readonly repository: string) {
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

  public loadCatalog(): CommandCatalogLoadResult {
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "commands.json",
      "read",
    );
    if (!resolved.ok)
      return catalogFailure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_COMMAND_CATALOG_IO"
          : "TIBER_COMMAND_CATALOG_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return catalogFailure(
        "TIBER_COMMAND_CATALOG_INVALID",
        "project command catalog is missing",
      );
    let text: string;
    try {
      text = readFileSync(resolved.value.value, "utf8");
    } catch {
      return catalogFailure(
        "TIBER_COMMAND_CATALOG_IO",
        "project command catalog could not be read",
      );
    }
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      return catalogFailure(
        "TIBER_COMMAND_CATALOG_INVALID",
        "project command catalog is not valid JSON",
      );
    }
    return compileCommandCatalog(value);
  }

  public saveCatalog(
    catalog: CompiledCommandCatalog,
  ): CommandAuthorityResult<void> {
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "commands.json",
      "write",
    );
    if (!resolved.ok)
      return authorityFailure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_COMMAND_GRANT_IO"
          : "TIBER_COMMAND_GRANT_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return authorityFailure(
        "TIBER_COMMAND_GRANT_INVALID",
        "command catalog parent is unavailable",
      );
    const target = resolved.value.value;
    const temporary = `${target}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(target), {
        recursive: true,
        mode: 0o700,
      });
      writeFileSync(
        temporary,
        `${JSON.stringify(
          {
            schemaVersion: catalog.schemaVersion,
            commands: catalog.commands,
          },
          null,
          2,
        )}\n`,
        { encoding: "utf8", mode: 0o600, flag: "wx" },
      );
      renameSync(temporary, target);
      const observed = (() => {
        try {
          const value: unknown = JSON.parse(readFileSync(target, "utf8"));
          return compileCommandCatalog(value);
        } catch {
          return undefined;
        }
      })();
      return observed?.ok === true && observed.value.digest === catalog.digest
        ? { ok: true, value: undefined }
        : authorityFailure(
            "TIBER_COMMAND_GRANT_INVALID",
            "durable command catalog observation did not match its intent",
          );
    } catch {
      rmSync(temporary, { force: true });
      return authorityFailure(
        "TIBER_COMMAND_GRANT_IO",
        "command catalog could not be written",
      );
    }
  }

  public removeCatalog(): CommandAuthorityResult<void> {
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "commands.json",
      "remove",
    );
    if (!resolved.ok)
      return authorityFailure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_COMMAND_GRANT_IO"
          : "TIBER_COMMAND_GRANT_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return existsSync(this.catalogPath)
        ? authorityFailure(
            "TIBER_COMMAND_GRANT_INVALID",
            "command catalog parent is unavailable",
          )
        : { ok: true, value: undefined };
    const target = resolved.value.value;
    try {
      rmSync(target, { force: true });
      return existsSync(target)
        ? authorityFailure(
            "TIBER_COMMAND_GRANT_INVALID",
            "command catalog removal was not observed",
          )
        : { ok: true, value: undefined };
    } catch {
      return authorityFailure(
        "TIBER_COMMAND_GRANT_IO",
        "command catalog could not be removed",
      );
    }
  }

  public readGrant(): CommandAuthorityResult<Option<CommandCatalogDigest>> {
    if (!existsSync(this.grantPath)) return { ok: true, value: none };
    try {
      const value: unknown = JSON.parse(readFileSync(this.grantPath, "utf8"));
      if (
        typeof value !== "object" ||
        value === null ||
        Array.isArray(value) ||
        !("schemaVersion" in value) ||
        value.schemaVersion !== 1 ||
        !("catalogDigest" in value) ||
        Object.keys(value).sort().join(",") !== "catalogDigest,schemaVersion"
      )
        return authorityFailure(
          "TIBER_COMMAND_GRANT_INVALID",
          "command grant document is invalid",
        );
      const digest = parseCommandCatalogDigest(value.catalogDigest);
      return digest.ok
        ? { ok: true, value: some(digest.value) }
        : authorityFailure(
            "TIBER_COMMAND_GRANT_INVALID",
            "command grant digest is invalid",
          );
    } catch {
      return authorityFailure(
        "TIBER_COMMAND_GRANT_IO",
        "command grant could not be read",
      );
    }
  }

  public grant(
    catalogDigest: CommandCatalogDigest,
  ): CommandAuthorityResult<void> {
    const temporary = `${this.grantPath}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.grantPath), { recursive: true, mode: 0o700 });
      writeFileSync(
        temporary,
        `${JSON.stringify({ schemaVersion: 1, catalogDigest }, null, 2)}\n`,
        { encoding: "utf8", mode: 0o600, flag: "wx" },
      );
      renameSync(temporary, this.grantPath);
      const observed = this.readGrant();
      return observed.ok &&
        observed.value.kind === "some" &&
        observed.value.value === catalogDigest
        ? { ok: true, value: undefined }
        : authorityFailure(
            "TIBER_COMMAND_GRANT_INVALID",
            "durable command grant observation did not match its intent",
          );
    } catch {
      rmSync(temporary, { force: true });
      return authorityFailure(
        "TIBER_COMMAND_GRANT_IO",
        "command grant was not durable",
      );
    }
  }
}
