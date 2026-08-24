import { isAbsolute } from "node:path";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const commandValuePurpose: unique symbol;
type CommandValue<Value, Purpose extends string> = Value & {
  readonly [commandValuePurpose]: Purpose;
};

export type CommandName = CommandValue<string, "command-name">;
export type CommandExecutable = CommandValue<string, "command-executable">;
export type CommandArgument = CommandValue<string, "command-argument">;
export type CommandEnvironmentName = CommandValue<
  string,
  "command-environment-name"
>;
export type CommandEnvironmentValue = CommandValue<
  string,
  "command-environment-value"
>;
export type CanonicalCommandCatalogJson = CommandValue<
  string,
  "canonical-command-catalog-json"
>;
export type CommandTimeoutMilliseconds = CommandValue<
  number,
  "command-timeout-milliseconds"
>;
export type CommandMaximumOutputBytes = CommandValue<
  number,
  "command-maximum-output-bytes"
>;
export type CommandCatalogDigest = CommandValue<
  string,
  "command-catalog-digest"
>;

type CommandValueField =
  | "commandName"
  | "commandExecutable"
  | "commandArgument"
  | "commandEnvironmentName"
  | "commandEnvironmentValue"
  | "canonicalCommandCatalogJson"
  | "commandTimeoutMilliseconds"
  | "commandMaximumOutputBytes"
  | "commandCatalogDigest";
type CommandValueFailure = TiberFailure<
  "TIBER_COMMAND_VALUE_INVALID",
  { readonly field: CommandValueField },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, CommandValueFailure>;

function invalid(field: CommandValueField): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_COMMAND_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function valid<Value, Purpose extends string>(
  value: Value,
): CommandValue<Value, Purpose> {
  return value as CommandValue<Value, Purpose>;
}

export function parseCommandName(value: unknown): Result<CommandName> {
  return typeof value === "string" && /^[a-z][a-z0-9-]{0,63}$/u.test(value)
    ? { ok: true, value: valid<string, "command-name">(value) }
    : invalid("commandName");
}

export const parseCommandArgument = (
  value: unknown,
): Result<CommandArgument> =>
  typeof value === "string" && value.length <= 4_096 && !value.includes("\0")
    ? { ok: true, value: valid<string, "command-argument">(value) }
    : invalid("commandArgument");
export const parseCommandEnvironmentName = (
  value: unknown,
): Result<CommandEnvironmentName> =>
  typeof value === "string" && /^[A-Z_][A-Z0-9_]{0,63}$/u.test(value)
    ? {
        ok: true,
        value: valid<string, "command-environment-name">(value),
      }
    : invalid("commandEnvironmentName");
export const parseCommandEnvironmentValue = (
  value: unknown,
): Result<CommandEnvironmentValue> =>
  typeof value === "string" && value.length <= 4_096 && !value.includes("\0")
    ? {
        ok: true,
        value: valid<string, "command-environment-value">(value),
      }
    : invalid("commandEnvironmentValue");
export const parseCanonicalCommandCatalogJson = (
  value: unknown,
): Result<CanonicalCommandCatalogJson> =>
  typeof value === "string" && value.length > 0
    ? {
        ok: true,
        value: valid<string, "canonical-command-catalog-json">(value),
      }
    : invalid("canonicalCommandCatalogJson");

export function parseCommandExecutable(
  value: unknown,
): Result<CommandExecutable> {
  return typeof value === "string" &&
    // Stryker disable next-line ConditionalExpression, EqualityOperator: isAbsolute independently rejects the empty string; this check documents the executable invariant.
    value.length > 0 &&
    value.length <= 500 &&
    isAbsolute(value) &&
    !value.includes("\0")
    ? { ok: true, value: valid<string, "command-executable">(value) }
    : invalid("commandExecutable");
}

export function parseCommandTimeoutMilliseconds(
  value: unknown,
): Result<CommandTimeoutMilliseconds> {
  // Stryker disable next-line ConditionalExpression, LogicalOperator: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 1 &&
    value <= 3_600_000
    ? {
        ok: true,
        value: valid<number, "command-timeout-milliseconds">(value),
      }
    : invalid("commandTimeoutMilliseconds");
}

export function parseCommandMaximumOutputBytes(
  value: unknown,
): Result<CommandMaximumOutputBytes> {
  // Stryker disable next-line ConditionalExpression, LogicalOperator: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 1 &&
    value <= 1_048_576
    ? {
        ok: true,
        value: valid<number, "command-maximum-output-bytes">(value),
      }
    : invalid("commandMaximumOutputBytes");
}

export function parseCommandCatalogDigest(
  value: unknown,
): Result<CommandCatalogDigest> {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/u.test(value)
    ? { ok: true, value: valid<string, "command-catalog-digest">(value) }
    : invalid("commandCatalogDigest");
}
