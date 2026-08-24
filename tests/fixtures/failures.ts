import type { SettingsFailure } from "../../src/core/configuration/settings.js";
import type {
  TaskBoardFailure,
  TaskBoardFailureReason,
} from "../../src/core/tasks/task-board.js";

export function expectedSemanticFailure<
  Code extends string,
  Field extends string,
>(code: Code, field: Field) {
  return {
    code,
    message: `Invalid ${field}`,
    safeContext: { field },
    causes: [],
    retryability: "retry-after-input" as const,
    requiredRecoveryEvidence: ["corrected-value"] as const,
    redaction: "public" as const,
  };
}

export function expectedOperationalFailure<
  Code extends string,
  Domain extends string,
>(
  code: Code,
  domain: Domain,
  message: string,
  retryability:
    | "not-retryable"
    | "retry-after-input"
    | "retry-after-state-change"
    | "transient",
) {
  const evidence =
    retryability === "retry-after-input"
      ? "corrected-input"
      : retryability === "retry-after-state-change"
        ? "state-change"
        : retryability === "transient"
          ? "retry-operation"
          : undefined;
  return {
    code,
    message,
    safeContext: { domain },
    causes: [],
    retryability,
    requiredRecoveryEvidence: evidence === undefined ? [] : [evidence],
    redaction: "public" as const,
  };
}

export function expectedSpecificationParseFailure() {
  return {
    code: "TIBER_SPECIFICATION_INVALID" as const,
    message: "Task specification is malformed or incomplete",
    safeContext: { boundary: "task-specification" as const },
    causes: [],
    retryability: "retry-after-input" as const,
    requiredRecoveryEvidence: ["corrected-specification"] as const,
    redaction: "public" as const,
  };
}

export function expectedTaskEventParseFailure(message: string) {
  return {
    code: "TIBER_TASK_EVENT_INVALID" as const,
    message,
    safeContext: { boundary: "task-event" as const },
    causes: [],
    retryability: "retry-after-input" as const,
    requiredRecoveryEvidence: ["corrected-task-event"] as const,
    redaction: "public" as const,
  };
}

export function expectedTaskBoardFailure(
  reason: TaskBoardFailureReason,
  message: string,
): TaskBoardFailure {
  return {
    code: "TIBER_TASK_BOARD_INVALID",
    message,
    safeContext: { domain: "task-board", reason },
    causes: [],
    retryability: "retry-after-state-change",
    requiredRecoveryEvidence: ["corrected-task-history"],
    redaction: "public",
  };
}

export function expectedSettingsFailure(
  code: SettingsFailure["code"],
  message: string,
): SettingsFailure {
  const io = code === "TIBER_SETTINGS_IO";
  const repository = code === "TIBER_SETTINGS_REPOSITORY_REQUIRED";
  return {
    code,
    message,
    safeContext: { domain: "settings" },
    causes: [],
    retryability: io
      ? "transient"
      : repository
        ? "retry-after-state-change"
        : "retry-after-input",
    requiredRecoveryEvidence: io
      ? ["retry-operation"]
      : repository
        ? ["repository-required"]
        : ["corrected-settings"],
    redaction: "public",
  };
}
