import {
  createAgentSession,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";

import type { TiberResult } from "../../core/failures/tiber-failure.js";
import type { TaskSpecification } from "../../core/tasks/readiness.js";
import type { ScenarioName } from "../../core/tasks/task-values.js";
import type { LightweightReview } from "../../core/workflow/green-increment.js";
import {
  parseIncrementReviewFindingCount,
  parseIncrementReviewRationale,
  type SourceDiffDigest,
  type SourceDiffText,
} from "../../core/workflow/workflow-values.js";
import {
  modelReviewFailure,
  type ModelReviewFailure,
} from "./model-review-failure.js";

export function parseIncrementReviewOutput(
  text: string,
  scenarioName: ScenarioName,
  sourceDiffDigest: SourceDiffDigest,
): TiberResult<LightweightReview, ModelReviewFailure> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_INCREMENT_REVIEW_INVALID",
        "increment review output is not valid JSON",
      ),
    };
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(",") !==
      "findingCount,overimplementation,rationale" ||
    !("findingCount" in value) ||
    !("overimplementation" in value) ||
    typeof value.overimplementation !== "boolean" ||
    !("rationale" in value)
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_INCREMENT_REVIEW_INVALID",
        "increment review output has an invalid shape",
      ),
    };
  const findingCount = parseIncrementReviewFindingCount(value.findingCount);
  const rationale = parseIncrementReviewRationale(
    typeof value.rationale === "string"
      ? value.rationale.trim()
      : value.rationale,
  );
  if (!findingCount.ok || !rationale.ok)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_INCREMENT_REVIEW_INVALID",
        "increment review values are invalid",
      ),
    };
  return {
    ok: true,
    value: {
      contextFreshness: "fresh",
      reviewerRole: "lightweight-increment-reviewer",
      reviewedScenarioName: scenarioName,
      reviewedSourceDiffDigest: sourceDiffDigest,
      findingCount: findingCount.value,
      overimplementation: value.overimplementation,
      rationale: rationale.value,
    },
  };
}

async function conductIncrementReview(
  cwd: string,
  specification: TaskSpecification,
  scenarioName: ScenarioName,
  sourceDiff: SourceDiffText,
  sourceDiffDigest: SourceDiffDigest,
): Promise<TiberResult<LightweightReview, ModelReviewFailure>> {
  if (Buffer.byteLength(sourceDiff) > 65_536)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_INCREMENT_REVIEW_INVALID",
        "increment source diff exceeds its review bound",
      ),
    };
  const { session } = await createAgentSession({
    cwd,
    agentDir: getAgentDir(),
    noTools: "all",
    sessionManager: SessionManager.inMemory(cwd),
    thinkingLevel: "medium",
  });
  const unsubscribe = session.subscribe((event) => {
    if (
      event.type === "message_update" &&
      event.message.role === "assistant" &&
      event.message.usage.output > 4096
    )
      void session.abort();
  });
  const assignment = [
    "ROLE: Tiber fresh lightweight vertical-increment reviewer v1.",
    "Assess semantics only and never authorize effects.",
    "Review the exact source diff against only the named scenario and specification.",
    "Count correctness, maintainability, scope, test, and semantic-type findings. Mark overimplementation when changes exceed the minimal scenario need.",
    "Reject primitive obsession: interchangeable identifier/revision/digest/path/limit/identity/state/capability primitives, repeated domain parsing, generic expected-error throws, and assertions substituting for trust-boundary validation.",
    'Return only JSON: {"findingCount":non-negative integer,"overimplementation":boolean,"rationale":"at least 12 chars"}.',
    `SCENARIO: ${scenarioName}`,
    `SPECIFICATION: ${JSON.stringify(specification)}`,
    `SOURCE_DIFF_DIGEST: ${sourceDiffDigest}`,
    `SOURCE_DIFF:\n${sourceDiff}`,
  ].join("\n");
  const timeout = setTimeout(() => {
    void session.abort();
  }, 60_000);
  try {
    await session.prompt(assignment, { expandPromptTemplates: false });
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
            "increment reviewer returned no text response",
          ),
        }
      : parseIncrementReviewOutput(text, scenarioName, sourceDiffDigest);
  } finally {
    clearTimeout(timeout);
    unsubscribe();
    session.dispose();
  }
}

export async function reviewIncrement(
  cwd: string,
  specification: TaskSpecification,
  scenarioName: ScenarioName,
  sourceDiff: SourceDiffText,
  sourceDiffDigest: SourceDiffDigest,
): Promise<TiberResult<LightweightReview, ModelReviewFailure>> {
  try {
    return await conductIncrementReview(
      cwd,
      specification,
      scenarioName,
      sourceDiff,
      sourceDiffDigest,
    );
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "increment review session failed",
      ),
    };
  }
}
