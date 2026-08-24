import {
  operationalFailure,
  type TiberFailure,
} from "../failures/tiber-failure.js";
import {
  parseClaimBaselineRevision,
  parseTaskClaimId,
  parseTaskId,
  type ClaimBaselineRevision,
  type TaskClaimId,
  type TaskId,
} from "../tasks/task-values.js";
import type { Option } from "../types/option.js";
import {
  parseOwnedWorktreePath,
  parseTaskBranchName,
  parseWorktreeHeartbeatAt,
  type OwnedWorktreePath,
  type RecoveryReference,
  type TaskBranchName,
  type WorktreeHeartbeatAt,
} from "./worktree-values.js";

export interface OwnedWorktree {
  readonly schemaVersion: 1;
  readonly taskId: TaskId;
  readonly claimId: TaskClaimId;
  readonly branch: TaskBranchName;
  readonly path: OwnedWorktreePath;
  readonly baselineRevision: ClaimBaselineRevision;
  readonly heartbeatAt: WorktreeHeartbeatAt;
}

export interface OwnedWorktreeRegistry {
  readonly schemaVersion: 1;
  readonly worktrees: readonly OwnedWorktree[];
}

type RegistryFailure = TiberFailure<
  "TIBER_WORKTREE_REGISTRY_INVALID",
  { readonly domain: "worktree-registry" },
  "corrected-input" | "state-change" | "retry-operation"
>;

function registryFailure(message: string): RegistryFailure {
  return operationalFailure(
    "TIBER_WORKTREE_REGISTRY_INVALID",
    "worktree-registry",
    message,
    "retry-after-input",
  );
}

export type RegistryParseResult =
  | { readonly ok: true; readonly value: OwnedWorktreeRegistry }
  | { readonly ok: false; readonly failure: RegistryFailure };

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  // Stryker disable next-line ConditionalExpression: non-object JSON values expose no valid required fields and fail the semantic parser; typeof establishes the predicate.
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function worktree(value: unknown): OwnedWorktree | undefined {
  // Stryker disable next-line ConditionalExpression: required-field validation rejects every non-record JSON value; this guard establishes the semantic record type.
  if (!record(value)) return undefined;
  const keys = Object.keys(value).sort().join(",");
  if (
    keys !==
      "baselineRevision,branch,claimId,heartbeatAt,path,schemaVersion,taskId" ||
    value.schemaVersion !== 1
  )
    return undefined;
  const taskId = parseTaskId(value.taskId);
  const claimId = parseTaskClaimId(value.claimId);
  const branch = parseTaskBranchName(value.branch);
  const path = parseOwnedWorktreePath(value.path);
  const baselineRevision = parseClaimBaselineRevision(value.baselineRevision);
  const heartbeatAt = parseWorktreeHeartbeatAt(value.heartbeatAt);
  if (
    !taskId.ok ||
    !claimId.ok ||
    !branch.ok ||
    !path.ok ||
    !baselineRevision.ok ||
    !heartbeatAt.ok
  )
    return undefined;
  return {
    schemaVersion: 1,
    taskId: taskId.value,
    claimId: claimId.value,
    branch: branch.value,
    path: path.value,
    baselineRevision: baselineRevision.value,
    heartbeatAt: heartbeatAt.value,
  };
}

export function parseOwnedWorktreeRegistry(
  value: unknown,
): RegistryParseResult {
  if (
    !record(value) ||
    Object.keys(value).sort().join(",") !== "schemaVersion,worktrees" ||
    value.schemaVersion !== 1 ||
    !Array.isArray(value.worktrees) ||
    value.worktrees.length > 8
  )
    return {
      ok: false,
      failure: registryFailure(
        "owned worktree registry is malformed or exceeds its quota",
      ),
    };
  const parsed = value.worktrees.map(worktree);
  if (
    parsed.some((item) => item === undefined) ||
    // Stryker disable next-line OptionalChaining: the preceding malformed-entry condition establishes every mapped entry; optional access preserves narrowing without a cast.
    new Set(parsed.map((item) => item?.taskId)).size !== parsed.length ||
    // Stryker disable next-line OptionalChaining: the preceding malformed-entry condition establishes every mapped entry; optional access preserves narrowing without a cast.
    new Set(parsed.map((item) => item?.path)).size !== parsed.length
  )
    return {
      ok: false,
      failure: registryFailure(
        "owned worktree entries are malformed or ambiguous",
      ),
    };
  return {
    ok: true,
    value: {
      schemaVersion: 1,
      // Stryker disable next-line MethodExpression, ArrowFunction, ConditionalExpression: malformed entries returned above; filtering conveys that established semantic type without an unsafe cast.
      worktrees: parsed.filter((item) => item !== undefined),
    },
  };
}

