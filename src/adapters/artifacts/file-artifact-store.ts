import { createHash, randomUUID } from "node:crypto";
import {
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

import type {
  ArtifactDigest,
  ArtifactLineNumber,
  ArtifactRangeLimit,
  ArtifactRangeOffset,
  ArtifactRangeText,
  ArtifactReapAtMilliseconds,
  ArtifactSearchMatchText,
  ArtifactSearchMaximumMatches,
  ArtifactSearchQuery,
  ArtifactsReapedCount,
} from "../../core/artifacts/artifact-values.js";
import {
  parseArtifactLineNumber,
  parseArtifactRangeOffset,
  parseArtifactRangeText,
  parseArtifactSearchMatchText,
  parseArtifactsReapedCount,
} from "../../core/artifacts/artifact-values.js";
import type { VirtualizedCommandOutput } from "../../core/artifacts/output-virtualization.js";
import {
  operationalFailure,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";

const DIGEST = /^sha256:([0-9a-f]{64})$/u;

type ArtifactFailureCode =
  | "TIBER_ARTIFACT_CORRUPT"
  | "TIBER_ARTIFACT_DIGEST_INVALID"
  | "TIBER_ARTIFACT_IO"
  | "TIBER_ARTIFACT_NOT_FOUND"
  | "TIBER_ARTIFACT_RANGE_INVALID"
  | "TIBER_ARTIFACT_REAP_FAILED"
  | "TIBER_ARTIFACT_RECEIPT_INVALID"
  | "TIBER_ARTIFACT_SEARCH_INVALID";
type ArtifactFailure = TiberFailure<
  ArtifactFailureCode,
  { readonly domain: "artifact-store" },
  "corrected-input" | "state-change" | "retry-operation"
>;

export type ArtifactResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: ArtifactFailure;
    };

function generated<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok)
    throw new Error("generated artifact store value violated its invariant");
  return result.value;
}

function failure(
  code: ArtifactFailureCode,
  message: string,
): ArtifactResult<never> {
  const retryability =
    code === "TIBER_ARTIFACT_IO" || code === "TIBER_ARTIFACT_REAP_FAILED"
      ? "transient"
      : code === "TIBER_ARTIFACT_NOT_FOUND"
        ? "retry-after-state-change"
        : "retry-after-input";
  return {
    ok: false,
    failure: operationalFailure(code, "artifact-store", message, retryability),
  };
}

export class FileArtifactStore {
  private readonly directory: string;

  public constructor(agentDirectory: string) {
    this.directory = join(agentDirectory, "tiber", "artifacts", "sha256");
  }

  public put(
    result: VirtualizedCommandOutput,
  ): ArtifactResult<Option<ArtifactDigest>> {
    if (result.kind === "inline") return { ok: true, value: none };
    const match = DIGEST.exec(result.digest);
    if (match === null)
      return failure(
        "TIBER_ARTIFACT_DIGEST_INVALID",
        "artifact digest is invalid",
      );
    const hash = match[1];
    if (hash === undefined)
      return failure(
        "TIBER_ARTIFACT_DIGEST_INVALID",
        "artifact digest is invalid",
      );
    const observed = createHash("sha256").update(result.content).digest("hex");
    if (
      observed !== hash ||
      Buffer.byteLength(result.content) !== result.byteLength
    )
      return failure(
        "TIBER_ARTIFACT_RECEIPT_INVALID",
        "artifact content does not match its receipt",
      );
    const path = join(this.directory, `${hash}.txt`);
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(this.directory, { recursive: true, mode: 0o700 });
      writeFileSync(temporary, result.content, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      try {
        renameSync(temporary, path);
      } catch {
        rmSync(temporary, { force: true });
      }
      const receipt = this.read(result.digest);
      return receipt.ok
        ? { ok: true, value: some(result.digest) }
        : failure(receipt.failure.code, receipt.failure.message);
    } catch {
      rmSync(temporary, { force: true });
      return failure(
        "TIBER_ARTIFACT_IO",
        "artifact could not be stored atomically",
      );
    }
  }

