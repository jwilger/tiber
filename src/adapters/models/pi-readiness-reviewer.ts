import {
  createAgentSession,
  createExtensionRuntime,
  getAgentDir,
  SessionManager,
  type ResourceLoader,
} from "@earendil-works/pi-coding-agent";

import type { TiberResult } from "../../core/failures/tiber-failure.js";
import type {
  ReadinessReview,
  TaskSpecification,
} from "../../core/tasks/readiness.js";
import {
  parseSpecificationReviewFindingCount,
  type SpecificationDigest,
} from "../../core/tasks/task-values.js";
import {
  modelReviewFailure,
  type ModelReviewFailure,
} from "./model-review-failure.js";

export interface ReadinessReviewResult {
  readonly review: ReadinessReview;
  readonly findings: readonly string[];
}

export interface ReadinessReviewExecution {
  readonly signal?: AbortSignal;
  readonly onProgress?: (message: string) => void;
}

function reportProgress(
  execution: ReadinessReviewExecution,
  message: string,
): boolean {
  try {
    execution.onProgress?.(message);
    return true;
  } catch {
    return false;
  }
}

function hasUnsafeFormatting(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value);
}

const READINESS_REVIEW_TIME_BUDGET_MS = 5 * 60_000;

const READINESS_REVIEWER_SYSTEM_PROMPT = [
  "ROLE: Tiber specification readiness reviewer v1.",
  "Review independently with fresh context. Do not authorize effects.",
  "Assess outcome, Gherkin scenarios, edge cases, exclusions, dependencies, test mappings, and architecture implications.",
  "Every finding must be a concise actionable correction, not merely a category or count.",
  'Return only JSON matching exactly: {"findingCount":<0-20>,"findings":["actionable finding"]}. The count must equal the array length; use an empty array when clean.',
].join("\n");

function readinessReviewerResources(): ResourceLoader {
  const extensions = {
    extensions: [],
    errors: [],
    runtime: createExtensionRuntime(),
  };
  return {
    getExtensions: () => extensions,
    getSkills: () => ({ skills: [], diagnostics: [] }),
    getPrompts: () => ({ prompts: [], diagnostics: [] }),
    getThemes: () => ({ themes: [], diagnostics: [] }),
    getAgentsFiles: () => ({ agentsFiles: [] }),
    getSystemPrompt: () => READINESS_REVIEWER_SYSTEM_PROMPT,
    getSystemPromptSource: () => undefined,
    getAppendSystemPrompt: () => [],
    getAppendSystemPromptSources: () => [],
    extendResources: () => undefined,
    reload: () => Promise.resolve(),
  };
}

export function parseReadinessReviewOutput(
  text: string,
  digest: SpecificationDigest,
): TiberResult<ReadinessReviewResult, ModelReviewFailure> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review output is not valid JSON",
      ),
    };
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(",") !== "findingCount,findings"
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review output has an invalid shape",
      ),
    };
  if (!("findingCount" in value) || !("findings" in value))
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review output omits findingCount",
      ),
    };
  const findingCount = parseSpecificationReviewFindingCount(value.findingCount);
  const findingValues: readonly unknown[] | undefined = Array.isArray(
    value.findings,
  )
    ? Array.from<unknown>(value.findings)
    : undefined;
  const findings = findingValues?.map((finding) =>
    typeof finding === "string" ? finding.trim() : finding,
  );
  if (
    !findingCount.ok ||
    findings?.length !== findingCount.value ||
    findings.length > 20 ||
    findings.some(
      (finding) =>
        typeof finding !== "string" ||
        finding.length === 0 ||
        finding.length > 500 ||
        hasUnsafeFormatting(finding),
    )
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review findings are invalid",
      ),
    };
  return {
    ok: true,
    value: {
      review: {
        contextFreshness: "fresh",
        reviewerRole: "specification-reviewer",
        findingCount: findingCount.value,
        reviewedSpecificationDigest: digest,
      },
      findings: findings.filter(
        (finding): finding is string => typeof finding === "string",
      ),
    },
  };
}

