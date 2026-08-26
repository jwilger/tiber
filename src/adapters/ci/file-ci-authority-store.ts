import { execFileSync } from "node:child_process";
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

import { FileSettingsStore } from "../settings/file-settings-store.js";
import {
  decideCiHoldRecovery,
  type CiSuccessReceipt,
  type RepositoryCiHold,
} from "../../core/ci/ci-authority.js";
import {
  parseCiAuthorityName,
  parseCiDiagnosis,
  parseCiObservationDigest,
  parseCiRevision,
  type CiDiagnosis,
} from "../../core/ci/ci-values.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  parseCiAuthorityCatalog,
  type CiAuthorityCatalog,
} from "./user-local-ci-authority.js";

type CiStoreFailure = TiberFailure<
  "TIBER_CI_STATE_INVALID" | "TIBER_CI_STATE_IO",
  { readonly domain: "ci-state" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type CiStoreResult<T> = TiberResult<T, CiStoreFailure>;

function failure(
  code: CiStoreFailure["code"],
  message: string,
): CiStoreResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "ci-state",
      message,
      code === "TIBER_CI_STATE_IO" ? "transient" : "retry-after-input",
    ),
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class FileCiAuthorityStore {
  private readonly catalogPath: string;
  private readonly legacyCatalogPath: string;
  private readonly holdPath: string;
  private readonly receiptsDirectory: string;

  public constructor(repository: string, agentDirectory: string) {
    this.legacyCatalogPath = join(
      agentDirectory,
      "tiber",
      "ci-authorities.v1.json",
    );
    const settings = new FileSettingsStore(agentDirectory, repository).load();
    this.catalogPath = settings.ok
      ? join(
          agentDirectory,
          "tiber",
          "projects",
          settings.value.projectId,
          "ci-authorities.v1.json",
        )
      : this.legacyCatalogPath;
    const common = execFileSync(
      "git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      {
        cwd: repository,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      },
    ).trim();
    this.holdPath = join(common, "tiber", "ci-hold.v1.json");
    this.receiptsDirectory = join(common, "tiber", "ci-receipts");
  }

  public saveCatalog(catalog: CiAuthorityCatalog): CiStoreResult<void> {
    const parsed = parseCiAuthorityCatalog(catalog);
    if (!parsed.ok)
      return failure(
        "TIBER_CI_STATE_INVALID",
        "CI authority catalog is invalid",
      );
    const temporary = `${this.catalogPath}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.catalogPath), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(parsed.value, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      renameSync(temporary, this.catalogPath);
      return this.loadCatalog().ok
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_CI_STATE_INVALID",
            "CI authority catalog observation is invalid",
          );
    } catch {
      rmSync(temporary, { force: true });
      return failure(
        "TIBER_CI_STATE_IO",
        "CI authority catalog could not be persisted",
      );
    }
  }

  public catalogExists(): boolean {
    return existsSync(this.catalogPath) || existsSync(this.legacyCatalogPath);
  }

  public loadCatalog(): CiStoreResult<CiAuthorityCatalog> {
    const path = existsSync(this.catalogPath)
      ? this.catalogPath
      : this.legacyCatalogPath;
    try {
      const parsed = parseCiAuthorityCatalog(
        JSON.parse(readFileSync(path, "utf8")),
      );
      return parsed.ok
        ? parsed
        : failure(
            "TIBER_CI_STATE_INVALID",
            "user-local CI authority catalog is invalid",
          );
    } catch {
      return failure(
        "TIBER_CI_STATE_IO",
        "user-local CI authority catalog is unavailable",
      );
    }
  }

  public readHold(): CiStoreResult<Option<RepositoryCiHold>> {
    if (!existsSync(this.holdPath)) return { ok: true, value: none };
    try {
      const value: unknown = JSON.parse(readFileSync(this.holdPath, "utf8"));
      if (
        !record(value) ||
        Object.keys(value).sort().join(",") !==
          "failedAuthorities,failedRevision,failureObservationDigest,schemaVersion" ||
        value.schemaVersion !== 1 ||
        !Array.isArray(value.failedAuthorities)
      )
        return failure(
          "TIBER_CI_STATE_INVALID",
          "repository CI hold is invalid",
        );
      const failedRevision = parseCiRevision(value.failedRevision);
      const failedAuthorities =
        value.failedAuthorities.map(parseCiAuthorityName);
      const failureObservationDigest = parseCiObservationDigest(
        value.failureObservationDigest,
      );
      if (
        !failedRevision.ok ||
        failedAuthorities.length === 0 ||
        failedAuthorities.some((authority) => !authority.ok) ||
        new Set(
          failedAuthorities.flatMap((authority) =>
            authority.ok ? [authority.value] : [],
          ),
        ).size !== failedAuthorities.length ||
        !failureObservationDigest.ok
      )
        return failure(
          "TIBER_CI_STATE_INVALID",
          "repository CI hold values are invalid",
        );
      return {
        ok: true,
        value: some({
          failedRevision: failedRevision.value,
          failedAuthorities: failedAuthorities.flatMap((authority) =>
            authority.ok ? [authority.value] : [],
          ),
          failureObservationDigest: failureObservationDigest.value,
        }),
      };
    } catch {
      return failure(
        "TIBER_CI_STATE_IO",
        "repository CI hold could not be read",
      );
    }
  }

  public recordHold(hold: RepositoryCiHold): CiStoreResult<void> {
    const current = this.readHold();
    if (!current.ok) return current;
    if (current.value.kind === "some") {
      const observed = current.value.value;
      return observed.failedRevision === hold.failedRevision &&
        observed.failureObservationDigest === hold.failureObservationDigest &&
        observed.failedAuthorities.length === hold.failedAuthorities.length &&
        observed.failedAuthorities.every(
          (authority, index) => authority === hold.failedAuthorities[index],
        )
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_CI_STATE_INVALID",
            "an unresolved repository CI hold already exists",
          );
    }
    return this.atomicWrite(this.holdPath, {
      schemaVersion: 1,
      ...hold,
    });
  }

  public recordSuccess(receipt: CiSuccessReceipt): CiStoreResult<void> {
    return this.atomicWrite(
      join(this.receiptsDirectory, `${receipt.revision}.json`),
      {
        schemaVersion: 1,
        ...receipt,
      },
    );
  }

  public recoverHold(
    diagnosis: CiDiagnosis,
    receipt: CiSuccessReceipt,
  ): CiStoreResult<void> {
    const observed = this.readHold();
    if (!observed.ok) return observed;
    if (observed.value.kind === "none")
      return failure(
        "TIBER_CI_STATE_INVALID",
        "repository has no CI hold to recover",
      );
    const decision = decideCiHoldRecovery(
      observed.value.value,
      diagnosis,
      receipt,
    );
    if (decision.status !== "recovered")
      return failure("TIBER_CI_STATE_INVALID", decision.code);
    const recoveryPath = join(
      this.receiptsDirectory,
      `${receipt.revision}.recovery.json`,
    );
    const recorded = this.atomicWrite(recoveryPath, {
      schemaVersion: 1,
      hold: observed.value.value,
      diagnosis,
      receipt,
    });
    if (!recorded.ok) return recorded;
    try {
      rmSync(this.holdPath);
      const released = this.readHold();
      return released.ok && released.value.kind === "none"
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_CI_STATE_INVALID",
            "repository CI hold release was not observed",
          );
    } catch {
      return failure(
        "TIBER_CI_STATE_IO",
        "repository CI hold could not be released",
      );
    }
  }

  public parseDiagnosis(input: unknown): CiStoreResult<CiDiagnosis> {
    const diagnosis = parseCiDiagnosis(input);
    return diagnosis.ok
      ? diagnosis
      : failure("TIBER_CI_STATE_INVALID", "CI recovery diagnosis is invalid");
  }

  private atomicWrite(path: string, value: unknown): CiStoreResult<void> {
    if (existsSync(path)) {
      try {
        const observed: unknown = JSON.parse(readFileSync(path, "utf8"));
        return JSON.stringify(observed) === JSON.stringify(value)
          ? { ok: true, value: undefined }
          : failure(
              "TIBER_CI_STATE_INVALID",
              "durable CI state conflicts with an existing receipt",
            );
      } catch {
        return failure(
          "TIBER_CI_STATE_INVALID",
          "existing CI state is invalid",
        );
      }
    }
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      linkSync(temporary, path);
      rmSync(temporary);
      const observed: unknown = JSON.parse(readFileSync(path, "utf8"));
      return JSON.stringify(observed) === JSON.stringify(value)
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_CI_STATE_INVALID",
            "durable CI state observation did not match its intent",
          );
    } catch {
      rmSync(temporary, { force: true });
      return failure("TIBER_CI_STATE_IO", "CI state was not durable");
    }
  }
}