  public read(digest: ArtifactDigest): ArtifactResult<Buffer> {
    const match = DIGEST.exec(digest);
    if (match === null)
      return failure(
        "TIBER_ARTIFACT_DIGEST_INVALID",
        "artifact digest is invalid",
      );
    const hash = match[1];
    if (hash === undefined)
      return failure(
        "TIBER_ARTIFACT_DIGEST_INVALID",
        "artifact digest is invalid",
      );
    try {
      const bytes = readFileSync(join(this.directory, `${hash}.txt`));
      if (createHash("sha256").update(bytes).digest("hex") !== hash)
        return failure(
          "TIBER_ARTIFACT_CORRUPT",
          "artifact digest verification failed",
        );
      return { ok: true, value: bytes };
    } catch {
      return failure("TIBER_ARTIFACT_NOT_FOUND", "artifact is unavailable");
    }
  }

  public range(
    digest: ArtifactDigest,
    offset: ArtifactRangeOffset,
    limit: ArtifactRangeLimit,
  ): ArtifactResult<{
    readonly text: ArtifactRangeText;
    readonly offset: ArtifactRangeOffset;
    readonly nextOffset: Option<ArtifactRangeOffset>;
  }> {
    const artifact = this.read(digest);
    if (!artifact.ok) return artifact;
    if (offset > artifact.value.length)
      return failure(
        "TIBER_ARTIFACT_RANGE_INVALID",
        "artifact offset exceeds content length",
      );
    const end = Math.min(artifact.value.length, offset + limit);
    return {
      ok: true,
      value: {
        text: generated(
          parseArtifactRangeText(
            artifact.value.subarray(offset, end).toString("utf8"),
          ),
        ),
        offset,
        nextOffset:
          end < artifact.value.length
            ? some(generated(parseArtifactRangeOffset(end)))
            : none,
      },
    };
  }

  public search(
    digest: ArtifactDigest,
    query: ArtifactSearchQuery,
    maximumMatches: ArtifactSearchMaximumMatches,
  ): ArtifactResult<
    readonly {
      readonly line: ArtifactLineNumber;
      readonly text: ArtifactSearchMatchText;
    }[]
  > {
    const artifact = this.read(digest);
    if (!artifact.ok) return artifact;
    const matches: {
      line: ArtifactLineNumber;
      text: ArtifactSearchMatchText;
    }[] = [];
    const lines = artifact.value.toString("utf8").split("\n");
    for (
      let index = 0;
      index < lines.length && matches.length < maximumMatches;
      index += 1
    ) {
      const line = lines[index] ?? "";
      if (line.includes(query))
        matches.push({
          line: generated(parseArtifactLineNumber(index + 1)),
          text: generated(parseArtifactSearchMatchText(line.slice(0, 1024))),
        });
    }
    return { ok: true, value: matches };
  }

  public reap(
    nowMs: ArtifactReapAtMilliseconds,
  ): ArtifactResult<ArtifactsReapedCount> {
    try {
      mkdirSync(this.directory, { recursive: true, mode: 0o700 });
      const entries = readdirSync(this.directory)
        .filter((name) => /^[0-9a-f]{64}\.txt$/u.test(name))
        .map((name) => ({ name, stat: statSync(join(this.directory, name)) }))
        .sort((left, right) => right.stat.mtimeMs - left.stat.mtimeMs);
      let bytes = 0;
      let removed = 0;
      for (const [index, entry] of entries.entries()) {
        bytes += entry.stat.size;
        if (
          nowMs - entry.stat.mtimeMs > 7 * 24 * 60 * 60 * 1000 ||
          index >= 128 ||
          bytes > 100 * 1024 * 1024
        ) {
          rmSync(join(this.directory, entry.name), { force: true });
          removed += 1;
        }
      }
      return {
        ok: true,
        value: generated(parseArtifactsReapedCount(removed)),
      };
    } catch {
      return failure(
        "TIBER_ARTIFACT_REAP_FAILED",
        "artifact quota reconciliation failed",
      );
    }
  }
}
