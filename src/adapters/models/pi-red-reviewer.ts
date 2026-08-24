import {
  createAgentSession,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";

import type { TiberResult } from "../../core/failures/tiber-failure.js";
import type { TaskSpecification } from "../../core/tasks/readiness.js";
import type {
  RedObservation,
  RedReview,
} from "../../core/workflow/semantic-red.js";
import {
  parseRedReviewRationale,
  type RedDiagnosticDigest,
} from "../../core/workflow/workflow-values.js";
import {
  modelReviewFailure,
  type ModelReviewFailure,
} from "./model-review-failure.js";

export function parseRedReviewOutput(
  text: string,
  diagnosticDigest: RedDiagnosticDigest,
): TiberResult<RedReview, ModelReviewFailure> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_RED_REVIEW_INVALID",
        "RED review output is not valid JSON",
      ),
    };
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(",") !==
      "classification,missingPublicSurface,rationale" ||
    !("classification" in value) ||
    (value.classification !== "valid-red" &&
      value.classification !== "unrelated-failure" &&
      value.classification !== "invalid-red") ||
    !("missingPublicSurface" in value) ||
    typeof value.missingPublicSurface !== "boolean" ||
    !("rationale" in value)
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_RED_REVIEW_INVALID",
        "RED review output has an invalid shape",
      ),
    };
  const rationale = parseRedReviewRationale(
    typeof value.rationale === "string"
      ? value.rationale.trim()
      : value.rationale,
  );
  if (!rationale.ok)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_RED_REVIEW_INVALID",
        "RED review rationale is invalid",
      ),
    };
  return {
    ok: true,
    value: {
      contextFreshness: "fresh",
      reviewerRole: "red-classifier",
      reviewedDiagnosticDigest: diagnosticDigest,
      classification: value.classification,
      missingPublicSurface: value.missingPublicSurface,
      rationale: rationale.value,
    },
  };
}

async function conductRedReview(
  cwd: string,
  specification: TaskSpecification,
  observation: RedObservation,
  diagnostic: string,
): Promise<TiberResult<RedReview, ModelReviewFailure>> {
  if (Buffer.byteLength(diagnostic) > 65_536)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_RED_REVIEW_INVALID",
        "RED diagnostic exceeds its review bound",
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
    "ROLE: Tiber independent semantic RED classifier v1.",
    "Use fresh context. Assess but never authorize effects.",
    "Classify whether the exact diagnostic is caused by the named mapped scenario lacking required production behavior.",
    "Reject unrelated failures, infrastructure failures, pre-existing failures, and tests that pass.",
    "A compile failure is valid only when it specifically demonstrates a missing public surface required by this scenario.",
    'Return only JSON: {"classification":"valid-red"|"unrelated-failure"|"invalid-red","missingPublicSurface":boolean,"rationale":"at least 12 chars"}.',
    `SPECIFICATION: ${JSON.stringify(specification)}`,
    `OBSERVATION: ${JSON.stringify(observation)}`,
    `EXACT_DIAGNOSTIC_SHA256: ${observation.diagnosticDigest}`,
    `EXACT_DIAGNOSTIC:\n${diagnostic}`,
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
            "RED reviewer returned no text response",
          ),
        }
      : parseRedReviewOutput(text, observation.diagnosticDigest);
  } finally {
    clearTimeout(timeout);
    unsubscribe();
    session.dispose();
  }
}

export async function reviewRedObservation(
  cwd: string,
  specification: TaskSpecification,
  observation: RedObservation,
  diagnostic: string,
): Promise<TiberResult<RedReview, ModelReviewFailure>> {
  try {
    return await conductRedReview(cwd, specification, observation, diagnostic);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "RED review session failed",
      ),
    };
  }
}
