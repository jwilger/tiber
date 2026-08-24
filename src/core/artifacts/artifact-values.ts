import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const artifactValuePurpose: unique symbol;
type ArtifactValue<Value, Purpose extends string> = Value & {
  readonly [artifactValuePurpose]: Purpose;
};

export type CommandStandardOutput = ArtifactValue<
  string,
  "command-standard-output"
>;
export type CommandStandardError = ArtifactValue<
  string,
  "command-standard-error"
>;
export type CommandExitCode = ArtifactValue<number, "command-exit-code">;
export type CommandDurationMilliseconds = ArtifactValue<
  number,
  "command-duration-milliseconds"
>;
export type InlineOutputMaximumBytes = ArtifactValue<
  number,
  "inline-output-maximum-bytes"
>;
export type ArtifactDigest = ArtifactValue<string, "artifact-digest">;
export type ArtifactContent = ArtifactValue<string, "artifact-content">;
export type ArtifactPreviewHead = ArtifactValue<
  string,
  "artifact-preview-head"
>;
export type ArtifactPreviewTail = ArtifactValue<
  string,
  "artifact-preview-tail"
>;
export type ArtifactByteLength = ArtifactValue<number, "artifact-byte-length">;
export type ArtifactOmittedBytes = ArtifactValue<
  number,
  "artifact-omitted-bytes"
>;
export type ArtifactRangeOffset = ArtifactValue<
  number,
  "artifact-range-offset"
>;
export type ArtifactRangeLimit = ArtifactValue<number, "artifact-range-limit">;
export type ArtifactSearchMaximumMatches = ArtifactValue<
  number,
  "artifact-search-maximum-matches"
>;
export type ArtifactSearchQuery = ArtifactValue<
  string,
  "artifact-search-query"
>;
export type ArtifactLineNumber = ArtifactValue<number, "artifact-line-number">;
export type ArtifactReapAtMilliseconds = ArtifactValue<
  number,
  "artifact-reap-at-milliseconds"
>;
export type ArtifactsReapedCount = ArtifactValue<
  number,
  "artifacts-reaped-count"
>;
export type ArtifactRangeText = ArtifactValue<string, "artifact-range-text">;
export type ArtifactSearchMatchText = ArtifactValue<
  string,
  "artifact-search-match-text"
>;

type Field =
  | "commandStandardOutput"
  | "commandStandardError"
  | "commandExitCode"
  | "commandDurationMilliseconds"
  | "inlineOutputMaximumBytes"
  | "artifactDigest"
  | "artifactContent"
  | "artifactPreviewHead"
  | "artifactPreviewTail"
  | "artifactByteLength"
  | "artifactOmittedBytes"
  | "artifactRangeOffset"
  | "artifactRangeLimit"
  | "artifactSearchMaximumMatches"
  | "artifactSearchQuery"
  | "artifactLineNumber"
  | "artifactReapAtMilliseconds"
  | "artifactsReapedCount"
  | "artifactRangeText"
  | "artifactSearchMatchText";
type Failure = TiberFailure<
  "TIBER_ARTIFACT_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_ARTIFACT_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function integer<Purpose extends string>(
  value: unknown,
  field: Field,
  minimum: number,
  maximum: number,
): Result<ArtifactValue<number, Purpose>> {
  // Stryker disable next-line ConditionalExpression: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= minimum &&
    value <= maximum
    ? { ok: true, value: value as ArtifactValue<number, Purpose> }
    : invalid(field);
}

function output<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<ArtifactValue<string, Purpose>> {
  return typeof value === "string"
    ? { ok: true, value: value as ArtifactValue<string, Purpose> }
    : invalid(field);
}

export const parseCommandStandardOutput = (
  value: unknown,
): Result<CommandStandardOutput> => output(value, "commandStandardOutput");
export const parseCommandStandardError = (
  value: unknown,
): Result<CommandStandardError> => output(value, "commandStandardError");
export const parseCommandExitCode = (value: unknown): Result<CommandExitCode> =>
  integer(value, "commandExitCode", 0, 255);
export const parseCommandDurationMilliseconds = (
  value: unknown,
): Result<CommandDurationMilliseconds> =>
  integer(value, "commandDurationMilliseconds", 0, 86_400_000);
export const parseInlineOutputMaximumBytes = (
  value: unknown,
): Result<InlineOutputMaximumBytes> =>
  integer(value, "inlineOutputMaximumBytes", 1, 1_048_576);
export const parseArtifactByteLength = (
  value: unknown,
): Result<ArtifactByteLength> =>
  integer(value, "artifactByteLength", 0, Number.MAX_SAFE_INTEGER);
export const parseArtifactOmittedBytes = (
  value: unknown,
): Result<ArtifactOmittedBytes> =>
  integer(value, "artifactOmittedBytes", 0, Number.MAX_SAFE_INTEGER);
export const parseArtifactRangeOffset = (
  value: unknown,
): Result<ArtifactRangeOffset> =>
  integer(value, "artifactRangeOffset", 0, Number.MAX_SAFE_INTEGER);
export const parseArtifactRangeLimit = (
  value: unknown,
): Result<ArtifactRangeLimit> =>
  integer(value, "artifactRangeLimit", 1, 65_536);
export const parseArtifactSearchMaximumMatches = (
  value: unknown,
): Result<ArtifactSearchMaximumMatches> =>
  integer(value, "artifactSearchMaximumMatches", 1, 100);
export const parseArtifactLineNumber = (
  value: unknown,
): Result<ArtifactLineNumber> =>
  integer(value, "artifactLineNumber", 1, Number.MAX_SAFE_INTEGER);
export const parseArtifactReapAtMilliseconds = (
  value: unknown,
): Result<ArtifactReapAtMilliseconds> =>
  integer(value, "artifactReapAtMilliseconds", 0, Number.MAX_SAFE_INTEGER);
export const parseArtifactsReapedCount = (
  value: unknown,
): Result<ArtifactsReapedCount> =>
  integer(value, "artifactsReapedCount", 0, Number.MAX_SAFE_INTEGER);

export const parseArtifactRangeText = (
  value: unknown,
): Result<ArtifactRangeText> => output(value, "artifactRangeText");
export const parseArtifactSearchMatchText = (
  value: unknown,
): Result<ArtifactSearchMatchText> => output(value, "artifactSearchMatchText");

export function parseArtifactSearchQuery(
  value: unknown,
): Result<ArtifactSearchQuery> {
  return typeof value === "string" && value.length >= 1 && value.length <= 256
    ? { ok: true, value: value as ArtifactSearchQuery }
    : invalid("artifactSearchQuery");
}

export const parseArtifactContent = (value: unknown): Result<ArtifactContent> =>
  output(value, "artifactContent");
export const parseArtifactPreviewHead = (
  value: unknown,
): Result<ArtifactPreviewHead> => output(value, "artifactPreviewHead");
export const parseArtifactPreviewTail = (
  value: unknown,
): Result<ArtifactPreviewTail> => output(value, "artifactPreviewTail");

export function parseArtifactDigest(value: unknown): Result<ArtifactDigest> {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/u.test(value)
    ? {
        ok: true,
        value: value as ArtifactDigest,
      }
    : invalid("artifactDigest");
}
