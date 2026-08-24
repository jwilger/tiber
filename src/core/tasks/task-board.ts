import {
  parseDeliveryCommitRevision,
  parseDeliveryDestinationRef,
  parseDeliveryTreeDigest,
} from "../delivery/git-delivery-values.js";
import {
  validateGitDeliveryReceipt,
  type GitDeliveryMode,
  type GitDeliveryReceipt,
} from "../delivery/git-delivery.js";
import {
  parseCommandCatalogDigest,
  parseCommandName,
  type CommandCatalogDigest,
  type CommandName,
} from "../commands/command-values.js";
import {
  parseClaimBaselineRevision,
  parseClaimOwnerIdentity,
  parseSpecificationDigest,
  parseScenarioName,
  parseSpecificationReviewFindingCount,
  parseTaskClaimId,
  parseTaskDescription,
  parseTaskEventId,
  parseTaskEventOccurredAt,
  parseTaskId,
  parseTaskTitle,
  parseTestMappingPath,
  type ClaimBaselineRevision,
  type ClaimOwnerIdentity,
  type ScenarioName,
  type SpecificationDigest,
  type TaskClaimId,
  type TaskDescription,
  type TaskEventId,
  type TaskEventOccurredAt,
  type TaskId,
  type TaskTitle,
  type TestMappingPath,
} from "./task-values.js";
import {
  parseCompiledWorkflowDigest,
  parseFinalReviewFindingCount,
  parseFinalReviewRationale,
  parseGreenDiagnosticDigest,
  parseIncrementReviewRationale,
  parseRedDiagnosticDigest,
  parseSourceDiffDigest,
  parseSourceSnapshotDigest,
  parseVerificationDiagnosticDigest,
  type CompiledWorkflowDigest,
  type GreenDiagnosticDigest,
  type IncrementReviewRationale,
  type RedDiagnosticDigest,
  type SourceDiffDigest,
  type SourceSnapshotDigest,
} from "../workflow/workflow-values.js";
import type { TiberFailure, TiberResult } from "../failures/tiber-failure.js";
import {
  advanceFinalReview,
  decideScopeCompletion,
  finalReviewRiskSignals,
  selectFinalReviewLenses,
  type AcceptanceVerificationReceipt,
  type FinalReviewIteration,
  type FinalReviewLens,
  type FinalReviewProgress,
} from "../workflow/final-review.js";
import { none, some, type Option } from "../types/option.js";
import {
  decideReadiness,
  digestTaskSpecification,
  parseTaskSpecification,
  type ReadinessReview,
  type TaskSpecification,
} from "./readiness.js";

export type TaskState = "Backlog" | "Ready" | "In Progress" | "Done";
// Stryker disable next-line ArrayDeclaration: this constant is used only on lifecycle-invalid final review events, which are independently denied before it can influence authority.
const NO_FINAL_REVIEW_LENSES: readonly FinalReviewLens[] = [];
export type TaskBlockStatus = "blocked" | "unblocked";

export interface TaskClaim {
  readonly claimId: TaskClaimId;
  readonly owner: ClaimOwnerIdentity;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly workflowDigest: CompiledWorkflowDigest;
}

export interface PreservedIncrement {
  readonly scenarioName: ScenarioName;
  readonly testMapping: TestMappingPath;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly commandCatalogDigest: CommandCatalogDigest;
  readonly commandName: CommandName;
  readonly redDiagnosticDigest: RedDiagnosticDigest;
  readonly greenDiagnosticDigest: GreenDiagnosticDigest;
  readonly sourceDiffDigest: SourceDiffDigest;
  readonly reviewRationale: IncrementReviewRationale;
}

export interface Task {
  readonly id: TaskId;
  readonly title: TaskTitle;
  readonly description: TaskDescription;
  readonly state: TaskState;
  readonly blockStatus: TaskBlockStatus;
  readonly specification: Option<TaskSpecification>;
  readonly specificationDigest: Option<SpecificationDigest>;
  readonly claim: Option<TaskClaim>;
  readonly preservedIncrements: readonly PreservedIncrement[];
  readonly finalReviewProgress: Option<FinalReviewProgress>;
  readonly completionRelease: Option<TaskClaimId>;
  readonly delivery: Option<GitDeliveryReceipt>;
}

