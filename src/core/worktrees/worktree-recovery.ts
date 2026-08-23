const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SHA = /^[0-9a-f]{40,64}$/u;
const BRANCH = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$/u;
const RECOVERY_REF = /^refs\/tiber\/recovery\/[A-Za-z0-9._/-]{1,180}$/u;

export interface OwnedWorktree {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly claimId: string;
  readonly branch: string;
  readonly path: string;
  readonly baselineRevision: string;
  readonly heartbeatAt: string;
}

export interface OwnedWorktreeRegistry {
  readonly schemaVersion: 1;
  readonly worktrees: readonly OwnedWorktree[];
}

interface RegistryFailure {
  readonly code: "TIBER_WORKTREE_REGISTRY_INVALID";
  readonly message: string;
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
    value.schemaVersion !== 1 ||
    // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects non-string JSON values and this guard narrows the type.
    typeof value.taskId !== "string" ||
    !UUID.test(value.taskId) ||
    // Stryker disable next-line ConditionalExpression: the following UUID grammar rejects non-string JSON values and this guard narrows the type.
    typeof value.claimId !== "string" ||
    !UUID.test(value.claimId) ||
    typeof value.branch !== "string" ||
    !BRANCH.test(value.branch) ||
    typeof value.path !== "string" ||
    !value.path.startsWith("/") ||
    value.path.includes("\0") ||
    // Stryker disable next-line ConditionalExpression: the following SHA grammar rejects non-string JSON values and this guard narrows the type.
    typeof value.baselineRevision !== "string" ||
    !SHA.test(value.baselineRevision) ||
    typeof value.heartbeatAt !== "string" ||
    !Number.isFinite(Date.parse(value.heartbeatAt))
  )
    return undefined;
  return {
    schemaVersion: 1,
    taskId: value.taskId,
    claimId: value.claimId,
    branch: value.branch,
    path: value.path,
    baselineRevision: value.baselineRevision,
    heartbeatAt: new Date(value.heartbeatAt).toISOString(),
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
      failure: {
        code: "TIBER_WORKTREE_REGISTRY_INVALID",
        message: "owned worktree registry is malformed or exceeds its quota",
      },
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
      failure: {
        code: "TIBER_WORKTREE_REGISTRY_INVALID",
        message: "owned worktree entries are malformed or ambiguous",
      },
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
  readonly path: string;
  readonly canonicalWithinRoot: boolean;
  readonly gitRegistered: boolean;
  readonly branch: string | undefined;
  readonly claimId: string | undefined;
  readonly processGroupAlive: boolean;
}

export function reconcileOwnedWorktrees(
  owned: readonly OwnedWorktree[],
  observations: readonly WorktreeObservation[],
): {
  readonly resumable: readonly OwnedWorktree[];
  readonly blocked: readonly OwnedWorktree[];
  readonly staleProcessGroups: readonly string[];
} {
  const resumable: OwnedWorktree[] = [];
  const blocked: OwnedWorktree[] = [];
  const staleProcessGroups: string[] = [];
  for (const entry of owned) {
    const observation = observations.find(
      (candidate) => candidate.path === entry.path,
    );
    if (
      observation?.canonicalWithinRoot === true &&
      observation.gitRegistered &&
      observation.branch === entry.branch &&
      observation.claimId === entry.claimId
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
  readonly branch: string | undefined;
  readonly claimActive: boolean;
  readonly dirtySource: boolean;
  readonly recoveryRef: string;
}

export type WorktreeCleanupEffect =
  | {
      readonly kind: "create-recovery-ref";
      readonly path: string;
      readonly ref: string;
    }
  | { readonly kind: "remove-owned-worktree"; readonly path: string }
  | { readonly kind: "remove-registry-entry"; readonly taskId: string };

export function decideWorktreeAbandonment(
  owned: OwnedWorktree,
  observation: AbandonmentObservation,
):
  | { readonly ok: true; readonly effects: readonly WorktreeCleanupEffect[] }
  | { readonly ok: false; readonly code: "TIBER_WORKTREE_CLEANUP_DENIED" } {
  if (
    !observation.canonicalWithinRoot ||
    !observation.gitRegistered ||
    observation.branch !== owned.branch ||
    observation.claimActive ||
    (observation.dirtySource && !RECOVERY_REF.test(observation.recoveryRef))
  )
    return { ok: false, code: "TIBER_WORKTREE_CLEANUP_DENIED" };
  return {
    ok: true,
    effects: [
      ...(observation.dirtySource
        ? ([
            {
              kind: "create-recovery-ref",
              path: owned.path,
              ref: observation.recoveryRef,
            },
          ] as const)
        : []),
      { kind: "remove-owned-worktree", path: owned.path },
      { kind: "remove-registry-entry", taskId: owned.taskId },
    ],
  };
}
