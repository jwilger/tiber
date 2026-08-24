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

import type { VirtualizedCommandOutput } from "../../core/artifacts/output-virtualization.js";

const DIGEST = /^sha256:([0-9a-f]{64})$/u;

export type ArtifactResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly failure: { readonly code: string; readonly message: string };
    };

function failure(code: string, message: string): ArtifactResult<never> {
  return { ok: false, failure: { code, message } };
}

export class FileArtifactStore {
  private readonly directory: string;

  public constructor(agentDirectory: string) {
    this.directory = join(agentDirectory, "tiber", "artifacts", "sha256");
  }

  public put(
    result: VirtualizedCommandOutput,
  ): ArtifactResult<string | undefined> {
    if (result.kind === "inline") return { ok: true, value: undefined };
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
        ? { ok: true, value: result.digest }
        : failure(receipt.failure.code, receipt.failure.message);
    } catch {
      rmSync(temporary, { force: true });
      return failure(
        "TIBER_ARTIFACT_IO",
        "artifact could not be stored atomically",
      );
    }
  }

  public read(digest: string): ArtifactResult<Buffer> {
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
    digest: string,
    offset: number,
    limit: number,
  ): ArtifactResult<{
    readonly text: string;
    readonly offset: number;
    readonly nextOffset?: number;
  }> {
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      !Number.isSafeInteger(limit) ||
      limit < 1 ||
      limit > 65_536
    )
      return failure(
        "TIBER_ARTIFACT_RANGE_INVALID",
        "artifact range is outside bounded limits",
      );
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
        text: artifact.value.subarray(offset, end).toString("utf8"),
        offset,
        ...(end < artifact.value.length ? { nextOffset: end } : {}),
      },
    };
  }

  public search(
    digest: string,
    query: string,
    maximumMatches: number,
  ): ArtifactResult<
    readonly { readonly line: number; readonly text: string }[]
  > {
    if (
      query.length < 1 ||
      query.length > 256 ||
      !Number.isSafeInteger(maximumMatches) ||
      maximumMatches < 1 ||
      maximumMatches > 100
    )
      return failure(
        "TIBER_ARTIFACT_SEARCH_INVALID",
        "artifact search bounds are invalid",
      );
    const artifact = this.read(digest);
    if (!artifact.ok) return artifact;
    const matches: { line: number; text: string }[] = [];
    const lines = artifact.value.toString("utf8").split("\n");
    for (
      let index = 0;
      index < lines.length && matches.length < maximumMatches;
      index += 1
    ) {
      const line = lines[index] ?? "";
      if (line.includes(query))
        matches.push({ line: index + 1, text: line.slice(0, 1024) });
    }
    return { ok: true, value: matches };
  }

  public reap(nowMs: number): ArtifactResult<number> {
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
      return { ok: true, value: removed };
    } catch {
      return failure(
        "TIBER_ARTIFACT_REAP_FAILED",
        "artifact quota reconciliation failed",
      );
    }
  }
}