async function conductSpecificationReview(
  cwd: string,
  specification: TaskSpecification,
  digest: SpecificationDigest,
  execution: ReadinessReviewExecution,
): Promise<TiberResult<ReadinessReviewResult, ModelReviewFailure>> {
  if (!reportProgress(execution, "Starting independent readiness review"))
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "readiness review progress reporting failed",
      ),
    };
  const agentDir = getAgentDir();
  const resourceLoader = readinessReviewerResources();
  const { session } = await createAgentSession({
    cwd,
    agentDir,
    noTools: "all",
    resourceLoader,
    sessionManager: SessionManager.inMemory(cwd),
    thinkingLevel: "medium",
  });
  type TerminationReason =
    "cancelled" | "output-budget" | "progress-failure" | "timed-out";
  let terminationReason: TerminationReason | undefined;
  let signalTermination: ((reason: TerminationReason) => void) | undefined;
  const termination = new Promise<TerminationReason>((resolve) => {
    signalTermination = resolve;
  });
  const abort = (reason: TerminationReason) => {
    if (terminationReason !== undefined) return;
    terminationReason = reason;
    signalTermination?.(reason);
    void session.abort().catch(() => undefined);
  };
  const failedTermination = (
    reason: TerminationReason,
  ): TiberResult<ReadinessReviewResult, ModelReviewFailure> => {
    const terminationFailure = {
      cancelled: ["TIBER_REVIEW_CANCELLED", "readiness review was cancelled"],
      "output-budget": [
        "TIBER_REVIEW_BUDGET_EXCEEDED",
        "readiness review exceeded its output budget",
      ],
      "progress-failure": [
        "TIBER_REVIEW_EXECUTION_FAILED",
        "readiness review progress reporting failed",
      ],
      "timed-out": [
        "TIBER_REVIEW_TIMED_OUT",
        "readiness review exceeded its time budget",
      ],
    } as const;
    const [code, message] = terminationFailure[reason];
    return { ok: false, failure: modelReviewFailure(code, message) };
  };
  const unsubscribe = session.subscribe((event) => {
    if (
      event.type === "message_update" &&
      event.message.role === "assistant" &&
      event.message.usage.output > 4096
    )
      abort("output-budget");
  });
  const cancel = () => {
    abort("cancelled");
  };
  execution.signal?.addEventListener("abort", cancel, { once: true });
  if (execution.signal?.aborted) cancel();
  const assignment = [
    `SPECIFICATION_DIGEST: ${digest}`,
    `SPECIFICATION: ${JSON.stringify(specification)}`,
  ].join("\n");
  const timeout = setTimeout(() => {
    abort("timed-out");
  }, READINESS_REVIEW_TIME_BUDGET_MS);
  const heartbeat = setInterval(() => {
    if (
      !reportProgress(
        execution,
        "Independent readiness review is still running",
      )
    )
      abort("progress-failure");
  }, 250);
  heartbeat.unref();
  try {
    if (!reportProgress(execution, "Independent readiness reviewer is working"))
      return {
        ok: false,
        failure: modelReviewFailure(
          "TIBER_REVIEW_EXECUTION_FAILED",
          "readiness review progress reporting failed",
        ),
      };
    if (terminationReason !== undefined)
      return failedTermination(terminationReason);
    const prompt = session
      .prompt(assignment, { expandPromptTemplates: false })
      .then(() => undefined);
    const terminated = await Promise.race([prompt, termination]);
    if (terminated !== undefined) return failedTermination(terminated);
    let text: string | undefined;
    for (let index = session.messages.length - 1; index >= 0; index -= 1) {
      const message = session.messages[index];
      if (message?.role !== "assistant") continue;
      const content = message.content.find((part) => part.type === "text");
      if (content?.type === "text") {
        text = content.text;
        break;
      }
    }
    return text === undefined
      ? {
          ok: false,
          failure: modelReviewFailure(
            "TIBER_REVIEW_RESPONSE_MISSING",
            "readiness reviewer returned no text response",
          ),
        }
      : parseReadinessReviewOutput(text, digest);
  } finally {
    clearTimeout(timeout);
    clearInterval(heartbeat);
    execution.signal?.removeEventListener("abort", cancel);
    unsubscribe();
    session.dispose();
  }
}

export async function reviewSpecification(
  cwd: string,
  specification: TaskSpecification,
  digest: SpecificationDigest,
  execution: ReadinessReviewExecution = {},
): Promise<TiberResult<ReadinessReviewResult, ModelReviewFailure>> {
  if (execution.signal?.aborted)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_CANCELLED",
        "readiness review was cancelled",
      ),
    };
  try {
    return await conductSpecificationReview(
      cwd,
      specification,
      digest,
      execution,
    );
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "readiness review session failed",
      ),
    };
  }
}