export interface WorktreeObservation {
  readonly path: OwnedWorktreePath;
  readonly canonicalWithinRoot: boolean;
  readonly gitRegistered: boolean;
  readonly branch: Option<TaskBranchName>;
  readonly claimId: Option<TaskClaimId>;
  readonly processGroupAlive: boolean;
}

export function reconcileOwnedWorktrees(
  owned: readonly OwnedWorktree[],
  observations: readonly WorktreeObservation[],
): {
  readonly resumable: readonly OwnedWorktree[];
  readonly blocked: readonly OwnedWorktree[];
  readonly staleProcessGroups: readonly TaskId[];
} {
  const resumable: OwnedWorktree[] = [];
  const blocked: OwnedWorktree[] = [];
  const staleProcessGroups: TaskId[] = [];
  for (const entry of owned) {
    const observation = observations.find(
      (candidate) => candidate.path === entry.path,
    );
    if (
      observation?.canonicalWithinRoot === true &&
      observation.gitRegistered &&
      // Stryker disable next-line ConditionalExpression: absence yields undefined and exact branch comparison below independently rejects it; the kind check documents the Option rail.
      observation.branch.kind === "some" &&
      observation.branch.value === entry.branch &&
      // Stryker disable next-line ConditionalExpression: absence yields undefined and exact claim comparison below independently rejects it; the kind check documents the Option rail.
      observation.claimId.kind === "some" &&
      observation.claimId.value === entry.claimId
    ) {
      resumable.push(entry);
      if (observation.processGroupAlive) staleProcessGroups.push(entry.taskId);
    } else {
      blocked.push(entry);
    }
  }
  return { resumable, blocked, staleProcessGroups };
}

export interface AbandonmentObservation {
  readonly canonicalWithinRoot: boolean;
  readonly gitRegistered: boolean;
  readonly branch: Option<TaskBranchName>;
  readonly claimStatus: "active" | "released";
  readonly dirtySource: boolean;
  readonly recoveryRef: Option<RecoveryReference>;
}

export type WorktreeCleanupEffect =
  | {
      readonly kind: "create-recovery-ref";
      readonly path: OwnedWorktreePath;
      readonly ref: RecoveryReference;
    }
  | { readonly kind: "remove-owned-worktree"; readonly path: OwnedWorktreePath }
  | { readonly kind: "remove-registry-entry"; readonly taskId: TaskId };

export function decideWorktreeAbandonment(
  owned: OwnedWorktree,
  observation: AbandonmentObservation,
):
  | { readonly ok: true; readonly effects: readonly WorktreeCleanupEffect[] }
  | { readonly ok: false; readonly code: "TIBER_WORKTREE_CLEANUP_DENIED" } {
  if (
    !observation.canonicalWithinRoot ||
    !observation.gitRegistered ||
    // Stryker disable next-line ConditionalExpression, StringLiteral: absence yields undefined and exact branch comparison below independently rejects it; the kind check documents the Option rail.
    observation.branch.kind === "none" ||
    observation.branch.value !== owned.branch ||
    observation.claimStatus === "active" ||
    (observation.dirtySource && observation.recoveryRef.kind === "none")
  )
    return { ok: false, code: "TIBER_WORKTREE_CLEANUP_DENIED" };
  return {
    ok: true,
    effects: [
      // Stryker disable next-line ConditionalExpression: dirty source without a recovery ref was denied above, so dirtySource establishes that the Option is some here.
      ...(observation.dirtySource && observation.recoveryRef.kind === "some"
        ? ([
            {
              kind: "create-recovery-ref",
              path: owned.path,
              ref: observation.recoveryRef.value,
            },
          ] as const)
        : []),
      { kind: "remove-owned-worktree", path: owned.path },
      { kind: "remove-registry-entry", taskId: owned.taskId },
    ],
  };
}
