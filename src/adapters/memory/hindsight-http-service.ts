import {
  authorizeHindsightOperation,
  decideHindsightRetention,
  parseHindsightRecallResponse,
  type HindsightConfiguration,
  type HindsightFailure,
  type HindsightMemory,
  type HindsightRecallRequest,
  type HindsightResult,
  type HindsightRetentionCandidate,
} from "../../core/memory/hindsight.js";
import { fail, succeed } from "../../core/failures/tiber-failure.js";

function failure(
  code: HindsightFailure["code"],
  message: string,
): HindsightResult<never> {
  return fail({
    code,
    message,
    safeContext: { domain: "hindsight" },
    causes: [],
    retryability:
      code === "TIBER_HINDSIGHT_HTTP_FAILED"
        ? "transient"
        : "retry-after-input",
    requiredRecoveryEvidence: [
      code === "TIBER_HINDSIGHT_HTTP_FAILED"
        ? "retry-memory-service"
        : "valid-memory-response",
    ],
    redaction: "public",
  });
}
export interface HindsightRecall {
  readonly scope: HindsightRecallRequest["scope"];
  readonly bankId: string;
  readonly memories: readonly HindsightMemory[];
}
export interface HindsightRetentionReceipt {
  readonly scope: HindsightRetentionCandidate["scope"];
  readonly bankId: string;
  readonly documentId: string;
  readonly itemsCount: number;
}
export class HindsightHttpService {
  public constructor(
    private readonly configuration: HindsightConfiguration,
    private readonly apiKey?: string,
  ) {}
  private async request(
    path: string,
    body: unknown,
    signal?: AbortSignal,
  ): Promise<HindsightResult<unknown>> {
    const boundedSignal = AbortSignal.any([
      AbortSignal.timeout(this.configuration.timeoutMs),
      ...(signal === undefined ? [] : [signal]),
    ]);
    try {
      const response = await fetch(new URL(path, this.configuration.endpoint), {
        method: "POST",
        redirect: "error",
        signal: boundedSignal,
        headers: {
          "content-type": "application/json",
          accept: "application/json",
          ...(this.apiKey === undefined
            ? {}
            : { authorization: `Bearer ${this.apiKey}` }),
        },
        body: JSON.stringify(body),
      });
      if (!response.ok || response.body === null)
        return failure(
          "TIBER_HINDSIGHT_HTTP_FAILED",
          "Hindsight request did not return success",
        );
      const reader = response.body.getReader();
      const chunks: Uint8Array[] = [];
      let length = 0;
      let reading = true;
      while (reading) {
        const part: unknown = await reader.read();
        if (typeof part !== "object" || part === null || !("done" in part))
          return failure(
            "TIBER_HINDSIGHT_RESPONSE_INVALID",
            "Hindsight response stream is invalid",
          );
        if (part.done === true) {
          reading = false;
          continue;
        }
        if (!("value" in part) || !(part.value instanceof Uint8Array))
          return failure(
            "TIBER_HINDSIGHT_RESPONSE_INVALID",
            "Hindsight response stream is invalid",
          );
        length += part.value.byteLength;
        if (length > this.configuration.maximumResponseBytes) {
          await reader.cancel();
          return failure(
            "TIBER_HINDSIGHT_RESPONSE_OVERSIZED",
            "Hindsight response exceeded its hard byte bound",
          );
        }
        chunks.push(part.value);
      }
      try {
        return succeed(
          JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown,
        );
      } catch {
        return failure(
          "TIBER_HINDSIGHT_RESPONSE_INVALID",
          "Hindsight response was not valid JSON",
        );
      }
    } catch {
      return failure(
        "TIBER_HINDSIGHT_HTTP_FAILED",
        "Hindsight request failed or timed out",
      );
    }
  }
  public async recall(
    request: HindsightRecallRequest,
    signal?: AbortSignal,
  ): Promise<HindsightResult<HindsightRecall>> {
    const bank = authorizeHindsightOperation(
      this.configuration,
      request.scope,
      "recall",
    );
    if (!bank.ok) return bank;
    const response = await this.request(
      `/v1/default/banks/${encodeURIComponent(bank.value)}/memories/recall`,
      {
        query: request.query,
        budget: "low",
        max_tokens: request.maximumTokens,
        types: ["world", "experience", "observation"],
        prefer_observations: true,
        include: { entities: null },
      },
      signal,
    );
    if (!response.ok) return response;
    const memories = parseHindsightRecallResponse(response.value);
    return memories.ok
      ? succeed({
          scope: request.scope,
          bankId: bank.value,
          memories: memories.value,
        })
      : memories;
  }
  public async retain(
    candidate: HindsightRetentionCandidate,
    signal?: AbortSignal,
  ): Promise<HindsightResult<HindsightRetentionReceipt>> {
    const decision = decideHindsightRetention(candidate);
    if (decision.status === "denied")
      return failure("TIBER_HINDSIGHT_PERMISSION_DENIED", decision.code);
    const bank = authorizeHindsightOperation(
      this.configuration,
      candidate.scope,
      "retain",
    );
    if (!bank.ok) return bank;
    const response = await this.request(
      `/v1/default/banks/${encodeURIComponent(bank.value)}/memories`,
      {
        items: [
          {
            content: candidate.content,
            context:
              candidate.kind === "completion"
                ? "reviewed Tiber task completion"
                : "private Tiber progress checkpoint",
            document_id: candidate.documentId,
            tags: [`scope:${candidate.scope}`],
            observation_scopes: "combined",
          },
        ],
        async: false,
      },
      signal,
    );
    if (!response.ok) return response;
    if (
      typeof response.value !== "object" ||
      response.value === null ||
      !("success" in response.value) ||
      response.value.success !== true ||
      !("items_count" in response.value) ||
      response.value.items_count !== 1
    )
      return failure(
        "TIBER_HINDSIGHT_RESPONSE_INVALID",
        "Hindsight retain response is invalid",
      );
    return succeed({
      scope: candidate.scope,
      bankId: bank.value,
      documentId: candidate.documentId,
      itemsCount: 1,
    });
  }
}