export interface TaskCreatedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-created";
  readonly occurredAt: TaskEventOccurredAt;
  readonly task: {
    readonly id: TaskId;
    readonly title: TaskTitle;
    readonly description: TaskDescription;
  };
}

export interface TaskSpecifiedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-specified";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly specification: TaskSpecification;
}

export interface TaskReadyEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-ready";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly review: ReadinessReview;
}

export interface TaskClaimedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claimed";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claim: TaskClaim;
}

export interface TaskClaimTakenOverEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claim-taken-over";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly previousClaimId: TaskClaimId;
  readonly claim: TaskClaim;
}

export interface TaskIncrementPreservedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-increment-preserved";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claimId: TaskClaimId;
  readonly increment: PreservedIncrement;
}

export interface TaskDeliveryRecordedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-delivery-recorded";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claimId: TaskClaimId;
  readonly receipt: GitDeliveryReceipt;
}

export interface TaskFinalReviewRecordedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-final-review-recorded";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly verification: AcceptanceVerificationReceipt;
  readonly iteration: FinalReviewIteration;
}

export interface TaskCompletedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-completed";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claimId: TaskClaimId;
  readonly sourceSnapshotDigest: SourceSnapshotDigest;
  readonly cleanup: {
    readonly processCleanupStatus: "clean";
    readonly worktreeCleanupStatus: "clean";
  };
}

export interface TaskClaimReleasedEvent {
  readonly schemaVersion: 1;
  readonly eventId: TaskEventId;
  readonly kind: "task-claim-released";
  readonly occurredAt: TaskEventOccurredAt;
  readonly taskId: TaskId;
  readonly specificationDigest: SpecificationDigest;
  readonly claimId: TaskClaimId;
  readonly reason: "baseline-drift" | "released" | "completed";
}

export type TaskEvent =
  | TaskCreatedEvent
  | TaskSpecifiedEvent
  | TaskReadyEvent
  | TaskClaimedEvent
  | TaskClaimTakenOverEvent
  | TaskIncrementPreservedEvent
  | TaskDeliveryRecordedEvent
  | TaskFinalReviewRecordedEvent
  | TaskClaimReleasedEvent
  | TaskCompletedEvent;

export type TaskBoardFailureReason =
  | "duplicate-authority-event"
  | "incomplete-final-review"
  | "invalid-delivery-receipt"
  | "invalid-preserved-increment"
  | "invalid-task-completion"
  | "non-exclusive-claim"
  | "non-exact-claim-release"
  | "non-exact-claim-takeover"
  | "stale-readiness-review"
  | "task-history-verification"
  | "unknown-task";
export type TaskBoardFailure = TiberFailure<
  "TIBER_TASK_BOARD_INVALID",
  {
    readonly domain: "task-board";
    readonly reason: TaskBoardFailureReason;
  },
  "corrected-task-history"
>;

export function taskBoardFailure(
  reason: TaskBoardFailureReason,
  message: string,
): TaskBoardFailure {
  return {
    code: "TIBER_TASK_BOARD_INVALID",
    message,
    safeContext: { domain: "task-board", reason },
    causes: [],
    retryability: "retry-after-state-change",
    requiredRecoveryEvidence: ["corrected-task-history"],
    redaction: "public",
  };
}

