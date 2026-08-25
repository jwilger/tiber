import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const sdk = vi.hoisted(() => {
  interface ReviewEvent {
    readonly type: "message_update";
    readonly message: {
      readonly role: "assistant";
      readonly usage: { readonly output: number };
    };
  }
  type Listener = (event: ReviewEvent) => void;
  let listener: Listener | undefined;
  let resolvePrompt: (() => void) | undefined;
  let behavior: "complete" | "output-budget" | "wait-for-abort" = "complete";
  let promptCount = 0;

  const session = {
    messages: [
      {
        role: "assistant" as const,
        content: [
          {
            type: "text" as const,
            text: '{"findingCount":0,"findings":[]}',
          },
        ],
      },
    ],
    subscribe(candidate: Listener): () => void {
      listener = candidate;
      return () => {
        listener = undefined;
      };
    },
    async prompt(): Promise<void> {
      promptCount += 1;
      if (behavior === "output-budget") {
        listener?.({
          type: "message_update",
          message: { role: "assistant", usage: { output: 4097 } },
        });
        return;
      }
      if (behavior === "wait-for-abort")
        await new Promise<void>((resolve) => {
          resolvePrompt = resolve;
        });
    },
    abort(): Promise<void> {
      resolvePrompt?.();
      resolvePrompt = undefined;
      return Promise.resolve();
    },
    dispose(): void {
      return;
    },
  };

  return {
    session,
    setBehavior(value: typeof behavior): void {
      behavior = value;
    },
    promptCount(): number {
      return promptCount;
    },
    reset(): void {
      behavior = "complete";
      promptCount = 0;
      listener = undefined;
      resolvePrompt = undefined;
    },
  };
});

vi.mock("@earendil-works/pi-coding-agent", () => ({
  createAgentSession: () => Promise.resolve({ session: sdk.session }),
  createExtensionRuntime: () => ({}),
  getAgentDir: () => "/agent",
  SessionManager: { inMemory: () => ({}) },
}));

import { reviewSpecification } from "../../src/adapters/models/pi-readiness-reviewer.js";
import { parseTaskSpecification } from "../../src/core/tasks/readiness.js";
import { parseSpecificationDigest } from "../../src/core/tasks/task-values.js";

const parsedDigest = parseSpecificationDigest(`sha256:${"a".repeat(64)}`);
if (!parsedDigest.ok) throw new Error("invalid digest fixture");
const digest = parsedDigest.value;
const parsedSpecification = parseTaskSpecification({
  outcome: "Reject every terminated readiness review.",
  scenarios: [
    {
      name: "terminated review",
      given: ["A model response appears valid"],
      when: ["The review exceeds a bound or is cancelled"],
      then: ["The response cannot establish readiness"],
    },
  ],
  acceptanceCriteria: ["Terminated output is never accepted"],
  exclusions: ["Live providers"],
  dependencies: ["Scripted Pi model session"],
  testMappings: ["terminated review maps to these adapter tests"],
  architectureImplications: "Model termination fails closed.",
});
if (!parsedSpecification.ok) throw new Error("invalid specification fixture");
const specification = parsedSpecification.value;

describe("readiness reviewer execution bounds", () => {
  beforeEach(() => {
    sdk.reset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("reports progress-rendering failure without dispatching the reviewer", async () => {
    const result = await reviewSpecification("/repo", specification, digest, {
      onProgress(message) {
        if (message === "Independent readiness reviewer is working")
          throw new Error("render failed");
      },
    });

    expect(result).toMatchObject({
      ok: false,
      failure: { code: "TIBER_REVIEW_EXECUTION_FAILED" },
    });
    expect(sdk.promptCount()).toBe(0);
  });

  it("rejects an apparently valid response after in-flight cancellation", async () => {
    const controller = new AbortController();

    const result = await reviewSpecification("/repo", specification, digest, {
      signal: controller.signal,
      onProgress(message) {
        if (message === "Independent readiness reviewer is working")
          controller.abort();
      },
    });

    expect(result).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_REVIEW_CANCELLED",
        retryability: "not-retryable",
        requiredRecoveryEvidence: [],
      },
    });
    expect(sdk.promptCount()).toBe(0);
  });

  it("rejects an apparently valid response after its output budget is exceeded", async () => {
    sdk.setBehavior("output-budget");

    await expect(
      reviewSpecification("/repo", specification, digest),
    ).resolves.toMatchObject({
      ok: false,
      failure: { code: "TIBER_REVIEW_BUDGET_EXCEEDED" },
    });
  });

  it("rejects an apparently valid response after its time budget is exceeded", async () => {
    vi.useFakeTimers();
    sdk.setBehavior("wait-for-abort");

    const result = reviewSpecification("/repo", specification, digest);
    await vi.advanceTimersByTimeAsync(60_000);

    await expect(result).resolves.toMatchObject({
      ok: false,
      failure: { code: "TIBER_REVIEW_TIMED_OUT" },
    });
  });
});
