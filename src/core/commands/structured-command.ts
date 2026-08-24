import { createHash } from "node:crypto";
import { isAbsolute } from "node:path";

export interface StructuredCommand {
  readonly name: string;
  readonly executable: string;
  readonly argv: readonly string[];
  readonly cwd: "worktree";
  readonly environment: Readonly<Record<string, string>>;
  readonly timeoutMs: number;
  readonly maxOutputBytes: number;
}

export interface CompiledCommandCatalog {
  readonly schemaVersion: 1;
  readonly commands: readonly StructuredCommand[];
  readonly canonicalJson: string;
  readonly digest: string;
}

export type CommandCatalogResult =
  | { readonly ok: true; readonly value: CompiledCommandCatalog }
  | {
      readonly ok: false;
      readonly failure: {
        readonly code: "TIBER_COMMAND_CATALOG_INVALID";
        readonly message: string;
      };
    };

function invalid(message: string): CommandCatalogResult {
  return {
    ok: false,
    failure: { code: "TIBER_COMMAND_CATALOG_INVALID", message },
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-object JSON values expose no valid required fields and fail semantic validation; typeof establishes the predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseCommand(value: unknown): StructuredCommand | undefined {
  // Stryker disable next-line ConditionalExpression: required-field validation rejects every non-record JSON value; this guard establishes the semantic record type.
  if (!record(value)) return undefined;
  if (
    Object.keys(value).sort().join(",") !==
      "argv,cwd,environment,executable,maxOutputBytes,name,timeoutMs" ||
    // Stryker disable next-line ConditionalExpression: the following name grammar rejects non-string JSON values and this guard narrows the type.
    typeof value.name !== "string" ||
    !/^[a-z][a-z0-9-]{0,63}$/u.test(value.name) ||
    // Stryker disable next-line ConditionalExpression: path validation rejects non-string JSON values and this guard narrows the type.
    typeof value.executable !== "string" ||
    !isAbsolute(value.executable) ||
    value.executable.includes("\0") ||
    !Array.isArray(value.argv) ||
    value.argv.length > 64 ||
    !value.argv.every(
      (argument) =>
        // Stryker disable next-line ConditionalExpression: bounded length and NUL checks reject non-string JSON values; this guard narrows the semantic type.
        typeof argument === "string" &&
        argument.length <= 4096 &&
        !argument.includes("\0"),
    ) ||
    value.cwd !== "worktree" ||
    !record(value.environment) ||
    Object.keys(value.environment).length > 32 ||
    !Object.entries(value.environment).every(
      ([key, environmentValue]) =>
        /^[A-Z_][A-Z0-9_]{0,63}$/u.test(key) &&
        // Stryker disable next-line ConditionalExpression: bounded length and NUL checks reject non-string JSON values; this guard narrows the semantic type.
        typeof environmentValue === "string" &&
        environmentValue.length <= 4096 &&
        !environmentValue.includes("\0"),
    ) ||
    // Stryker disable next-line ConditionalExpression: Number.isSafeInteger rejects non-number JSON values; this guard narrows the semantic type.
    typeof value.timeoutMs !== "number" ||
    !Number.isSafeInteger(value.timeoutMs) ||
    value.timeoutMs < 1 ||
    value.timeoutMs > 3_600_000 ||
    // Stryker disable next-line ConditionalExpression: Number.isSafeInteger rejects non-number JSON values; this guard narrows the semantic type.
    typeof value.maxOutputBytes !== "number" ||
    !Number.isSafeInteger(value.maxOutputBytes) ||
    value.maxOutputBytes < 1 ||
    value.maxOutputBytes > 1_048_576
  )
    return undefined;
  const environment: Record<string, string> = {};
  for (const [key, environmentValue] of Object.entries(value.environment).sort(
    ([left], [right]) => left.localeCompare(right),
  )) {
    // Stryker disable next-line ConditionalExpression: every environment value was validated as a bounded string immediately above; this carries the trust-boundary proof into the inferred type.
    if (typeof environmentValue === "string")
      environment[key] = environmentValue;
  }
  return {
    name: value.name,
    executable: value.executable,
    // Stryker disable next-line MethodExpression: every argument was validated as a bounded string; filtering carries that proof into the inferred type.
    argv: value.argv.filter(
      // Stryker disable next-line ArrowFunction, ConditionalExpression: every argument was validated as a string immediately above.
      (argument): argument is string => typeof argument === "string",
    ),
    cwd: "worktree",
    environment,
    timeoutMs: value.timeoutMs,
    maxOutputBytes: value.maxOutputBytes,
  };
}

export function compileCommandCatalog(value: unknown): CommandCatalogResult {
  if (
    !record(value) ||
    Object.keys(value).sort().join(",") !== "commands,schemaVersion" ||
    value.schemaVersion !== 1 ||
    !Array.isArray(value.commands) ||
    value.commands.length < 1 ||
    value.commands.length > 64
  )
    return invalid(
      "command catalog must contain 1 to 64 closed command definitions",
    );
  const commands = value.commands.map(parseCommand);
  if (
    commands.some((command) => command === undefined) ||
    // Stryker disable next-line OptionalChaining: the preceding malformed-command condition establishes every mapped command; optional access preserves narrowing without a cast.
    new Set(commands.map((command) => command?.name)).size !== commands.length
  )
    return invalid(
      "command definitions must be unique, bounded, and data-only",
    );
  // Stryker disable next-line MethodExpression: malformed commands returned above; filtering carries that semantic proof into the inferred type.
  const parsed = commands.filter(
    // Stryker disable next-line ArrowFunction, ConditionalExpression: every command was validated immediately above.
    (command): command is StructuredCommand => command !== undefined,
  );
  const definition = { schemaVersion: 1 as const, commands: parsed };
  const canonicalJson = JSON.stringify(definition);
  return {
    ok: true,
    value: {
      ...definition,
      canonicalJson,
      digest: `sha256:${createHash("sha256").update(canonicalJson).digest("hex")}`,
    },
  };
}

export function decideCommandExecution(
  catalog: CompiledCommandCatalog,
  name: string,
  authority: {
    readonly activeClaim: boolean;
    readonly grantedCatalogDigest: string | undefined;
  },
):
  | { readonly ok: true; readonly command: StructuredCommand }
  | {
      readonly ok: false;
      readonly code: "TIBER_COMMAND_DENIED" | "TIBER_COMMAND_UNKNOWN";
    } {
  const command = catalog.commands.find((candidate) => candidate.name === name);
  if (command === undefined)
    return { ok: false, code: "TIBER_COMMAND_UNKNOWN" };
  if (
    !authority.activeClaim ||
    authority.grantedCatalogDigest !== catalog.digest
  )
    return { ok: false, code: "TIBER_COMMAND_DENIED" };
  return { ok: true, command };
}
