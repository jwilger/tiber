import { randomUUID } from "node:crypto";
import {
  existsSync,
  linkSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

import {
  parseProjectId,
  type ProjectId,
} from "../../core/configuration/configuration-values.js";
import {
  digestSetupExpectedAuthority,
  digestSetupPlan,
  formatSetupPlan,
  parseSetupPlan,
  sameSetupAuthorityState,
  type SetupPlan,
} from "../../core/configuration/setup.js";
import {
  parseSetupExpectedAuthorityDigest,
  parseSetupPlanDigest,
  parseSetupRepositoryPath,
  type SetupAgentDirectoryPath,
  type SetupExpectedAuthorityDigest,
  type SetupPlanDigest,
  type SetupRepositoryPath,
} from "../../core/configuration/setup-values.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";

export interface PendingSetupApplication {
  readonly expectedCurrent: SetupPlan;
  readonly expectedCurrentDigest: SetupExpectedAuthorityDigest;
  readonly plan: SetupPlan;
  readonly planDigest: SetupPlanDigest;
}

type SetupJournalFailure = TiberFailure<
  | "TIBER_SETUP_JOURNAL_CONFLICT"
  | "TIBER_SETUP_JOURNAL_INVALID"
  | "TIBER_SETUP_JOURNAL_IO",
  { readonly domain: "setup-journal" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type SetupJournalResult<Value> = TiberResult<Value, SetupJournalFailure>;

function failure(
  code: SetupJournalFailure["code"],
  message: string,
): SetupJournalResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "setup-journal",
      message,
      code === "TIBER_SETUP_JOURNAL_IO"
        ? "transient"
        : code === "TIBER_SETUP_JOURNAL_CONFLICT"
          ? "retry-after-state-change"
          : "retry-after-input",
    ),
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class FileSetupJournal {
  private readonly directory: string;
  private readonly pendingPath: string;

  public constructor(
    agentDirectory: SetupAgentDirectoryPath,
    private readonly repository: SetupRepositoryPath,
    private readonly projectId: ProjectId,
  ) {
    this.directory = join(agentDirectory, "tiber", "setup-applications");
    this.pendingPath = join(this.directory, "pending.v1.json");
  }

  public loadPending(): SetupJournalResult<Option<PendingSetupApplication>> {
    if (!existsSync(this.pendingPath)) return { ok: true, value: none };
    let text: string;
    try {
      text = readFileSync(this.pendingPath, "utf8");
    } catch {
      return failure(
        "TIBER_SETUP_JOURNAL_IO",
        "pending setup application could not be read",
      );
    }
    let input: unknown;
    try {
      input = JSON.parse(text);
    } catch {
      return failure(
        "TIBER_SETUP_JOURNAL_INVALID",
        "pending setup application is not valid JSON",
      );
    }
    try {
      if (
        !record(input) ||
        Object.keys(input).sort().join(",") !==
          "expectedCurrent,expectedCurrentDigest,plan,planDigest,projectId,repositoryPath,schemaVersion" ||
        input.schemaVersion !== 1
      )
        return failure(
          "TIBER_SETUP_JOURNAL_INVALID",
          "pending setup application has an invalid shape",
        );
      const projectId = parseProjectId(input.projectId);
      const repositoryPath = parseSetupRepositoryPath(input.repositoryPath);
      const expectedCurrent = parseSetupPlan(input.expectedCurrent);
      const expectedCurrentDigest = parseSetupExpectedAuthorityDigest(
        input.expectedCurrentDigest,
      );
      const plan = parseSetupPlan(input.plan);
      const planDigest = parseSetupPlanDigest(input.planDigest);
      if (
        !projectId.ok ||
        !repositoryPath.ok ||
        !expectedCurrent.ok ||
        !expectedCurrentDigest.ok ||
        !plan.ok ||
        !planDigest.ok ||
        digestSetupExpectedAuthority(expectedCurrent.value) !==
          expectedCurrentDigest.value ||
        digestSetupPlan(plan.value) !== planDigest.value
      )
        return failure(
          "TIBER_SETUP_JOURNAL_INVALID",
          "pending setup application does not match its digest",
        );
      if (
        projectId.value !== this.projectId ||
        repositoryPath.value !== this.repository
      )
        return failure(
          "TIBER_SETUP_JOURNAL_CONFLICT",
          "another repository has an unresolved setup application",
        );
      return {
        ok: true,
        value: some({
          expectedCurrent: expectedCurrent.value,
          expectedCurrentDigest: expectedCurrentDigest.value,
          plan: plan.value,
          planDigest: planDigest.value,
        }),
      };
    } catch {
      return failure(
        "TIBER_SETUP_JOURNAL_INVALID",
        "pending setup application values are invalid",
      );
    }
  }

  public begin(
    expectedCurrent: SetupPlan,
    plan: SetupPlan,
  ): SetupJournalResult<SetupPlanDigest> {
    const expectedCurrentDigest = digestSetupExpectedAuthority(expectedCurrent);
    const planDigest = digestSetupPlan(plan);
    const pending = this.loadPending();
    if (!pending.ok) return pending;
    if (pending.value.kind === "some")
      return pending.value.value.planDigest === planDigest &&
        sameSetupAuthorityState(
          pending.value.value.expectedCurrent,
          expectedCurrent,
        )
        ? { ok: true, value: planDigest }
        : failure(
            "TIBER_SETUP_JOURNAL_CONFLICT",
            "a different confirmed setup application requires recovery",
          );
    const written = this.createIntent({
      schemaVersion: 1,
      projectId: this.projectId,
      repositoryPath: this.repository,
      planDigest,
      expectedCurrentDigest,
      expectedCurrent: formatSetupPlan(expectedCurrent),
      plan: formatSetupPlan(plan),
    });
    if (!written.ok) {
      const raced = this.loadPending();
      return raced.ok &&
        raced.value.kind === "some" &&
        raced.value.value.planDigest === planDigest &&
        sameSetupAuthorityState(
          raced.value.value.expectedCurrent,
          expectedCurrent,
        )
        ? { ok: true, value: planDigest }
        : written;
    }
    const observed = this.loadPending();
    return observed.ok &&
      observed.value.kind === "some" &&
      observed.value.value.planDigest === planDigest &&
      sameSetupAuthorityState(
        observed.value.value.expectedCurrent,
        expectedCurrent,
      )
      ? { ok: true, value: planDigest }
      : failure(
          "TIBER_SETUP_JOURNAL_INVALID",
          "durable setup intent observation did not match its plan",
        );
  }

  public complete(planDigest: SetupPlanDigest): SetupJournalResult<void> {
    const pending = this.loadPending();
    if (!pending.ok) return pending;
    if (
      pending.value.kind === "none" ||
      pending.value.value.planDigest !== planDigest
    )
      return failure(
        "TIBER_SETUP_JOURNAL_CONFLICT",
        "setup receipt does not match the pending application",
      );
    const receipt = this.atomicWrite(
      join(
        this.directory,
        `receipt-${this.projectId}-${planDigest.slice("sha256:".length)}.v1.json`,
      ),
      {
        schemaVersion: 1,
        projectId: this.projectId,
        repositoryPath: this.repository,
        planDigest,
        expectedCurrentDigest: pending.value.value.expectedCurrentDigest,
        expectedCurrent: formatSetupPlan(pending.value.value.expectedCurrent),
        plan: formatSetupPlan(pending.value.value.plan),
        status: "applied",
      },
    );
    if (!receipt.ok) return receipt;
    try {
      rmSync(this.pendingPath);
      const observed = this.loadPending();
      return observed.ok && observed.value.kind === "none"
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_SETUP_JOURNAL_INVALID",
            "completed setup intent release was not observed",
          );
    } catch {
      return failure(
        "TIBER_SETUP_JOURNAL_IO",
        "completed setup intent could not be released",
      );
    }
  }

  private createIntent(value: unknown): SetupJournalResult<void> {
    const temporary = `${this.pendingPath}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.pendingPath), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      linkSync(temporary, this.pendingPath);
      rmSync(temporary);
      const observed: unknown = JSON.parse(
        readFileSync(this.pendingPath, "utf8"),
      );
      return JSON.stringify(observed) === JSON.stringify(value)
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_SETUP_JOURNAL_INVALID",
            "durable setup intent did not match its plan",
          );
    } catch {
      rmSync(temporary, { force: true });
      return existsSync(this.pendingPath)
        ? failure(
            "TIBER_SETUP_JOURNAL_CONFLICT",
            "a concurrent setup application recorded a different intent",
          )
        : failure(
            "TIBER_SETUP_JOURNAL_IO",
            "setup application intent was not durable",
          );
    }
  }

  private atomicWrite(path: string, value: unknown): SetupJournalResult<void> {
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, path);
      const observed: unknown = JSON.parse(readFileSync(path, "utf8"));
      return JSON.stringify(observed) === JSON.stringify(value)
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_SETUP_JOURNAL_INVALID",
            "durable setup application state did not match its intent",
          );
    } catch {
      rmSync(temporary, { force: true });
      return failure(
        "TIBER_SETUP_JOURNAL_IO",
        "setup application state was not durable",
      );
    }
  }
}