export interface TaskBoard {
  readonly mode: "writable" | "degraded-read-only";
  readonly tasks: readonly Task[];
  readonly failure: Option<TaskBoardFailure>;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-null JSON primitives safely expose undefined required fields and are rejected by the shape parser; typeof establishes the TypeScript predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseTaskCreatedEventValue(
  value: unknown,
): TaskCreatedEvent | undefined {
  if (
    !isRecord(value) ||
    value.schemaVersion !== 1 ||
    value.kind !== "task-created" ||
    !isRecord(value.task)
  )
    return undefined;
  const eventId = parseTaskEventId(value.eventId);
  const occurredAt = parseTaskEventOccurredAt(value.occurredAt);
  const taskId = parseTaskId(value.task.id);
  const title = parseTaskTitle(
    typeof value.task.title === "string"
      ? value.task.title.trim()
      : value.task.title,
  );
  const description = parseTaskDescription(value.task.description);
  if (
    !eventId.ok ||
    !occurredAt.ok ||
    !taskId.ok ||
    !title.ok ||
    !description.ok
  )
    return undefined;
  return {
    schemaVersion: 1,
    eventId: eventId.value,
    kind: "task-created",
    occurredAt: occurredAt.value,
    task: {
      id: taskId.value,
      title: title.value,
      description: description.value,
    },
  };
}

function parseCommonEvent(value: Readonly<Record<string, unknown>>):
  | {
      readonly eventId: TaskEventId;
      readonly occurredAt: TaskEventOccurredAt;
    }
  | undefined {
  if (value.schemaVersion !== 1) return undefined;
  const eventId = parseTaskEventId(value.eventId);
  const occurredAt = parseTaskEventOccurredAt(value.occurredAt);
  return eventId.ok && occurredAt.ok
    ? { eventId: eventId.value, occurredAt: occurredAt.value }
    : undefined;
}

function parseTaskClaim(value: unknown): TaskClaim | undefined {
  if (!isRecord(value)) return undefined;
  const claimId = parseTaskClaimId(value.claimId);
  const owner = parseClaimOwnerIdentity(
    typeof value.owner === "string" ? value.owner.trim() : value.owner,
  );
  const baselineRevision = parseClaimBaselineRevision(value.baselineRevision);
  const workflowDigest = parseCompiledWorkflowDigest(value.workflowDigest);
  return claimId.ok && owner.ok && baselineRevision.ok && workflowDigest.ok
    ? {
        claimId: claimId.value,
        owner: owner.value,
        baselineRevision: baselineRevision.value,
        workflowDigest: workflowDigest.value,
      }
    : undefined;
}

function parseFinalReviewLens(value: unknown): FinalReviewLens | undefined {
  return value === "behavior" ||
    value === "architecture" ||
    value === "security" ||
    value === "operability"
    ? value
    : undefined;
}

function parseFinalReviewIteration(
  value: unknown,
): FinalReviewIteration | undefined {
  if (
    !isRecord(value) ||
    !Array.isArray(value.selectedLenses) ||
    !Array.isArray(value.reviews)
  )
    return undefined;
  const sourceSnapshotDigest = parseSourceSnapshotDigest(
    value.sourceSnapshotDigest,
  );
  const verificationDiagnosticDigest = parseVerificationDiagnosticDigest(
    value.verificationDiagnosticDigest,
  );
  const selectedLenses = value.selectedLenses.map(parseFinalReviewLens);
  if (
    !sourceSnapshotDigest.ok ||
    !verificationDiagnosticDigest.ok ||
    selectedLenses.some((lens) => lens === undefined)
  )
    return undefined;
  const reviews = value.reviews.map((review) => {
    if (!isRecord(review)) return undefined;
    const lens = parseFinalReviewLens(review.lens);
    const findingCount = parseFinalReviewFindingCount(review.findingCount);
    const rationale = parseFinalReviewRationale(review.rationale);
    return lens === undefined ||
      review.contextFreshness !== "fresh" ||
      !findingCount.ok ||
      !rationale.ok
      ? undefined
      : {
          lens,
          contextFreshness: "fresh" as const,
          findingCount: findingCount.value,
          rationale: rationale.value,
        };
  });
  if (reviews.some((review) => review === undefined)) return undefined;
  return {
    sourceSnapshotDigest: sourceSnapshotDigest.value,
    verificationDiagnosticDigest: verificationDiagnosticDigest.value,
    // Stryker disable next-line MethodExpression, ConditionalExpression: the preceding undefined guard proves every selected lens; filter is the TypeScript narrowing step only.
    selectedLenses: selectedLenses.filter((lens) => lens !== undefined),
    // Stryker disable next-line MethodExpression, ConditionalExpression: the preceding undefined guard proves every review; filter is the TypeScript narrowing step only.
    reviews: reviews.filter((review) => review !== undefined),
  };
}

function parseDeliveryMode(value: unknown): GitDeliveryMode | undefined {
  return value === "local-only" ||
    value === "branch-push" ||
    value === "direct" ||
    value === "review"
    ? value
    : undefined;
}

function parseTaskEventValue(value: unknown): TaskEvent | undefined {
  const created = parseTaskCreatedEventValue(value);
  if (created !== undefined) return created;
  if (!isRecord(value)) return undefined;
  const common = parseCommonEvent(value);
  if (common === undefined) return undefined;
  const taskId = parseTaskId(value.taskId);
  const specificationDigest = parseSpecificationDigest(
    value.specificationDigest,
  );
  if (!taskId.ok || !specificationDigest.ok) return undefined;
  if (value.kind === "task-specified") {
    const specification = parseTaskSpecification(value.specification);
    return !specification.ok ||
      digestTaskSpecification(specification.value) !== specificationDigest.value
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-specified",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          specification: specification.value,
        };
  }
  if (value.kind === "task-claimed") {
    const claim = parseTaskClaim(value.claim);
    return claim === undefined
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-claimed",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          claim,
        };
  }
  if (value.kind === "task-claim-taken-over") {
    const previousClaimId = parseTaskClaimId(value.previousClaimId);
    const claim = parseTaskClaim(value.claim);
    return !previousClaimId.ok || claim === undefined
      ? undefined
      : {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-claim-taken-over",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          previousClaimId: previousClaimId.value,
          claim,
        };
  }
  if (value.kind === "task-increment-preserved" && isRecord(value.increment)) {
    const claimId = parseTaskClaimId(value.claimId);
    const scenarioName = parseScenarioName(value.increment.scenarioName);
    const testMapping = parseTestMappingPath(value.increment.testMapping);
    const baselineRevision = parseClaimBaselineRevision(
      value.increment.baselineRevision,
    );
    const commandCatalogDigest = parseCommandCatalogDigest(
      value.increment.commandCatalogDigest,
    );
    const commandName = parseCommandName(value.increment.commandName);
    const redDiagnosticDigest = parseRedDiagnosticDigest(
      value.increment.redDiagnosticDigest,
    );
    const greenDiagnosticDigest = parseGreenDiagnosticDigest(
      value.increment.greenDiagnosticDigest,
    );
    const sourceDiffDigest = parseSourceDiffDigest(
      value.increment.sourceDiffDigest,
    );
    const reviewRationale = parseIncrementReviewRationale(
      value.increment.reviewRationale,
    );
    if (
      !claimId.ok ||
      !scenarioName.ok ||
      !testMapping.ok ||
      !baselineRevision.ok ||
      !commandCatalogDigest.ok ||
      !commandName.ok ||
      !redDiagnosticDigest.ok ||
      !greenDiagnosticDigest.ok ||
      !sourceDiffDigest.ok ||
      !reviewRationale.ok
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-increment-preserved",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      claimId: claimId.value,
      increment: {
        scenarioName: scenarioName.value,
        testMapping: testMapping.value,
        baselineRevision: baselineRevision.value,
        commandCatalogDigest: commandCatalogDigest.value,
        commandName: commandName.value,
        redDiagnosticDigest: redDiagnosticDigest.value,
        greenDiagnosticDigest: greenDiagnosticDigest.value,
        sourceDiffDigest: sourceDiffDigest.value,
        reviewRationale: reviewRationale.value,
      },
    };
  }
  if (value.kind === "task-delivery-recorded" && isRecord(value.receipt)) {
    const claimId = parseTaskClaimId(value.claimId);
    const mode = parseDeliveryMode(value.receipt.mode);
    const baselineRevision = parseClaimBaselineRevision(
      value.receipt.baselineRevision,
    );
    const commit = parseDeliveryCommitRevision(value.receipt.commit);
    const tree = parseDeliveryTreeDigest(value.receipt.tree);
    const sourceSnapshotDigest = parseSourceSnapshotDigest(
      value.receipt.sourceSnapshotDigest,
    );
    const destinationValue = value.receipt.destination;
    const remoteValue = value.receipt.observedRemoteCommit;
    const destination =
      isRecord(destinationValue) && destinationValue.kind === "none"
        ? none
        : isRecord(destinationValue) && destinationValue.kind === "some"
          ? parseDeliveryDestinationRef(destinationValue.value)
          : undefined;
    const observedRemoteCommit =
      isRecord(remoteValue) && remoteValue.kind === "none"
        ? none
        : isRecord(remoteValue) && remoteValue.kind === "some"
          ? parseDeliveryCommitRevision(remoteValue.value)
          : undefined;
    if (
      !claimId.ok ||
      mode === undefined ||
      !baselineRevision.ok ||
      !commit.ok ||
      !tree.ok ||
      !sourceSnapshotDigest.ok ||
      destination === undefined ||
      observedRemoteCommit === undefined ||
      ("ok" in destination && !destination.ok) ||
      // Stryker disable next-line ConditionalExpression, StringLiteral: malformed remote revisions are independently rejected by exact receipt validation below; this check preserves boundary narrowing.
      ("ok" in observedRemoteCommit && !observedRemoteCommit.ok)
    )
      return undefined;
    const receipt: GitDeliveryReceipt = {
      mode,
      baselineRevision: baselineRevision.value,
      commit: commit.value,
      tree: tree.value,
      sourceSnapshotDigest: sourceSnapshotDigest.value,
      destination: "ok" in destination ? some(destination.value) : destination,
      observedRemoteCommit:
        "ok" in observedRemoteCommit
          ? some(observedRemoteCommit.value)
          : observedRemoteCommit,
    };
    return validateGitDeliveryReceipt(receipt).status === "authorized"
      ? {
          schemaVersion: 1,
          eventId: common.eventId,
          kind: "task-delivery-recorded",
          occurredAt: common.occurredAt,
          taskId: taskId.value,
          specificationDigest: specificationDigest.value,
          claimId: claimId.value,
          receipt,
        }
      : undefined;
  }
  if (
    value.kind === "task-final-review-recorded" &&
    isRecord(value.verification)
  ) {
    const claimId = parseTaskClaimId(value.verification.claimId);
    const commandCatalogDigest = parseCommandCatalogDigest(
      value.verification.commandCatalogDigest,
    );
    const diagnosticDigest = parseVerificationDiagnosticDigest(
      value.verification.diagnosticDigest,
    );
    const sourceSnapshotDigest = parseSourceSnapshotDigest(
      value.verification.sourceSnapshotDigest,
    );
    const iteration = parseFinalReviewIteration(value.iteration);
    if (
      !claimId.ok ||
      value.verification.specificationDigest !== specificationDigest.value ||
      !commandCatalogDigest.ok ||
      !diagnosticDigest.ok ||
      !sourceSnapshotDigest.ok ||
      iteration === undefined
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-final-review-recorded",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      verification: {
        claimId: claimId.value,
        specificationDigest: specificationDigest.value,
        commandCatalogDigest: commandCatalogDigest.value,
        diagnosticDigest: diagnosticDigest.value,
        sourceSnapshotDigest: sourceSnapshotDigest.value,
      },
      iteration,
    };
  }
  if (
    value.kind === "task-completed" &&
    isRecord(value.cleanup) &&
    value.cleanup.processCleanupStatus === "clean" &&
    value.cleanup.worktreeCleanupStatus === "clean"
  ) {
    const claimId = parseTaskClaimId(value.claimId);
    const sourceSnapshotDigest = parseSourceSnapshotDigest(
      value.sourceSnapshotDigest,
    );
    if (!claimId.ok || !sourceSnapshotDigest.ok) return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-completed",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      claimId: claimId.value,
      sourceSnapshotDigest: sourceSnapshotDigest.value,
      cleanup: {
        processCleanupStatus: "clean",
        worktreeCleanupStatus: "clean",
      },
    };
  }
  if (value.kind === "task-claim-released") {
    const claimId = parseTaskClaimId(value.claimId);
    if (
      !claimId.ok ||
      (value.reason !== "baseline-drift" &&
        value.reason !== "released" &&
        value.reason !== "completed")
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-claim-released",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      claimId: claimId.value,
      reason: value.reason,
    };
  }
  if (value.kind === "task-ready" && isRecord(value.review)) {
    const review = value.review;
    const findingCount = parseSpecificationReviewFindingCount(
      review.findingCount,
    );
    if (
      review.contextFreshness !== "fresh" ||
      review.reviewerRole !== "specification-reviewer" ||
      !findingCount.ok ||
      review.reviewedSpecificationDigest !== specificationDigest.value
    )
      return undefined;
    return {
      schemaVersion: 1,
      eventId: common.eventId,
      kind: "task-ready",
      occurredAt: common.occurredAt,
      taskId: taskId.value,
      specificationDigest: specificationDigest.value,
      review: {
        contextFreshness: "fresh",
        reviewerRole: "specification-reviewer",
        findingCount: findingCount.value,
        reviewedSpecificationDigest: specificationDigest.value,
      },
    };
  }
  return undefined;
}

