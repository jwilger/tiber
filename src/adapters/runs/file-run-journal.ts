import { randomUUID } from "node:crypto";
import {
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import {
  parseCommandCatalogDigest,
  type CommandCatalogDigest,
} from "../../core/commands/command-values.js";
import {
  parseClaimBaselineRevision,
  parseScenarioName,
  parseSpecificationDigest,
  parseTaskClaimId,
  parseTestMappingPath,
  type ClaimBaselineRevision,
  type ScenarioName,
  type SpecificationDigest,
  type TaskClaimId,
  type TaskId,
  type TestMappingPath,
} from "../../core/tasks/task-values.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  parseCompiledWorkflowDigest,
  parseRedDiagnosticDigest,
  type CompiledWorkflowDigest,
  type RedDiagnosticDigest,
} from "../../core/workflow/workflow-values.js";
import {
  parseOwnedWorktreePath,
  type OwnedWorktreePath,
} from "../../core/worktrees/worktree-values.js";

export interface RunJournalRecord {
  readonly schemaVersion: 1;
  readonly taskId: TaskId;
  readonly claimId: TaskClaimId;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly workflowDigest: CompiledWorkflowDigest;
  readonly state:
    | "claim-intent"
    | "active"
    | "blocked-baseline-drift"
    | "blocked-worktree"
    | "red-accepted"
    | "green-review-clean"
    | "green-rework-required"
    | "red-reinstated";
  readonly worktreePath: Option<OwnedWorktreePath>;
  readonly redReceipt: Option<{
    readonly scenarioName: ScenarioName;
    readonly testMapping: TestMappingPath;
    readonly specificationDigest: SpecificationDigest;
    readonly commandCatalogDigest: CommandCatalogDigest;
    readonly diagnosticDigest: RedDiagnosticDigest;
    readonly missingPublicSurface: boolean;
  }>;
}

type RunJournalFailure = TiberFailure<
  "TIBER_RUN_JOURNAL_INVALID" | "TIBER_RUN_JOURNAL_IO",
  { readonly domain: "run-journal" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type RunJournalResult<Value> = TiberResult<Value, RunJournalFailure>;

function failure(
  code: RunJournalFailure["code"],
  message: string,
): RunJournalResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "run-journal",
      message,
      code === "TIBER_RUN_JOURNAL_IO" ? "transient" : "retry-after-input",
    ),
  };
}

function errorCode(error: unknown): Option<string> {
  return typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof error.code === "string"
    ? some(error.code)
    : none;
}

export class FileRunJournal {
  public constructor(private readonly agentDirectory: string) {}

  public read(taskId: TaskId): RunJournalResult<Option<RunJournalRecord>> {
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
        !("baselineRevision" in value) ||
        !("workflowDigest" in value) ||
        !("state" in value) ||
        (value.state !== "claim-intent" &&
          value.state !== "active" &&
          value.state !== "blocked-baseline-drift" &&
          value.state !== "blocked-worktree" &&
          value.state !== "red-accepted" &&
          value.state !== "green-review-clean" &&
          value.state !== "green-rework-required" &&
          value.state !== "red-reinstated")
      )
        return failure(
          "TIBER_RUN_JOURNAL_INVALID",
          "run journal has an invalid document shape",
        );
      const claimId = parseTaskClaimId(value.claimId);
      const baselineRevision = parseClaimBaselineRevision(
        value.baselineRevision,
      );
      const workflowDigest = parseCompiledWorkflowDigest(value.workflowDigest);
      if (!claimId.ok || !baselineRevision.ok || !workflowDigest.ok)
        return failure(
          "TIBER_RUN_JOURNAL_INVALID",
          "run journal authority values are invalid",
        );
      const parsedWorktree =
        "worktreePath" in value
          ? parseOwnedWorktreePath(value.worktreePath)
          : undefined;
      if (parsedWorktree !== undefined && !parsedWorktree.ok)
        return failure(
          "TIBER_RUN_JOURNAL_INVALID",
          "run journal worktree path is invalid",
        );
      const worktreePath =
        parsedWorktree?.ok === true ? some(parsedWorktree.value) : none;
      const red = "redReceipt" in value ? value.redReceipt : undefined;
      const redReceipt =
        typeof red === "object" &&
        red !== null &&
        !Array.isArray(red) &&
        "scenarioName" in red &&
        "testMapping" in red &&
        "specificationDigest" in red &&
        "commandCatalogDigest" in red &&
        "diagnosticDigest" in red &&
        "missingPublicSurface" in red &&
        typeof red.missingPublicSurface === "boolean"
          ? (() => {
              const scenarioName = parseScenarioName(red.scenarioName);
              const testMapping = parseTestMappingPath(red.testMapping);
              const specificationDigest = parseSpecificationDigest(
                red.specificationDigest,
              );
              const commandCatalogDigest = parseCommandCatalogDigest(
                red.commandCatalogDigest,
              );
              const diagnosticDigest = parseRedDiagnosticDigest(
                red.diagnosticDigest,
              );
              return scenarioName.ok &&
                testMapping.ok &&
                specificationDigest.ok &&
                commandCatalogDigest.ok &&
                diagnosticDigest.ok
                ? {
                    scenarioName: scenarioName.value,
                    testMapping: testMapping.value,
                    specificationDigest: specificationDigest.value,
                    commandCatalogDigest: commandCatalogDigest.value,
                    diagnosticDigest: diagnosticDigest.value,
                    missingPublicSurface: red.missingPublicSurface,
                  }
                : undefined;
            })()
          : undefined;
      if (value.state === "red-accepted" && redReceipt === undefined)
        return failure(
          "TIBER_RUN_JOURNAL_INVALID",
          "accepted RED run journal omits a valid receipt",
        );
      return {
        ok: true,
        value: some({
          schemaVersion: 1,
          taskId,
          claimId: claimId.value,
          baselineRevision: baselineRevision.value,
          workflowDigest: workflowDigest.value,
          state: value.state,
          worktreePath,
          redReceipt: redReceipt === undefined ? none : some(redReceipt),
        }),
      };
    } catch (error) {
      const code = errorCode(error);
      return code.kind === "some" && code.value === "ENOENT"
        ? { ok: true, value: none }
        : failure("TIBER_RUN_JOURNAL_IO", "run journal could not be read");
    }
  }

  public write(record: RunJournalRecord): RunJournalResult<void> {
    const path = join(
      this.agentDirectory,
      "tiber",
      "runs",
      `${record.taskId}.json`,
    );
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      const document = {
        schemaVersion: 1,
        taskId: record.taskId,
        claimId: record.claimId,
        baselineRevision: record.baselineRevision,
        workflowDigest: record.workflowDigest,
        state: record.state,
        ...(record.worktreePath.kind === "some"
          ? { worktreePath: record.worktreePath.value }
          : {}),
        ...(record.redReceipt.kind === "some"
          ? { redReceipt: record.redReceipt.value }
          : {}),
      };
      writeFileSync(temporary, `${JSON.stringify(document, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, path);
      return { ok: true, value: undefined };
    } catch {
      try {
        rmSync(temporary, { force: true });
      } catch {
        return failure(
          "TIBER_RUN_JOURNAL_IO",
          "run journal cleanup failed after a write error",
        );
      }
      return failure("TIBER_RUN_JOURNAL_IO", "run journal was not durable");
    }
  }
}
