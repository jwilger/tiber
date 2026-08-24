import { createHash } from "node:crypto";

import type { Option } from "../types/option.js";
import {
  parseArtifactByteLength,
  parseArtifactContent,
  parseArtifactDigest,
  parseArtifactOmittedBytes,
  parseArtifactPreviewHead,
  parseArtifactPreviewTail,
  type ArtifactByteLength,
  type ArtifactContent,
  type ArtifactDigest,
  type ArtifactOmittedBytes,
  type ArtifactPreviewHead,
  type ArtifactPreviewTail,
  type CommandDurationMilliseconds,
  type CommandExitCode,
  type CommandStandardError,
  type CommandStandardOutput,
  type InlineOutputMaximumBytes,
} from "./artifact-values.js";

export interface CommandOutput {
  readonly stdout: CommandStandardOutput;
  readonly stderr: CommandStandardError;
  readonly exitCode: Option<CommandExitCode>;
  readonly durationMs: CommandDurationMilliseconds;
}

export interface VirtualizedTextArtifact {
  readonly kind: "artifact";
  readonly digest: ArtifactDigest;
  readonly byteLength: ArtifactByteLength;
  readonly content: ArtifactContent;
  readonly preview: {
    readonly head: ArtifactPreviewHead;
    readonly tail: ArtifactPreviewTail;
    readonly omittedBytes: ArtifactOmittedBytes;
  };
}

export type VirtualizedTextOutput =
  | {
      readonly kind: "inline";
      readonly text: string;
      readonly byteLength: ArtifactByteLength;
    }
  | VirtualizedTextArtifact;

export type VirtualizedCommandOutput =
  | {
      readonly kind: "inline";
      readonly output: CommandOutput;
      readonly byteLength: ArtifactByteLength;
    }
  | VirtualizedTextArtifact;

function generated<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  // Stryker disable next-line ConditionalExpression, BlockStatement: callers pass only values derived from bounded generated lengths/content; parser rejection is an internal defect.
  if (!result.ok) {
    // Stryker disable next-line StringLiteral, CallExpression: generated bounded values make this defect throw unreachable.
    throw new Error("generated artifact value violated its invariant");
  }
  return result.value;
}

function utf8Prefix(buffer: Buffer, maximumBytes: number): string {
  return buffer
    .subarray(0, Math.min(maximumBytes, buffer.length))
    .toString("utf8")
    .replace(/�$/u, "");
}

function utf8Suffix(buffer: Buffer, maximumBytes: number): string {
  return buffer
    .subarray(Math.max(0, buffer.length - maximumBytes))
    .toString("utf8")
    .replace(/^�+/u, "");
}

export function virtualizeTextOutput(
  text: string,
  maximumInlineBytes: InlineOutputMaximumBytes,
): VirtualizedTextOutput {
  // Stryker disable next-line StringLiteral: Node treats an empty encoding as UTF-8; the explicit encoding documents artifact identity.
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= maximumInlineBytes)
    return {
      kind: "inline",
      text,
      byteLength: generated(parseArtifactByteLength(bytes.length)),
    };
  const headBudget = Math.floor(maximumInlineBytes / 2);
  const tailBudget = maximumInlineBytes - headBudget;
  const head = utf8Prefix(bytes, headBudget);
  const tail = utf8Suffix(bytes, tailBudget);
  const shown = Buffer.byteLength(head) + Buffer.byteLength(tail);
  return {
    kind: "artifact",
    digest: generated(
      parseArtifactDigest(
        `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
      ),
    ),
    byteLength: generated(parseArtifactByteLength(bytes.length)),
    content: generated(parseArtifactContent(text)),
    preview: {
      head: generated(parseArtifactPreviewHead(head)),
      tail: generated(parseArtifactPreviewTail(tail)),
      omittedBytes: generated(parseArtifactOmittedBytes(bytes.length - shown)),
    },
  };
}

export function virtualizeCommandOutput(
  output: CommandOutput,
  maximumInlineBytes: InlineOutputMaximumBytes,
): VirtualizedCommandOutput {
  const directBytes =
    Buffer.byteLength(output.stdout) + Buffer.byteLength(output.stderr);
  if (directBytes <= maximumInlineBytes)
    return {
      kind: "inline",
      output,
      byteLength: generated(parseArtifactByteLength(directBytes)),
    };
  const exitCode =
    output.exitCode.kind === "some" ? String(output.exitCode.value) : "signal";
  const content = `exitCode: ${exitCode}\ndurationMs: ${String(output.durationMs)}\n--- stdout ---\n${output.stdout}\n--- stderr ---\n${output.stderr}`;
  // Stryker disable next-line StringLiteral: UTF-8 is Buffer.from's specified default; the explicit encoding documents artifact identity.
  const bytes = Buffer.from(content, "utf8");
  const headBudget = Math.floor(maximumInlineBytes / 2);
  const tailBudget = maximumInlineBytes - headBudget;
  const head = utf8Prefix(bytes, headBudget);
  const tail = utf8Suffix(bytes, tailBudget);
  const shown = Buffer.byteLength(head) + Buffer.byteLength(tail);
  return {
    kind: "artifact",
    digest: generated(
      parseArtifactDigest(
        `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
      ),
    ),
    byteLength: generated(parseArtifactByteLength(bytes.length)),
    content: generated(parseArtifactContent(content)),
    preview: {
      head: generated(parseArtifactPreviewHead(head)),
      tail: generated(parseArtifactPreviewTail(tail)),
      omittedBytes: generated(
        parseArtifactOmittedBytes(Math.max(0, bytes.length - shown)),
      ),
    },
  };
}
