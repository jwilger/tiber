import {
  createAgentSession,
  getAgentDir,
  SessionManager,
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

export function parseReadinessReviewOutput(
  text: string,
  digest: SpecificationDigest,
): TiberResult<ReadinessReview, ModelReviewFailure> {
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
    Object.keys(value).length !== 1
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review output has an invalid shape",
      ),
    };
  if (!("findingCount" in value))
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_READINESS_REVIEW_INVALID",
        "readiness review output omits findingCount",
      ),
    };
  const findingCount = parseSpecificationReviewFindingCount(value.findingCount);
  return findingCount.ok
    ? {
        ok: true,
        value: {
          contextFreshness: "fresh",
          reviewerRole: "specification-reviewer",
          findingCount: findingCount.value,
          reviewedSpecificationDigest: digest,
        },
      }
    : {
        ok: false,
        failure: modelReviewFailure(
          "TIBER_READINESS_REVIEW_INVALID",
          "readiness review findingCount is invalid",
        ),
      };
}

async function conductSpecificationReview(
  cwd: string,
  specification: TaskSpecification,
  digest: SpecificationDigest,
): Promise<TiberResult<ReadinessReview, ModelReviewFailure>> {
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
    ) {
      void session.abort();
    }
  });
  const assignment = [
    "ROLE: Tiber specification readiness reviewer v1.",
    "Review independently with fresh context. Do not authorize effects.",
    "Assess outcome, Gherkin scenarios, edge cases, exclusions, dependencies, test mappings, and architecture implications.",
    'Return only JSON matching exactly: {"findingCount":<non-negative integer>}.',
    `SPECIFICATION_DIGEST: ${digest}`,
    `SPECIFICATION: ${JSON.stringify(specification)}`,
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
            "readiness reviewer returned no text response",
          ),
        }
      : parseReadinessReviewOutput(text, digest);
  } finally {
    clearTimeout(timeout);
    unsubscribe();
    session.dispose();
  }
}

export async function reviewSpecification(
  cwd: string,
  specification: TaskSpecification,
  digest: SpecificationDigest,
): Promise<TiberResult<ReadinessReview, ModelReviewFailure>> {
  try {
    return await conductSpecificationReview(cwd, specification, digest);
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
