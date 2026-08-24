import { createHash } from "node:crypto";

export interface CommandOutput {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number | null;
  readonly durationMs: number;
}

export type VirtualizedCommandOutput =
  | {
      readonly kind: "inline";
      readonly output: CommandOutput;
      readonly byteLength: number;
    }
  | {
      readonly kind: "artifact";
      readonly digest: string;
      readonly byteLength: number;
      readonly content: string;
      readonly preview: {
        readonly head: string;
        readonly tail: string;
        readonly omittedBytes: number;
      };
    };

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

export function virtualizeCommandOutput(
  output: CommandOutput,
  maximumInlineBytes: number,
): VirtualizedCommandOutput {
  if (
    !Number.isSafeInteger(maximumInlineBytes) ||
    maximumInlineBytes < 1 ||
    maximumInlineBytes > 1_048_576
  )
    throw new Error("TIBER_OUTPUT_BOUND_INVALID");
  const directBytes =
    Buffer.byteLength(output.stdout) + Buffer.byteLength(output.stderr);
  if (directBytes <= maximumInlineBytes)
    return { kind: "inline", output, byteLength: directBytes };
  const content = `exitCode: ${String(output.exitCode)}\ndurationMs: ${String(output.durationMs)}\n--- stdout ---\n${output.stdout}\n--- stderr ---\n${output.stderr}`;
  // Stryker disable next-line StringLiteral: UTF-8 is Buffer.from's specified default; the explicit encoding documents artifact identity.
  const bytes = Buffer.from(content, "utf8");
  const headBudget = Math.floor(maximumInlineBytes / 2);
  const tailBudget = maximumInlineBytes - headBudget;
  const head = utf8Prefix(bytes, headBudget);
  const tail = utf8Suffix(bytes, tailBudget);
  const shown = Buffer.byteLength(head) + Buffer.byteLength(tail);
  return {
    kind: "artifact",
    digest: `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
    byteLength: bytes.length,
    content,
    preview: {
      head,
      tail,
      omittedBytes: Math.max(0, bytes.length - shown),
    },
  };
}
