import {
  createAgentSession,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";

import type {
  ReadinessReview,
  TaskSpecification,
} from "../../core/tasks/readiness.js";

export function parseReadinessReviewOutput(
  text: string,
  digest: string,
): ReadinessReview | undefined {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).length !== 1
  )
    return undefined;
  if (!("findingCount" in value)) return undefined;
  const findingCount: unknown = value.findingCount;
  return typeof findingCount === "number" &&
    Number.isSafeInteger(findingCount) &&
    findingCount >= 0
    ? {
        freshContext: true,
        reviewerRole: "specification-reviewer",
        findingCount,
        reviewedSpecificationDigest: digest,
      }
    : undefined;
}

export async function reviewSpecification(
  cwd: string,
  specification: TaskSpecification,
  digest: string,
): Promise<ReadinessReview | undefined> {
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
      ? undefined
      : parseReadinessReviewOutput(text, digest);
  } finally {
    clearTimeout(timeout);
    unsubscribe();
    session.dispose();
  }
}
