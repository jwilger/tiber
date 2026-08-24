import { createHash, randomUUID } from "node:crypto";
import { mkdir, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import {
  fail,
  operationalFailure,
  succeed,
  type Result,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";

type ArtifactFailure = TiberFailure<string, unknown, unknown>;
export interface CompactionSourceArtifact {
  readonly digest: string;
  readonly path: string;
  readonly bytes: number;
}

export class FileCompactionArtifactStore {
  readonly #directory: string;
  constructor(agentDirectory: string, repositoryPath: string) {
    const repository = createHash("sha256")
      .update(repositoryPath)
      .digest("hex");
    this.#directory = path.join(
      agentDirectory,
      "tiber",
      "compaction",
      repository,
    );
  }
  async preserve(
    source: string,
  ): Promise<Result<CompactionSourceArtifact, ArtifactFailure>> {
    const digest = createHash("sha256").update(source).digest("hex");
    const destination = path.join(this.#directory, `${digest}.txt`);
    try {
      await mkdir(this.#directory, { recursive: true, mode: 0o700 });
      const temporary = `${destination}.${randomUUID()}.tmp`;
      await writeFile(temporary, source, { encoding: "utf8", mode: 0o600 });
      try {
        await rename(temporary, destination);
      } catch (cause) {
        if ((cause as NodeJS.ErrnoException).code !== "EEXIST") throw cause;
      }
      return succeed({
        digest,
        path: destination,
        bytes: Buffer.byteLength(source, "utf8"),
      });
    } catch {
      return fail(
        operationalFailure(
          "TIBER_COMPACTION_ARTIFACT_IO",
          "compaction-artifact",
          "compaction source artifact could not be preserved",
          "transient",
        ),
      );
    }
  }
}