type TaskEventParseFailure = TiberFailure<
  "TIBER_TASK_EVENT_INVALID",
  { readonly boundary: "task-event" },
  "corrected-task-event"
>;

export function parseTaskEvent(
  value: unknown,
): TiberResult<TaskEvent, TaskEventParseFailure> {
  const event = parseTaskEventValue(value);
  return event === undefined
    ? {
        ok: false,
        failure: {
          code: "TIBER_TASK_EVENT_INVALID",
          message:
            "Task event is malformed or violates its semantic invariants",
          safeContext: { boundary: "task-event" },
          causes: [],
          retryability: "retry-after-input",
          requiredRecoveryEvidence: ["corrected-task-event"],
          redaction: "public",
        },
      }
    : { ok: true, value: event };
}

export function foldTaskEvents(events: readonly TaskEvent[]): TaskBoard {
  const tasks = new Map<string, Task>();
  const eventIds = new Set<string>();
  for (const event of events) {
    if (eventIds.has(event.eventId)) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "duplicate-authority-event",
            "duplicate task authority event",
          ),
        ),
      };
    }
    eventIds.add(event.eventId);
    if (event.kind === "task-created") {
      if (tasks.has(event.task.id)) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "duplicate-authority-event",
              "duplicate task authority event",
            ),
          ),
        };
      }
      tasks.set(event.task.id, {
        id: event.task.id,
        title: event.task.title,
        description: event.task.description,
        state: "Backlog",
        blockStatus: "unblocked",
        specification: none,
        specificationDigest: none,
        claim: none,
        preservedIncrements: [],
        finalReviewProgress: none,
        completionRelease: none,
        delivery: none,
      });
      continue;
    }
    const task = tasks.get(event.taskId);
    if (task === undefined) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "unknown-task",
            "task event references an unknown task",
          ),
        ),
      };
    }
    if (event.kind === "task-specified") {
      tasks.set(task.id, {
        ...task,
        specification: some(event.specification),
        specificationDigest: some(event.specificationDigest),
        preservedIncrements: [],
        finalReviewProgress: none,
        completionRelease: none,
        delivery: none,
      });
      continue;
    }
    if (event.kind === "task-claimed") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so either condition independently detects an existing claim; both document the closed state invariant.
        task.state !== "Ready" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: claim and In Progress state are installed atomically, so the state check already detects every existing claim; this check documents exclusivity directly.
        task.claim.kind === "some" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exclusive-claim",
              "task claim is not exclusive or state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        state: "In Progress",
        claim: some(event.claim),
        finalReviewProgress: none,
        completionRelease: none,
        delivery: none,
      });
      continue;
    }
    if (event.kind === "task-claim-taken-over") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically, so exact claim identity independently establishes this state; both checks document the invariant.
        task.state !== "In Progress" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: claim and In Progress state are installed atomically; the state check already establishes presence.
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.previousClaimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        task.claim.value.baselineRevision !== event.claim.baselineRevision ||
        task.claim.value.workflowDigest !== event.claim.workflowDigest ||
        event.claim.claimId === event.previousClaimId
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exact-claim-takeover",
              "task claim takeover is not exact or state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        claim: some(event.claim),
        finalReviewProgress: none,
        completionRelease: none,
        delivery: none,
      });
      continue;
    }
    if (event.kind === "task-increment-preserved") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: claim and In Progress state are installed atomically; either check independently establishes the same lifecycle invariant.
        task.state !== "In Progress" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: claim and In Progress state are installed atomically; the state check already establishes presence.
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.claimId ||
        task.claim.value.baselineRevision !==
          event.increment.baselineRevision ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: an exact active claim is only installed after specification; this check documents the established Option invariant.
        task.specification.kind === "none" ||
        !task.specification.value.scenarios.some(
          (scenario) => scenario.name === event.increment.scenarioName,
        ) ||
        !task.specification.value.testMappings.some(
          (mapping) => mapping === event.increment.testMapping,
        ) ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        task.preservedIncrements.some(
          (increment) =>
            increment.scenarioName === event.increment.scenarioName,
        )
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "invalid-preserved-increment",
              "preserved increment is not unique or state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        preservedIncrements: [...task.preservedIncrements, event.increment],
        finalReviewProgress: none,
        completionRelease: none,
        delivery: none,
      });
      continue;
    }
    if (event.kind === "task-delivery-recorded") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: In Progress atomically establishes active claim and specification digest; exact identities remain independently checked below.
        task.state !== "In Progress" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: In Progress atomically establishes claim presence; this check preserves explicit Option narrowing.
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.claimId ||
        task.claim.value.baselineRevision !== event.receipt.baselineRevision ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: In Progress atomically establishes specification digest presence; this check preserves explicit Option narrowing.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        task.finalReviewProgress.kind === "none" ||
        task.finalReviewProgress.value.cleanStreak !== 3 ||
        task.finalReviewProgress.value.sourceSnapshotDigest !==
          event.receipt.sourceSnapshotDigest ||
        task.delivery.kind === "some"
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "invalid-delivery-receipt",
              "delivery receipt is duplicate, stale, or not state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, { ...task, delivery: some(event.receipt) });
      continue;
    }
    if (event.kind === "task-final-review-recorded") {
      const expectedLenses =
        // Stryker disable next-line ConditionalExpression, ArrayDeclaration: In Progress is installed only with a parsed specification, so the absent branch is unreachable but preserves explicit Option narrowing.
        task.specification.kind === "some"
          ? selectFinalReviewLenses(
              finalReviewRiskSignals(task.specification.value),
            )
          : NO_FINAL_REVIEW_LENSES;
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: In Progress, active claim, specification, and digest are installed atomically; exact claim identity below remains independently authoritative.
        task.state !== "In Progress" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: In Progress atomically establishes claim presence; this check preserves explicit Option narrowing.
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.verification.claimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: In Progress atomically establishes specification presence; this check preserves explicit Option narrowing.
        task.specification.kind === "none" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: In Progress atomically establishes digest presence; this check preserves explicit Option narrowing.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        // Stryker disable next-line ConditionalExpression: the task-event parser has already required these two signed digest fields to be identical.
        event.verification.specificationDigest !== event.specificationDigest ||
        event.verification.sourceSnapshotDigest !==
          event.iteration.sourceSnapshotDigest ||
        event.verification.diagnosticDigest !==
          event.iteration.verificationDiagnosticDigest ||
        expectedLenses.length !== event.iteration.selectedLenses.length ||
        !expectedLenses.every(
          (lens, index) => lens === event.iteration.selectedLenses[index],
        ) ||
        decideScopeCompletion(
          task.specification.value,
          task.preservedIncrements,
        ).status !== "complete"
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "incomplete-final-review",
              "final review is incomplete, stale, or not state-bound",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        finalReviewProgress: some(
          advanceFinalReview(task.finalReviewProgress, event.iteration),
        ),
      });
      continue;
    }
    if (event.kind === "task-claim-released") {
      if (
        task.claim.kind === "none" ||
        task.claim.value.claimId !== event.claimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: active claims are installed only after specification and digest, so these checks document established invariants.
        task.specification.kind === "none" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: active claims are installed only after specification and digest, so these checks document established invariants.
        task.specificationDigest.kind === "none" ||
        (event.reason === "completed" &&
          (task.finalReviewProgress.kind === "none" ||
            task.finalReviewProgress.value.cleanStreak !== 3))
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "non-exact-claim-release",
              "task claim release does not match the active claim",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        id: task.id,
        title: task.title,
        description: task.description,
        state: "Ready",
        blockStatus: "unblocked",
        specification: task.specification,
        specificationDigest: task.specificationDigest,
        claim: none,
        preservedIncrements: task.preservedIncrements,
        finalReviewProgress:
          event.reason === "completed" ? task.finalReviewProgress : none,
        completionRelease:
          event.reason === "completed" ? some(event.claimId) : none,
        delivery: task.delivery,
      });
      continue;
    }
    if (event.kind === "task-completed") {
      if (
        // Stryker disable next-line ConditionalExpression, LogicalOperator: a completion release atomically establishes Ready with no claim, retained digest, exact release id, and a three-clean-review receipt.
        task.state !== "Ready" ||
        // Stryker disable next-line ConditionalExpression: a completion release atomically clears the active claim.
        task.claim.kind !== "none" ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: a completion release atomically retains the specification digest; this check preserves explicit Option narrowing.
        task.specificationDigest.kind === "none" ||
        task.specificationDigest.value !== event.specificationDigest ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: Ready-with-completion authority is installed atomically with this Option; exact identity is checked next.
        task.completionRelease.kind === "none" ||
        task.completionRelease.value !== event.claimId ||
        // Stryker disable next-line ConditionalExpression, StringLiteral: completion release is authorized only from a present three-clean-review receipt; this check preserves explicit Option narrowing.
        task.finalReviewProgress.kind === "none" ||
        // Stryker disable next-line ConditionalExpression: completion release cannot be installed unless the retained review streak is exactly three.
        task.finalReviewProgress.value.cleanStreak !== 3 ||
        task.finalReviewProgress.value.sourceSnapshotDigest !==
          event.sourceSnapshotDigest
      ) {
        return {
          mode: "degraded-read-only",
          tasks: [...tasks.values()],
          failure: some(
            taskBoardFailure(
              "invalid-task-completion",
              "task completion lacks exact review, release, or cleanup evidence",
            ),
          ),
        };
      }
      tasks.set(task.id, {
        ...task,
        state: "Done",
        completionRelease: none,
      });
      continue;
    }
    if (
      // Stryker disable next-line ConditionalExpression, LogicalOperator, StringLiteral: specification and digest are installed atomically by the only preceding event, so digest absence/mismatch already denies every state where specification is absent.
      task.specification.kind === "none" ||
      // Stryker disable next-line ConditionalExpression, StringLiteral: digest absence yields undefined and exact comparison below independently rejects it; the kind check documents the Option rail.
      task.specificationDigest.kind === "none" ||
      // Stryker disable next-line ConditionalExpression: the parsed Ready review is bound to the event digest, so decideReadiness below independently rejects every event/task digest mismatch as stale.
      task.specificationDigest.value !== event.specificationDigest ||
      decideReadiness(task.specificationDigest.value, event.review).status !==
        "ready"
    ) {
      return {
        mode: "degraded-read-only",
        tasks: [...tasks.values()],
        failure: some(
          taskBoardFailure(
            "stale-readiness-review",
            "Ready event lacks an exact clean specification review",
          ),
        ),
      };
    }
    tasks.set(task.id, { ...task, state: "Ready" });
  }
  return {
    mode: "writable",
    tasks: [...tasks.values()].sort((left, right) =>
      left.id.localeCompare(right.id),
    ),
    failure: none,
  };
}

export function formatTaskBoard(board: TaskBoard): string {
  const rows = board.tasks.map(
    (task) =>
      `${task.state}${task.blockStatus === "blocked" ? " [Blocked]" : ""} | ${task.id} | ${task.title}`,
  );
  return [
    `Task board: ${board.mode}`,
    ...(board.failure.kind === "none"
      ? []
      : [`Failure: ${board.failure.value.message}`]),
    "State | ID | Title",
    ...(rows.length === 0 ? ["(no tasks)"] : rows),
  ].join("\n");
}
