import { createHash } from "node:crypto";
import {
  operationalFailure,
  type TiberFailure,
} from "../failures/tiber-failure.js";
import {
  parseCanonicalCommandCatalogJson,
  parseCommandArgument,
  parseCommandCatalogDigest,
  parseCommandEnvironmentName,
  parseCommandEnvironmentValue,
  parseCommandExecutable,
  parseCommandMaximumOutputBytes,
  parseCommandName,
  parseCommandTimeoutMilliseconds,
  type CanonicalCommandCatalogJson,
  type CommandArgument,
  type CommandCatalogDigest,
  type CommandEnvironmentName,
  type CommandEnvironmentValue,
  type CommandExecutable,
  type CommandMaximumOutputBytes,
  type CommandName,
  type CommandTimeoutMilliseconds,
} from "./command-values.js";
import type { ClaimPublicationStatus } from "../tasks/task-values.js";
import type { Option } from "../types/option.js";

export interface StructuredCommand {
  readonly name: CommandName;
  readonly executable: CommandExecutable;
  readonly purpose: "test" | "verification";
  readonly argv: readonly CommandArgument[];
  readonly cwd: "worktree";
  readonly environment: Readonly<
    Record<CommandEnvironmentName, CommandEnvironmentValue>
  >;
  readonly timeoutMs: CommandTimeoutMilliseconds;
  readonly maxOutputBytes: CommandMaximumOutputBytes;
}

export interface CompiledCommandCatalog {
  readonly schemaVersion: 1;
  readonly commands: readonly StructuredCommand[];
  readonly canonicalJson: CanonicalCommandCatalogJson;
  readonly digest: CommandCatalogDigest;
}

export type CommandCatalogResult =
  | { readonly ok: true; readonly value: CompiledCommandCatalog }
  | {
      readonly ok: false;
      readonly failure: TiberFailure<
        "TIBER_COMMAND_CATALOG_INVALID",
        { readonly domain: "command-catalog" },
        "corrected-input" | "state-change" | "retry-operation"
      >;
    };

function invalid(message: string): CommandCatalogResult {
  return {
    ok: false,
    failure: operationalFailure(
      "TIBER_COMMAND_CATALOG_INVALID",
      "command-catalog",
      message,
      "retry-after-input",
    ),
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-object JSON values expose no valid required fields and fail semantic validation; typeof establishes the predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseCommand(value: unknown): StructuredCommand | undefined {
  // Stryker disable next-line ConditionalExpression: required-field validation rejects every non-record JSON value; this guard establishes the semantic record type.
  if (!record(value)) return undefined;
  const name = parseCommandName(value.name);
  const executable = parseCommandExecutable(value.executable);
  const timeout = parseCommandTimeoutMilliseconds(value.timeoutMs);
  const maximumOutput = parseCommandMaximumOutputBytes(value.maxOutputBytes);
  const arguments_ = Array.isArray(value.argv)
    ? value.argv.map(parseCommandArgument)
    : /* Stryker disable next-line ArrayDeclaration: the non-array branch is rejected by the closed shape guard below before this placeholder can escape. */ [];
  const environmentEntries = record(value.environment)
    ? Object.entries(value.environment)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, environmentValue]) => ({
          key: parseCommandEnvironmentName(key),
          value: parseCommandEnvironmentValue(environmentValue),
        }))
    : /* Stryker disable next-line ArrayDeclaration: the non-record branch is rejected by the closed shape guard below before this placeholder can escape. */ [];
  if (
    Object.keys(value).sort().join(",") !==
      "argv,cwd,environment,executable,maxOutputBytes,name,purpose,timeoutMs" ||
    !name.ok ||
    !executable.ok ||
    (value.purpose !== "test" && value.purpose !== "verification") ||
    !Array.isArray(value.argv) ||
    value.argv.length > 64 ||
    arguments_.some((argument) => !argument.ok) ||
    value.cwd !== "worktree" ||
    !record(value.environment) ||
    Object.keys(value.environment).length > 32 ||
    environmentEntries.some((entry) => !entry.key.ok || !entry.value.ok) ||
    !timeout.ok ||
    !maximumOutput.ok
  )
    return undefined;
  const environment: Record<CommandEnvironmentName, CommandEnvironmentValue> =
    {};
  for (const entry of environmentEntries) {
    // Stryker disable next-line ConditionalExpression, LogicalOperator: malformed entries returned above, so both parsers succeeded; this guard conveys the proof without a cast.
    if (entry.key.ok && entry.value.ok)
      environment[entry.key.value] = entry.value.value;
  }
  const argv: CommandArgument[] = [];
  for (const argument of arguments_) {
    // Stryker disable next-line ConditionalExpression: malformed arguments returned above, so every parser succeeded; this guard conveys the proof without a cast.
    if (argument.ok) argv.push(argument.value);
  }
  return {
    name: name.value,
    executable: executable.value,
    purpose: value.purpose,
    argv,
    cwd: "worktree",
    environment,
    timeoutMs: timeout.value,
    maxOutputBytes: maximumOutput.value,
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
  const canonicalJson = parseCanonicalCommandCatalogJson(
    JSON.stringify(definition),
  );
  // Stryker disable next-line ConditionalExpression, BlockStatement: JSON.stringify of the validated catalog is non-empty and always satisfies the canonical JSON parser; this is a defect assertion.
  if (!canonicalJson.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: validated JSON generation makes this defect throw unreachable.
    throw new Error("generated command catalog JSON violated its invariant");
  }
  const digest = parseCommandCatalogDigest(
    `sha256:${createHash("sha256").update(canonicalJson.value).digest("hex")}`,
  );
  // Stryker disable next-line ConditionalExpression, BlockStatement: SHA-256 generation always satisfies the command catalog digest parser; this is a defect assertion.
  if (!digest.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: SHA-256 generation makes this defect throw unreachable.
    throw new Error("generated command catalog digest violated its invariant");
  }
  return {
    ok: true,
    value: {
      ...definition,
      canonicalJson: canonicalJson.value,
      digest: digest.value,
    },
  };
}

export function decideCommandExecution(
  catalog: CompiledCommandCatalog,
  name: CommandName,
  authority: {
    readonly claimStatus: ClaimPublicationStatus;
    readonly grantedCatalogDigest: Option<CommandCatalogDigest>;
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
    authority.claimStatus !== "published" ||
    // Stryker disable next-line ConditionalExpression, StringLiteral: Option.none has no digest value, so exact digest comparison independently rejects absence; the kind check documents the rail.
    authority.grantedCatalogDigest.kind === "none" ||
    authority.grantedCatalogDigest.value !== catalog.digest
  )
    return { ok: false, code: "TIBER_COMMAND_DENIED" };
  return { ok: true, command };
}
