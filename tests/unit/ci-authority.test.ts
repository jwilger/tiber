import { describe, expect, it } from "vitest";

import {
  authorizeDeliveryDuringCiHold,
  decideCiEvaluation,
  decideCiHoldRecovery,
  type CiAuthorityObservation,
  type RepositoryCiHold,
} from "../../src/core/ci/ci-authority.js";
import { none, some } from "../../src/core/types/option.js";
import {
  parseCiAuthorityName,
  parseCiDiagnosis,
  parseCiExecutableDigest,
  parseCiObservationDigest,
  parseCiRevision,
} from "../../src/core/ci/ci-values.js";

function parsed<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  if (!result.ok) throw new Error("fixture invalid");
  return result.value;
}

const revision = parsed(parseCiRevision("a".repeat(40)));
const otherRevision = parsed(parseCiRevision("b".repeat(40)));
const quality = parsed(parseCiAuthorityName("quality"));
const acceptance = parsed(parseCiAuthorityName("acceptance"));
const digest = parsed(parseCiObservationDigest("c".repeat(64)));
const adapterDigest = parsed(parseCiExecutableDigest("e".repeat(64)));

function observation(
  authority: CiAuthorityObservation["authority"],
  status: CiAuthorityObservation["status"],
  observedRevision = revision,
): CiAuthorityObservation {
  return {
    authority,
    revision: observedRevision,
    status,
    adapterDigest,
    observationDigest: digest,
  };
}

describe("CI authority", () => {
  it("rejects success observed for a different revision", () => {
    expect(
      decideCiEvaluation(
        revision,
        [quality],
        [observation(quality, "success", otherRevision)],
      ),
    ).toEqual({ status: "denied", code: "TIBER_CI_REVISION_MISMATCH" });
  });

  it("requires terminal success from every required authority", () => {
    expect(
      decideCiEvaluation(
        revision,
        [quality, acceptance],
        [observation(quality, "success"), observation(acceptance, "pending")],
      ),
    ).toEqual({
      status: "waiting",
      code: "TIBER_CI_AUTHORITIES_INCOMPLETE",
      pendingAuthorities: [acceptance],
    });

    const decision = decideCiEvaluation(
      revision,
      [quality, acceptance],
      [observation(quality, "success"), observation(acceptance, "success")],
    );
    expect(decision.status).toBe("succeeded");
    if (decision.status === "succeeded")
      expect(decision.receipt.observations).toHaveLength(2);
  });

  it("rejects duplicate or unexpected authority sets", () => {
    expect(
      decideCiEvaluation(
        revision,
        [quality, quality],
        [observation(quality, "success")],
      ),
    ).toEqual({ status: "denied", code: "TIBER_CI_AUTHORITY_SET_INVALID" });
    expect(
      decideCiEvaluation(
        revision,
        [quality],
        [observation(quality, "success"), observation(quality, "success")],
      ),
    ).toEqual({ status: "denied", code: "TIBER_CI_AUTHORITY_SET_INVALID" });
    expect(
      decideCiEvaluation(
        revision,
        [quality],
        [observation(acceptance, "success")],
      ),
    ).toEqual({ status: "denied", code: "TIBER_CI_AUTHORITY_SET_INVALID" });
  });

  it("reports every absent required authority as pending", () => {
    expect(decideCiEvaluation(revision, [quality, acceptance], [])).toEqual({
      status: "waiting",
      code: "TIBER_CI_AUTHORITIES_INCOMPLETE",
      pendingAuthorities: [quality, acceptance],
    });
  });

  it("creates a repository hold on terminal failure", () => {
    const acceptanceDigest = parsed(parseCiObservationDigest("d".repeat(64)));
    const decision = decideCiEvaluation(
      revision,
      [quality, acceptance],
      [
        {
          ...observation(acceptance, "failure"),
          observationDigest: acceptanceDigest,
        },
        observation(quality, "failure"),
      ],
    );
    expect(decision.status).toBe("failed");
    if (decision.status === "failed") {
      expect(decision.code).toBe("TIBER_CI_TERMINAL_FAILURE");
      expect(decision.hold.failedRevision).toBe(revision);
      expect(decision.hold.failedAuthorities).toEqual([quality, acceptance]);
      expect(decision.hold.failureObservationDigest).toBe(digest);
    }
  });

  it("denies repository delivery while a failure hold exists", () => {
    const hold: RepositoryCiHold = {
      failedRevision: revision,
      failedAuthorities: [quality],
      failureObservationDigest: digest,
    };
    expect(authorizeDeliveryDuringCiHold(none)).toEqual({
      status: "authorized",
    });
    expect(authorizeDeliveryDuringCiHold(some(hold))).toEqual({
      status: "denied",
      code: "TIBER_CI_REPOSITORY_HOLD",
    });
  });

  it("requires causal diagnosis and exact-revision success to recover a hold", () => {
    const hold: RepositoryCiHold = {
      failedRevision: revision,
      failedAuthorities: [quality],
      failureObservationDigest: digest,
    };
    const diagnosis = parsed(
      parseCiDiagnosis(
        "The provider had a transient outage; its rerun is authoritative.",
      ),
    );
    const success = decideCiEvaluation(
      revision,
      [quality],
      [observation(quality, "success")],
    );
    if (success.status !== "succeeded") throw new Error("fixture invalid");

    expect(decideCiHoldRecovery(hold, diagnosis, success.receipt)).toEqual({
      status: "recovered",
    });

    const wrong = decideCiEvaluation(
      otherRevision,
      [quality],
      [observation(quality, "success", otherRevision)],
    );
    if (wrong.status !== "succeeded") throw new Error("fixture invalid");
    expect(decideCiHoldRecovery(hold, diagnosis, wrong.receipt)).toEqual({
      status: "denied",
      code: "TIBER_CI_RECOVERY_REVISION_MISMATCH",
    });
    const multiAuthorityHold = {
      ...hold,
      failedAuthorities: [quality, acceptance],
    };
    expect(
      decideCiHoldRecovery(multiAuthorityHold, diagnosis, {
        revision,
        requiredAuthorities: [quality, acceptance],
        observations: [
          observation(quality, "success"),
          observation(acceptance, "failure"),
        ],
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_CI_RECOVERY_AUTHORITY_MISSING",
    });
    expect(
      decideCiHoldRecovery(multiAuthorityHold, diagnosis, {
        revision,
        requiredAuthorities: [quality],
        observations: [
          observation(quality, "success"),
          observation(acceptance, "success"),
        ],
      }),
    ).toEqual({
      status: "denied",
      code: "TIBER_CI_RECOVERY_AUTHORITY_MISSING",
    });
  });
});
