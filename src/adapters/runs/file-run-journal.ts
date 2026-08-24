import { randomUUID } from "node:crypto";
import {
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

export interface RunJournalRecord {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly claimId: string;
  readonly baselineRevision: string;
  readonly workflowDigest: string;
  readonly state:
    | "claim-intent"
    | "active"
    | "blocked-baseline-drift"
    | "blocked-worktree"
    | "red-accepted";
  readonly worktreePath?: string;
  readonly redReceipt?: {
    readonly scenarioName: string;
    readonly testMapping: string;
    readonly diagnosticDigest: string;
    readonly missingPublicSurface: boolean;
  };
}

export class FileRunJournal {
  public constructor(private readonly agentDirectory: string) {}

  public read(taskId: string): RunJournalRecord | undefined {
    if (!/^[0-9a-f-]{36}$/u.test(taskId)) return undefined;
    try {
      const value: unknown = JSON.parse(
        readFileSync(
          join(this.agentDirectory, "tiber", "runs", `${taskId}.json`),
          "utf8",
        ),
      );
      if (
        typeof value !== "object" ||
        value === null ||
        Array.isArray(value) ||
        !("schemaVersion" in value) ||
        value.schemaVersion !== 1 ||
        !("taskId" in value) ||
        value.taskId !== taskId ||
        !("claimId" in value) ||
        typeof value.claimId !== "string" ||
        !("baselineRevision" in value) ||
        typeof value.baselineRevision !== "string" ||
        !("workflowDigest" in value) ||
        typeof value.workflowDigest !== "string" ||
        !("state" in value) ||
        (value.state !== "claim-intent" &&
          value.state !== "active" &&
          value.state !== "blocked-baseline-drift" &&
          value.state !== "blocked-worktree" &&
          value.state !== "red-accepted")
      )
        return undefined;
      const worktreePath =
        "worktreePath" in value && typeof value.worktreePath === "string"
          ? value.worktreePath
          : undefined;
      const red = "redReceipt" in value ? value.redReceipt : undefined;
      const redReceipt =
        typeof red === "object" &&
        red !== null &&
        !Array.isArray(red) &&
        "scenarioName" in red &&
        typeof red.scenarioName === "string" &&
        "testMapping" in red &&
        typeof red.testMapping === "string" &&
        "diagnosticDigest" in red &&
        typeof red.diagnosticDigest === "string" &&
        "missingPublicSurface" in red &&
        typeof red.missingPublicSurface === "boolean"
          ? {
              scenarioName: red.scenarioName,
              testMapping: red.testMapping,
              diagnosticDigest: red.diagnosticDigest,
              missingPublicSurface: red.missingPublicSurface,
            }
          : undefined;
      if (value.state === "red-accepted" && redReceipt === undefined)
        return undefined;
      return {
        schemaVersion: 1,
        taskId,
        claimId: value.claimId,
        baselineRevision: value.baselineRevision,
        workflowDigest: value.workflowDigest,
        state: value.state,
        ...(worktreePath === undefined ? {} : { worktreePath }),
        ...(redReceipt === undefined ? {} : { redReceipt }),
      };
    } catch {
      return undefined;
    }
  }

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
