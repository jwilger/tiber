import { createHash, randomUUID } from "node:crypto";
import { mkdir, open, readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import {
  fail,
  operationalFailure,
  succeed,
  type Result,
  type TiberFailure,
} from "../../core/failures/tiber-failure.js";

type ExceptionFailure = TiberFailure<string, unknown, unknown>;
import {
  parseExceptionBlockerClaim,
  parseHumanExceptionApproval,
  type ExceptionAttention,
  type ExceptionBlockerClaim,
  type ExceptionExecutionAttempt,
  type ExceptionExecutionObservation,
  type ExceptionExecutionTime,
  type ExceptionNecessityReview,
  type HumanExceptionApproval,
} from "../../core/exceptions/human-exception.js";

interface StoredAttention {
  readonly attention: ExceptionAttention;
  readonly claim: ExceptionBlockerClaim;
  readonly approval?: HumanExceptionApproval;
  readonly attemptId?: string;
}
type AuditEvent =
  | {
      readonly kind: "exception-escalated";
      readonly identity: string;
      readonly recordedAt: string;
      readonly claimDigest: string;
    }
  | {
      readonly kind: "exception-approved";
      readonly identity: string;
      readonly recordedAt: string;
      readonly humanIdentity: string;
      readonly expiresAt: string;
    }
  | {
      readonly kind: "exception-consumed";
      readonly identity: string;
      readonly recordedAt: string;
      readonly attentionId: string;
      readonly claimDigest: string;
    }
  | {
      readonly kind: "exception-observed";
      readonly identity: string;
      readonly recordedAt: string;
      readonly exitCode: number;
      readonly stdoutDigest: string;
      readonly stderrDigest: string;
    };
interface State {
  readonly attentions: readonly StoredAttention[];
  readonly events: readonly AuditEvent[];
}

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (typeof value === "object" && value !== null) {
    return `{${Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${canonical(item)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}
const hash = (value: unknown): string =>
  createHash("sha256").update(canonical(value)).digest("hex");
const failure = (message: string, context: string) =>
  operationalFailure(
    "TIBER_EXCEPTION_AUTHORITY_DENIED",
    context,
    message,
    "not-retryable",
  );
const invalidStore = () =>
  operationalFailure(
    "TIBER_EXCEPTION_STORE_INVALID",
    "exception audit store",
    "human exception authority persistence is invalid",
    "retry-after-state-change",
  );
const ioFailure = () =>
  operationalFailure(
    "TIBER_EXCEPTION_STORE_IO",
    "exception audit store",
    "human exception authority persistence failed",
    "transient",
  );

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseState(value: unknown): Result<State, ExceptionFailure> {
  if (
    !object(value) ||
    !("attentions" in value) ||
    !("events" in value) ||
    !Array.isArray(value.attentions) ||
    !Array.isArray(value.events)
  )
    return fail(invalidStore());
  const attentions: StoredAttention[] = [];
  for (const item of value.attentions) {
    if (!object(item) || !("attention" in item) || !("claim" in item))
      return fail(invalidStore());
    const claim = parseExceptionBlockerClaim(item.claim);
    const attention = item.attention;
    if (
      !claim.ok ||
      !object(attention) ||
      !("attentionId" in attention) ||
      typeof attention.attentionId !== "string" ||
      !("claimDigest" in attention) ||
      attention.claimDigest !== hash(claim.value) ||
      !("taskId" in attention) ||
      attention.taskId !== claim.value.taskId ||
      !("runId" in attention) ||
      attention.runId !== claim.value.runId ||
      !("goal" in attention) ||
      attention.goal !== claim.value.goal ||
      !("denialCode" in attention) ||
      attention.denialCode !== claim.value.denialCode ||
      !("rationale" in attention) ||
      typeof attention.rationale !== "string"
    )
      return fail(invalidStore());
    let approval: HumanExceptionApproval | undefined;
    if ("approval" in item) {
      const parsed = parseHumanExceptionApproval(item.approval);
      if (!parsed.ok || parsed.value.attentionId !== attention.attentionId)
        return fail(invalidStore());
      approval = parsed.value;
    }
    const attemptId = "attemptId" in item ? item.attemptId : undefined;
    if (
      attemptId !== undefined &&
      (typeof attemptId !== "string" || approval === undefined)
    )
      return fail(invalidStore());
    const parsedAttention: ExceptionAttention = {
      attentionId: attention.attentionId,
      claimDigest: attention.claimDigest,
      taskId: attention.taskId,
      runId: attention.runId,
      goal: attention.goal,
      denialCode: attention.denialCode,
      rationale: attention.rationale,
    };
    attentions.push({
      attention: parsedAttention,
      claim: claim.value,
      ...(approval === undefined ? {} : { approval }),
      ...(attemptId === undefined ? {} : { attemptId }),
    });
  }
  const events: AuditEvent[] = [];
  for (const event of value.events) {
    if (
      !object(event) ||
      !("kind" in event) ||
      (event.kind !== "exception-escalated" &&
        event.kind !== "exception-approved" &&
        event.kind !== "exception-consumed" &&
        event.kind !== "exception-observed") ||
      !("identity" in event) ||
      typeof event.identity !== "string" ||
      !("recordedAt" in event) ||
      typeof event.recordedAt !== "string" ||
      !Number.isFinite(Date.parse(event.recordedAt))
    )
      return fail(invalidStore());
    if (
      event.kind === "exception-escalated" &&
      "claimDigest" in event &&
      typeof event.claimDigest === "string"
    )
      events.push({
        kind: event.kind,
        identity: event.identity,
        recordedAt: event.recordedAt,
        claimDigest: event.claimDigest,
      });
    else if (
      event.kind === "exception-approved" &&
      "humanIdentity" in event &&
      typeof event.humanIdentity === "string" &&
      "expiresAt" in event &&
      typeof event.expiresAt === "string"
    )
      events.push({
        kind: event.kind,
        identity: event.identity,
        recordedAt: event.recordedAt,
        humanIdentity: event.humanIdentity,
        expiresAt: event.expiresAt,
      });
    else if (
      event.kind === "exception-consumed" &&
      "attentionId" in event &&
      typeof event.attentionId === "string" &&
      "claimDigest" in event &&
      typeof event.claimDigest === "string"
    )
      events.push({
        kind: event.kind,
        identity: event.identity,
        recordedAt: event.recordedAt,
        attentionId: event.attentionId,
        claimDigest: event.claimDigest,
      });
    else if (
      event.kind === "exception-observed" &&
      "exitCode" in event &&
      Number.isSafeInteger(event.exitCode) &&
      "stdoutDigest" in event &&
      typeof event.stdoutDigest === "string" &&
      "stderrDigest" in event &&
      typeof event.stderrDigest === "string"
    )
      events.push({
        kind: event.kind,
        identity: event.identity,
        recordedAt: event.recordedAt,
        exitCode: event.exitCode as number,
        stdoutDigest: event.stdoutDigest,
        stderrDigest: event.stderrDigest,
      });
    else return fail(invalidStore());
  }
  return succeed({ attentions, events });
}

export class FileHumanExceptionAuthority {
  readonly #directory: string;
  readonly #file: string;
  readonly #lock: string;
  constructor(directory: string) {
    this.#directory = directory;
    this.#file = path.join(directory, "audit.json");
    this.#lock = path.join(directory, ".lock");
  }

  async #withLock<T>(
    operation: (
      state: State,
    ) => Result<{ readonly state: State; readonly value: T }, ExceptionFailure>,
  ): Promise<Result<T, ExceptionFailure>> {
    try {
      await mkdir(this.#directory, { recursive: true });
      let lock;
      for (let attempt = 0; attempt < 100; attempt += 1) {
        try {
          lock = await open(this.#lock, "wx");
          break;
        } catch (cause) {
          if ((cause as NodeJS.ErrnoException).code !== "EEXIST")
            return fail(ioFailure());
          await new Promise((resolve) => setTimeout(resolve, 10));
        }
      }
      if (lock === undefined)
        return fail(
          failure("human exception authority is busy", "exception store lock"),
        );
      try {
        let state: State = { attentions: [], events: [] };
        let document: string | undefined;
        try {
          document = await readFile(this.#file, "utf8");
        } catch (cause) {
          if ((cause as NodeJS.ErrnoException).code !== "ENOENT")
            return fail(ioFailure());
        }
        if (document !== undefined) {
          let value: unknown;
          try {
            value = JSON.parse(document);
          } catch {
            return fail(invalidStore());
          }
          const parsed = parseState(value);
          if (!parsed.ok) return parsed;
          state = parsed.value;
        }
        const result = operation(state);
        if (!result.ok) return result;
        const temporary = `${this.#file}.${randomUUID()}.tmp`;
        await writeFile(
          temporary,
          `${JSON.stringify(result.value.state, null, 2)}\n`,
          { mode: 0o600 },
        );
        await rename(temporary, this.#file);
        return succeed(result.value.value);
      } finally {
        await lock.close();
        await rm(this.#lock, { force: true });
      }
    } catch {
      return fail(ioFailure());
    }
  }

  async escalate(
    claim: ExceptionBlockerClaim,
    review: ExceptionNecessityReview,
  ): Promise<Result<ExceptionAttention, ExceptionFailure>> {
    return this.#withLock((state) => {
      if (
        review.disposition !== "necessary" ||
        claim.compliantAlternatives.length !== 0 ||
        review.reviewerIdentity.length === 0 ||
        review.rationale.length === 0
      )
        return fail(
          failure(
            "independent review did not establish exception necessity",
            "exception escalation",
          ),
        );
      const claimDigest = hash(claim);
      const existing = state.attentions.find(
        (item) => item.attention.claimDigest === claimDigest,
      );
      if (existing !== undefined)
        return succeed({ state, value: existing.attention });
      const attention: ExceptionAttention = {
        attentionId: randomUUID(),
        claimDigest,
        taskId: claim.taskId,
        runId: claim.runId,
        goal: claim.goal,
        denialCode: claim.denialCode,
        rationale: review.rationale,
      };
      return succeed({
        state: {
          attentions: [...state.attentions, { attention, claim }],
          events: [
            ...state.events,
            {
              kind: "exception-escalated",
              identity: attention.attentionId,
              recordedAt: new Date().toISOString(),
              claimDigest,
            },
          ],
        },
        value: attention,
      });
    });
  }

  async pending(): Promise<
    Result<readonly StoredAttention[], ExceptionFailure>
  > {
    return this.#withLock((state) =>
      succeed({
        state,
        value: state.attentions.filter(
          (item) => item.approval === undefined && item.attemptId === undefined,
        ),
      }),
    );
  }

  async approve(
    approval: HumanExceptionApproval,
  ): Promise<Result<HumanExceptionApproval, ExceptionFailure>> {
    return this.#withLock((state) => {
      const index = state.attentions.findIndex(
        (item) => item.attention.attentionId === approval.attentionId,
      );
      if (index < 0)
        return fail(
          failure("exception attention item does not exist", "human approval"),
        );
      const current = state.attentions[index];
      if (current?.approval !== undefined)
        return fail(
          failure(
            "exception attention item is already approved",
            "human approval",
          ),
        );
      const updated = [...state.attentions];
      updated[index] = { ...current, approval } as StoredAttention;
      return succeed({
        state: {
          attentions: updated,
          events: [
            ...state.events,
            {
              kind: "exception-approved",
              identity: approval.attentionId,
              recordedAt: approval.approvedAt,
              humanIdentity: approval.humanIdentity,
              expiresAt: approval.expiresAt,
            },
          ],
        },
        value: approval,
      });
    });
  }

  async consume(
    claim: ExceptionBlockerClaim,
    now: ExceptionExecutionTime,
  ): Promise<Result<ExceptionExecutionAttempt, ExceptionFailure>> {
    return this.#withLock((state) => {
      const claimDigest = hash(claim);
      const index = state.attentions.findIndex(
        (item) => item.attention.claimDigest === claimDigest,
      );
      if (index < 0)
        return fail(
          failure(
            "operation does not exactly match an escalated blocker claim",
            "exception execution",
          ),
        );
      const current = state.attentions[index];
      if (current?.approval === undefined)
        return fail(
          failure("exact human approval is required", "exception execution"),
        );
      if (current.attemptId !== undefined)
        return fail(
          failure(
            "human exception capability has already been consumed",
            "exception replay",
          ),
        );
      if (
        Date.parse(now) < Date.parse(current.approval.approvedAt) ||
        Date.parse(now) >= Date.parse(current.approval.expiresAt)
      )
        return fail(
          failure("human exception capability has expired", "exception expiry"),
        );
      const attemptId = randomUUID();
      const updated = [...state.attentions];
      updated[index] = { ...current, attemptId };
      return succeed({
        state: {
          attentions: updated,
          events: [
            ...state.events,
            {
              kind: "exception-consumed",
              identity: attemptId,
              recordedAt: now,
              attentionId: current.attention.attentionId,
              claimDigest,
            },
          ],
        },
        value: { attemptId, claim: current.claim },
      });
    });
  }

  async recordObservation(
    observation: ExceptionExecutionObservation,
  ): Promise<Result<ExceptionExecutionObservation, ExceptionFailure>> {
    return this.#withLock((state) => {
      const current = state.attentions.find(
        (item) => item.attemptId === observation.attemptId,
      );
      if (current === undefined)
        return fail(
          failure("exception attempt does not exist", "exception observation"),
        );
      if (
        state.events.some(
          (event) =>
            event.kind === "exception-observed" &&
            event.identity === observation.attemptId,
        )
      )
        return fail(
          failure(
            "exception attempt observation is already recorded",
            "exception observation replay",
          ),
        );
      return succeed({
        state: {
          ...state,
          events: [
            ...state.events,
            {
              kind: "exception-observed",
              identity: observation.attemptId,
              recordedAt: observation.observedAt,
              exitCode: observation.exitCode,
              stdoutDigest: observation.stdoutDigest,
              stderrDigest: observation.stderrDigest,
            },
          ],
        },
        value: observation,
      });
    });
  }
}
