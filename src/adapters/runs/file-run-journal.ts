import { randomUUID } from "node:crypto";
import { mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

export interface RunJournalRecord {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly claimId: string;
  readonly baselineRevision: string;
  readonly workflowDigest: string;
  readonly state: "claim-intent" | "active" | "blocked-baseline-drift";
}

export class FileRunJournal {
  public constructor(private readonly agentDirectory: string) {}

  public write(record: RunJournalRecord): boolean {
    const path = join(
      this.agentDirectory,
      "tiber",
      "runs",
      `${record.taskId}.json`,
    );
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(record, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, path);
      return true;
    } catch {
      try {
        rmSync(temporary, { force: true });
      } catch {
        return false;
      }
      return false;
    }
  }
}
