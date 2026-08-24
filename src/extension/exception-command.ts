import { createHash } from "node:crypto";
import path from "node:path";
import {
  getAgentDir,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { FileHumanExceptionAuthority } from "../adapters/exceptions/file-human-exception-authority.js";
import { parseHumanExceptionApproval } from "../core/exceptions/human-exception.js";

export function exceptionAuthority(
  context: ExtensionContext,
): FileHumanExceptionAuthority {
  const identity = createHash("sha256").update(context.cwd).digest("hex");
  return new FileHumanExceptionAuthority(
    path.join(getAgentDir(), "tiber", "exceptions", identity),
  );
}

export async function handleExceptionCommand(
  argumentsText: string,
  context: ExtensionContext,
): Promise<void> {
  const authority = exceptionAuthority(context);
  const pending = await authority.pending();
  if (!pending.ok) {
    context.ui.notify(
      `${pending.failure.code}: ${pending.failure.message}`,
      "error",
    );
    return;
  }
  const attentionId = argumentsText.trim();
  if (attentionId.length === 0) {
    context.ui.notify(
      pending.value.length === 0
        ? "No pending human exception requests"
        : pending.value
            .map(
              (item) =>
                `${item.attention.attentionId} | ${item.attention.taskId} | ${item.attention.denialCode} | ${item.attention.goal}`,
            )
            .join("\n"),
      "info",
    );
    return;
  }
  const item = pending.value.find(
    (candidate) => candidate.attention.attentionId === attentionId,
  );
  if (item === undefined) {
    context.ui.notify("TIBER_EXCEPTION_ATTENTION_NOT_FOUND", "error");
    return;
  }
  const approved = await context.ui.confirm(
    "Approve one exact short-lived exception?",
    [
      `Goal: ${item.attention.goal}`,
      `Necessity: ${item.attention.rationale}`,
      `Frozen operation: ${JSON.stringify(item.claim.operation)}`,
      `Task/run/revision: ${item.claim.taskId} / ${item.claim.runId} / ${item.claim.revision}`,
      "This approval expires in five minutes and can be consumed once.",
    ].join("\n"),
  );
  if (!approved) {
    context.ui.notify("Human exception approval cancelled", "info");
    return;
  }
  const approvedAt = new Date();
  const approval = parseHumanExceptionApproval({
    attentionId,
    approvedAt: approvedAt.toISOString(),
    expiresAt: new Date(approvedAt.getTime() + 5 * 60_000).toISOString(),
    humanIdentity: "interactive-human",
  });
  if (!approval.ok) {
    context.ui.notify(approval.failure.code, "error");
    return;
  }
  const result = await authority.approve(approval.value);
  context.ui.notify(
    result.ok
      ? "Exact one-use human exception approved"
      : `${result.failure.code}: ${result.failure.message}`,
    result.ok ? "info" : "error",
  );
}
