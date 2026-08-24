import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type, type Static } from "typebox";
import { freezeExceptionClaim } from "../adapters/exceptions/exception-state-observer.js";
import type { FileHumanExceptionAuthority } from "../adapters/exceptions/file-human-exception-authority.js";
import { executeFrozenException } from "../adapters/exceptions/frozen-exception-executor.js";
import { reviewExceptionNecessity } from "../adapters/models/pi-exception-necessity-reviewer.js";
import {
  parseExceptionExecutionTime,
  parseHumanExceptionApproval,
  type ExceptionBlockerClaim,
} from "../core/exceptions/human-exception.js";
import { exceptionAuthority } from "./exception-command.js";

const operationSchema = Type.Object({
  kind: Type.Literal("structured-command"),
  executable: Type.String(),
  arguments: Type.Array(Type.String()),
  environment: Type.Array(
    Type.Object({ name: Type.String(), value: Type.String() }),
  ),
  workingDirectory: Type.String(),
  timeoutMs: Type.Integer(),
  maxOutputBytes: Type.Integer(),
  paths: Type.Array(Type.String()),
});
const claimSchema = Type.Object({
  taskId: Type.String(),
  runId: Type.String(),
  goal: Type.String(),
  denialCode: Type.String(),
  compliantAlternatives: Type.Array(Type.String()),
  operation: operationSchema,
});
const requestSchema = Type.Object({
  action: StringEnum(["escalate", "execute"] as const),
  claim: claimSchema,
});
type Request = Static<typeof requestSchema>;
const response = (text: string, disposition: string) => ({
  content: [{ type: "text" as const, text }],
  details: { disposition },
});

async function executeApprovedClaim(
  authority: FileHumanExceptionAuthority,
  claim: ExceptionBlockerClaim,
) {
  const executionTime = parseExceptionExecutionTime(new Date().toISOString());
  if (!executionTime.ok) return response(executionTime.failure.code, "failed");
  const consumed = await authority.consume(claim, executionTime.value);
  if (!consumed.ok)
    return response(
      `${consumed.failure.code}: ${consumed.failure.message}`,
      "denied",
    );
  const observed = await executeFrozenException(consumed.value);
  if (!observed.ok)
    return response(
      `${observed.failure.code}: ${observed.failure.message}`,
      "failed",
    );
  const recorded = await authority.recordObservation(observed.value);
  return recorded.ok
    ? response(
        `Frozen exception operation observed with exit ${String(recorded.value.exitCode)}; stdout ${recorded.value.stdoutDigest}; stderr ${recorded.value.stderrDigest}`,
        "observed",
      )
    : response(
        `${recorded.failure.code}: ${recorded.failure.message}`,
        "failed",
      );
}

export function registerExceptionRequestTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "tiber_exception_request",
    label: "Tiber human exception request",
    description:
      "Request independent escalation or execution evaluation for one exact blocked operation. The model cannot approve, receive, or reuse exception capability.",
    promptSnippet:
      "Request exact human exception evaluation only after compliant recovery is exhausted",
    promptGuidelines: [
      "Use only for a genuine observed blocker after every compliant route is exhausted. Never claim approval or ask for capability material.",
    ],
    parameters: requestSchema,
    async execute(_id, parameters: Request, signal, _update, context) {
      if (signal?.aborted)
        return response("TIBER_EXCEPTION_REQUEST_CANCELLED", "cancelled");
      const claim = await freezeExceptionClaim(parameters.claim);
      if (!claim.ok)
        return response(
          `${claim.failure.code}: ${claim.failure.message}`,
          "denied",
        );
      const authority = exceptionAuthority(context);
      if (parameters.action === "escalate") {
        const review = await reviewExceptionNecessity(context.cwd, claim.value);
        if (!review.ok)
          return response(
            `${review.failure.code}: ${review.failure.message}`,
            "denied",
          );
        const escalated = await authority.escalate(claim.value, review.value);
        if (!escalated.ok)
          return response(
            `${escalated.failure.code}: ${escalated.failure.message}`,
            "denied",
          );
        const humanApproved = await context.ui.confirm(
          "Approve and execute one exact short-lived exception?",
          [
            `Goal: ${escalated.value.goal}`,
            `Necessity: ${escalated.value.rationale}`,
            `Frozen claim: ${JSON.stringify(claim.value)}`,
            "Approval expires in five minutes, is consumed before execution, and cannot be replayed.",
          ].join("\n"),
        );
        if (!humanApproved)
          return response(
            `Human attention requested for exact blocker ${escalated.value.attentionId}`,
            "attention-requested",
          );
        const approvedAt = new Date();
        const approval = parseHumanExceptionApproval({
          attentionId: escalated.value.attentionId,
          approvedAt: approvedAt.toISOString(),
          expiresAt: new Date(approvedAt.getTime() + 5 * 60_000).toISOString(),
          humanIdentity: "interactive-human",
        });
        if (!approval.ok) return response(approval.failure.code, "failed");
        const approved = await authority.approve(approval.value);
        if (!approved.ok)
          return response(
            `${approved.failure.code}: ${approved.failure.message}`,
            "denied",
          );
        return executeApprovedClaim(authority, claim.value);
      }
      return executeApprovedClaim(authority, claim.value);
    },
  });
}
