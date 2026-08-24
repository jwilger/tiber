import type { Option } from "../types/option.js";
import type {
  CiAuthorityName,
  CiDiagnosis,
  CiExecutableDigest,
  CiObservationDigest,
  CiRevision,
} from "./ci-values.js";

export type CiTerminalStatus = "success" | "failure";
export type CiObservationStatus = CiTerminalStatus | "pending";

export interface CiAuthorityObservation {
  readonly authority: CiAuthorityName;
  readonly revision: CiRevision;
  readonly status: CiObservationStatus;
  readonly adapterDigest: CiExecutableDigest;
  readonly observationDigest: CiObservationDigest;
}

export interface CiSuccessReceipt {
  readonly revision: CiRevision;
  readonly requiredAuthorities: readonly CiAuthorityName[];
  readonly observations: readonly CiAuthorityObservation[];
}

export interface RepositoryCiHold {
  readonly failedRevision: CiRevision;
  readonly failedAuthorities: readonly CiAuthorityName[];
  readonly failureObservationDigest: CiObservationDigest;
}

export type CiEvaluationDecision =
  | { readonly status: "succeeded"; readonly receipt: CiSuccessReceipt }
  | {
      readonly status: "waiting";
      readonly code: "TIBER_CI_AUTHORITIES_INCOMPLETE";
      readonly pendingAuthorities: readonly CiAuthorityName[];
    }
  | {
      readonly status: "failed";
      readonly code: "TIBER_CI_TERMINAL_FAILURE";
      readonly hold: RepositoryCiHold;
    }
  | {
      readonly status: "denied";
      readonly code:
        "TIBER_CI_AUTHORITY_SET_INVALID" | "TIBER_CI_REVISION_MISMATCH";
    };

export function decideCiEvaluation(
  deliveredRevision: CiRevision,
  requiredAuthorities: readonly CiAuthorityName[],
  observations: readonly CiAuthorityObservation[],
): CiEvaluationDecision {
  const required = new Set(requiredAuthorities);
  const observed = new Set(observations.map(({ authority }) => authority));
  if (
    required.size !== requiredAuthorities.length ||
    observed.size !== observations.length ||
    observations.some(({ authority }) => !required.has(authority))
  )
    return { status: "denied", code: "TIBER_CI_AUTHORITY_SET_INVALID" };
  if (observations.some(({ revision }) => revision !== deliveredRevision))
    return { status: "denied", code: "TIBER_CI_REVISION_MISMATCH" };

  const byAuthority = new Map(
    observations.map((observation) => [observation.authority, observation]),
  );
  const failed = requiredAuthorities.filter(
    (authority) => byAuthority.get(authority)?.status === "failure",
  );
  if (failed.length > 0) {
    const firstFailure = observations.find(
      ({ authority }) => authority === failed[0],
    );
    // Stryker disable next-line ConditionalExpression: failed is derived from this same observation map, so absence is an internal invariant defect.
    if (firstFailure === undefined)
      // Stryker disable next-line CallExpression, StringLiteral: this unreachable defect throw cannot influence domain authority.
      throw new Error("failed CI authority disappeared");
    return {
      status: "failed",
      code: "TIBER_CI_TERMINAL_FAILURE",
      hold: {
        failedRevision: deliveredRevision,
        failedAuthorities: failed,
        failureObservationDigest: firstFailure.observationDigest,
      },
    };
  }

  const pending = requiredAuthorities.filter(
    (authority) => byAuthority.get(authority)?.status !== "success",
  );
  return pending.length > 0
    ? {
        status: "waiting",
        code: "TIBER_CI_AUTHORITIES_INCOMPLETE",
        pendingAuthorities: pending,
      }
    : {
        status: "succeeded",
        receipt: {
          revision: deliveredRevision,
          requiredAuthorities,
          observations,
        },
      };
}

export function authorizeDeliveryDuringCiHold(
  hold: Option<RepositoryCiHold>,
):
  | { readonly status: "authorized" }
  | { readonly status: "denied"; readonly code: "TIBER_CI_REPOSITORY_HOLD" } {
  return hold.kind === "none"
    ? { status: "authorized" }
    : { status: "denied", code: "TIBER_CI_REPOSITORY_HOLD" };
}

export type CiHoldRecoveryDecision =
  | { readonly status: "recovered" }
  | {
      readonly status: "denied";
      readonly code:
        | "TIBER_CI_RECOVERY_REVISION_MISMATCH"
        | "TIBER_CI_RECOVERY_AUTHORITY_MISSING";
    };

export function decideCiHoldRecovery(
  hold: RepositoryCiHold,
  _diagnosis: CiDiagnosis,
  receipt: CiSuccessReceipt,
): CiHoldRecoveryDecision {
  if (receipt.revision !== hold.failedRevision)
    return { status: "denied", code: "TIBER_CI_RECOVERY_REVISION_MISMATCH" };
  const required = new Set(receipt.requiredAuthorities);
  const successful = new Set(
    receipt.observations
      .filter(({ status }) => status === "success")
      .map(({ authority }) => authority),
  );
  return hold.failedAuthorities.every(
    (authority) => required.has(authority) && successful.has(authority),
  )
    ? { status: "recovered" }
    : { status: "denied", code: "TIBER_CI_RECOVERY_AUTHORITY_MISSING" };
}
