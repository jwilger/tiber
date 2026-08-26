import { randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";

import type { SetupRepositoryPath } from "../../core/configuration/setup-values.js";
import { canonicalRepositoryDeclarationPath } from "../configuration/repository-declaration-path.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";
import {
  compileWorkflow,
  type CompiledWorkflow,
} from "../../core/workflow/workflow.js";

type WorkflowConfigurationFailure = TiberFailure<
  "TIBER_WORKFLOW_CONFIGURATION_INVALID" | "TIBER_WORKFLOW_CONFIGURATION_IO",
  { readonly domain: "workflow-configuration" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type WorkflowConfigurationResult<Value> = TiberResult<
  Value,
  WorkflowConfigurationFailure
>;

function failure(
  code: WorkflowConfigurationFailure["code"],
  message: string,
): WorkflowConfigurationResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "workflow-configuration",
      message,
      code === "TIBER_WORKFLOW_CONFIGURATION_IO"
        ? "transient"
        : "retry-after-input",
    ),
  };
}

export class FileWorkflowConfiguration {
  private readonly path: string;

  public constructor(private readonly repository: SetupRepositoryPath) {
    this.path = join(repository, ".tiber", "workflow.json");
  }

  public load(): WorkflowConfigurationResult<Option<CompiledWorkflow>> {
    if (!existsSync(this.path)) return { ok: true, value: none };
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "workflow.json",
      "read",
    );
    if (!resolved.ok)
      return failure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_WORKFLOW_CONFIGURATION_IO"
          : "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        "project workflow parent is unavailable",
      );
    const target = resolved.value.value;
    let text: string;
    try {
      text = readFileSync(target, "utf8");
    } catch {
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_IO",
        "project workflow could not be read",
      );
    }
    let input: unknown;
    try {
      input = JSON.parse(text);
    } catch {
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        "project workflow is not valid JSON",
      );
    }
    const compiled = compileWorkflow(input);
    return compiled.ok
      ? { ok: true, value: some(compiled.value) }
      : failure(
          "TIBER_WORKFLOW_CONFIGURATION_INVALID",
          compiled.failure.message,
        );
  }

  public save(workflow: CompiledWorkflow): WorkflowConfigurationResult<void> {
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "workflow.json",
      "write",
    );
    if (!resolved.ok)
      return failure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_WORKFLOW_CONFIGURATION_IO"
          : "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        "project workflow parent is unavailable",
      );
    const target = resolved.value.value;
    const temporary = `${target}.${randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(target), { recursive: true, mode: 0o700 });
      writeFileSync(
        temporary,
        `${JSON.stringify(workflow.definition, null, 2)}\n`,
        { encoding: "utf8", mode: 0o600, flag: "wx" },
      );
      renameSync(temporary, target);
      const observed = (() => {
        try {
          const input: unknown = JSON.parse(readFileSync(target, "utf8"));
          return compileWorkflow(input);
        } catch {
          return undefined;
        }
      })();
      return observed?.ok === true && observed.value.digest === workflow.digest
        ? { ok: true, value: undefined }
        : failure(
            "TIBER_WORKFLOW_CONFIGURATION_INVALID",
            "durable project workflow observation did not match its intent",
          );
    } catch {
      rmSync(temporary, { force: true });
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_IO",
        "project workflow could not be written",
      );
    }
  }

  public useBuiltIn(): WorkflowConfigurationResult<void> {
    const resolved = canonicalRepositoryDeclarationPath(
      this.repository,
      "workflow.json",
      "remove",
    );
    if (!resolved.ok)
      return failure(
        resolved.failure.code === "TIBER_DECLARATION_PATH_IO"
          ? "TIBER_WORKFLOW_CONFIGURATION_IO"
          : "TIBER_WORKFLOW_CONFIGURATION_INVALID",
        resolved.failure.message,
      );
    if (resolved.value.kind === "none")
      return existsSync(this.path)
        ? failure(
            "TIBER_WORKFLOW_CONFIGURATION_INVALID",
            "project workflow parent is unavailable",
          )
        : { ok: true, value: undefined };
    const target = resolved.value.value;
    try {
      rmSync(target, { force: true });
      return existsSync(target)
        ? failure(
            "TIBER_WORKFLOW_CONFIGURATION_INVALID",
            "project workflow removal was not observed",
          )
        : { ok: true, value: undefined };
    } catch {
      return failure(
        "TIBER_WORKFLOW_CONFIGURATION_IO",
        "project workflow could not be removed",
      );
    }
  }
}
