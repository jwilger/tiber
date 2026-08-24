import {
  parseArtifactRangeLimit,
  parseArtifactRangeOffset,
  parseArtifactReapAtMilliseconds,
  parseArtifactSearchMaximumMatches,
  parseArtifactSearchQuery,
  parseCommandDurationMilliseconds,
  parseCommandExitCode,
  parseCommandStandardError,
  parseCommandStandardOutput,
  parseInlineOutputMaximumBytes,
} from "../../src/core/artifacts/artifact-values.js";
import type { CommandOutput } from "../../src/core/artifacts/output-virtualization.js";
import { none, some } from "../../src/core/types/option.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid artifact semantic fixture");
  return result.value;
}

export const inlineOutputMaximumBytes = (value: number) =>
  required(parseInlineOutputMaximumBytes(value));
export const artifactReapAtMilliseconds = (value: number) =>
  required(parseArtifactReapAtMilliseconds(value));
export const artifactRangeOffset = (value: number) =>
  required(parseArtifactRangeOffset(value));
export const artifactRangeLimit = (value: number) =>
  required(parseArtifactRangeLimit(value));
export const artifactSearchMaximumMatches = (value: number) =>
  required(parseArtifactSearchMaximumMatches(value));
export const artifactSearchQuery = (value: string) =>
  required(parseArtifactSearchQuery(value));
export const commandExitCode = (value: number) =>
  required(parseCommandExitCode(value));

export function commandOutput(input: {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number | null;
  readonly durationMs: number;
}): CommandOutput {
  return {
    stdout: required(parseCommandStandardOutput(input.stdout)),
    stderr: required(parseCommandStandardError(input.stderr)),
    exitCode:
      input.exitCode === null ? none : some(commandExitCode(input.exitCode)),
    durationMs: required(parseCommandDurationMilliseconds(input.durationMs)),
  };
}
