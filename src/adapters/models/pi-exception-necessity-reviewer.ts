import {
  createAgentSession,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";
import type {
  ExceptionBlockerClaim,
  ExceptionNecessityReview,
} from "../../core/exceptions/human-exception.js";
import type { TiberResult } from "../../core/failures/tiber-failure.js";
import {
  modelReviewFailure,
  type ModelReviewFailure,
} from "./model-review-failure.js";

export function parseExceptionNecessityReviewOutput(
  text: string,
): TiberResult<ExceptionNecessityReview, ModelReviewFailure> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_EXCEPTION_REVIEW_INVALID",
        "exception necessity review output is not valid JSON",
      ),
    };
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).length !== 2 ||
    !("disposition" in value) ||
    !("rationale" in value) ||
    (value.disposition !== "necessary" &&
      value.disposition !== "compliant-route-available") ||
    typeof value.rationale !== "string" ||
    value.rationale.length === 0
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_EXCEPTION_REVIEW_INVALID",
        "exception necessity review output has an invalid shape",
      ),
    };
  return {
    ok: true,
    value: {
      disposition: value.disposition,
      rationale: value.rationale,
      reviewerIdentity: "independent-exception-reviewer",
    },
  };
}

export async function reviewExceptionNecessity(
  cwd: string,
  claim: ExceptionBlockerClaim,
): Promise<TiberResult<ExceptionNecessityReview, ModelReviewFailure>> {
  try {
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
    const timeout = setTimeout(() => void session.abort(), 60_000);
    try {
      await session.prompt(
        [
          "ROLE: Tiber independent human-exception necessity reviewer v1.",
          "Do not authorize effects. Determine whether the stated goal is genuinely blocked and no compliant route remains.",
          'Return only exact JSON: {"disposition":"necessary"|"compliant-route-available","rationale":"non-empty explanation"}.',
          `BLOCKER_CLAIM: ${JSON.stringify(claim)}`,
        ].join("\n"),
        { expandPromptTemplates: false },
      );
      const message = [...session.messages]
        .reverse()
        .find((item) => item.role === "assistant");
      const content = message?.content.find((part) => part.type === "text");
      return content?.type === "text"
        ? parseExceptionNecessityReviewOutput(content.text)
        : {
            ok: false,
            failure: modelReviewFailure(
              "TIBER_REVIEW_RESPONSE_MISSING",
              "exception necessity reviewer returned no text response",
            ),
          };
    } finally {
      clearTimeout(timeout);
      unsubscribe();
      session.dispose();
    }
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "exception necessity review session failed",
      ),
    };
  }
}
