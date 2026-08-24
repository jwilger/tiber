import {
  createAgentSession,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";

import type { TiberResult } from "../../core/failures/tiber-failure.js";
import type { TaskSpecification } from "../../core/tasks/readiness.js";
import type {
  FinalLensReview,
  FinalReviewLens,
} from "../../core/workflow/final-review.js";
import {
  parseFinalReviewFindingCount,
  parseFinalReviewRationale,
  type SourceDiffText,
  type SourceSnapshotDigest,
  type VerificationDiagnosticDigest,
} from "../../core/workflow/workflow-values.js";
import {
  modelReviewFailure,
  type ModelReviewFailure,
} from "./model-review-failure.js";

export function parseFinalReviewOutput(
  text: string,
  lens: FinalReviewLens,
): TiberResult<FinalLensReview, ModelReviewFailure> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_FINAL_REVIEW_INVALID",
        "final review output is not valid JSON",
      ),
    };
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(",") !== "findingCount,rationale" ||
    !("findingCount" in value) ||
    !("rationale" in value)
  )
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_FINAL_REVIEW_INVALID",
        "final review output has an invalid shape",
      ),
    };
  const findingCount = parseFinalReviewFindingCount(value.findingCount);
  const rationale = parseFinalReviewRationale(
    typeof value.rationale === "string"
      ? value.rationale.trim()
      : value.rationale,
  );
  if (!findingCount.ok || !rationale.ok)
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_FINAL_REVIEW_INVALID",
        "final review values are invalid",
      ),
    };
  return {
    ok: true,
    value: {
      lens,
      contextFreshness: "fresh",
      findingCount: findingCount.value,
      rationale: rationale.value,
    },
  };
}

async function conductFinalReview(
  cwd: string,
  specification: TaskSpecification,
  lens: FinalReviewLens,
  sourceDiff: SourceDiffText,
  sourceSnapshotDigest: SourceSnapshotDigest,
  verificationDiagnosticDigest: VerificationDiagnosticDigest,
): Promise<TiberResult<FinalLensReview, ModelReviewFailure>> {
  const { session } = await createAgentSession({
    cwd,
    agentDir: getAgentDir(),
    noTools: "all",
    sessionManager: SessionManager.inMemory(cwd),
    thinkingLevel: "high",
  });
  const unsubscribe = session.subscribe((event) => {
    if (
      event.type === "message_update" &&
      event.message.role === "assistant" &&
      event.message.usage.output > 8192
    )
      void session.abort();
  });
  const assignment = [
    `ROLE: Tiber fresh complete final ${lens} reviewer v1.`,
    "Assess semantics only and never authorize effects.",
    "Review the complete exact source diff and specification through only the named lens. Do not omit findings because another lens may cover them.",
    "Reject primitive obsession, repeated domain parsing, generic expected-error throws, assertions at trust boundaries, and any incomplete acceptance behavior applicable to this lens.",
    'Return only JSON: {"findingCount":non-negative integer,"rationale":"at least 12 chars"}.',
    `SPECIFICATION: ${JSON.stringify(specification)}`,
    `SOURCE_SNAPSHOT_DIGEST: ${sourceSnapshotDigest}`,
    `VERIFICATION_DIAGNOSTIC_DIGEST: ${verificationDiagnosticDigest}`,
    `SOURCE_DIFF:\n${sourceDiff}`,
  ].join("\n");
  const timeout = setTimeout(() => void session.abort(), 90_000);
  try {
    await session.prompt(assignment, { expandPromptTemplates: false });
    let response: string | undefined;
    for (let index = session.messages.length - 1; index >= 0; index -= 1) {
      const message = session.messages[index];
      if (message?.role !== "assistant") continue;
      const content = message.content.find((part) => part.type === "text");
      if (content?.type === "text") {
        response = content.text;
        break;
      }
    }
    return response === undefined
      ? {
          ok: false,
          failure: modelReviewFailure(
            "TIBER_REVIEW_RESPONSE_MISSING",
            "final reviewer returned no text response",
          ),
        }
      : parseFinalReviewOutput(response, lens);
  } finally {
    clearTimeout(timeout);
    unsubscribe();
    session.dispose();
  }
}

export async function reviewFinalLens(
  cwd: string,
  specification: TaskSpecification,
  lens: FinalReviewLens,
  sourceDiff: SourceDiffText,
  sourceSnapshotDigest: SourceSnapshotDigest,
  verificationDiagnosticDigest: VerificationDiagnosticDigest,
): Promise<TiberResult<FinalLensReview, ModelReviewFailure>> {
  try {
    return await conductFinalReview(
      cwd,
      specification,
      lens,
      sourceDiff,
      sourceSnapshotDigest,
      verificationDiagnosticDigest,
    );
  } catch {
    return {
      ok: false,
      failure: modelReviewFailure(
        "TIBER_REVIEW_EXECUTION_FAILED",
        "final review session failed",
      ),
    };
  }
}
