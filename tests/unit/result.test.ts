import { describe, expect, it } from "vitest";

import {
  fail,
  flatMapResult,
  mapFailure,
  mapResult,
  operationalFailure,
  semanticValueFailure,
  succeed,
} from "../../src/core/failures/tiber-failure.js";

describe("Result", () => {
  it("maps only the success rail", () => {
    expect(mapResult(succeed(2), (value) => value * 3)).toEqual(succeed(6));
    const denied = fail("denied");
    expect(mapResult(denied, (value: number) => value * 3)).toBe(denied);
  });

  it("flat maps without hiding either failure type", () => {
    expect(
      flatMapResult(succeed(2), (value) =>
        value === 2 ? succeed("ready") : fail("unexpected"),
      ),
    ).toEqual(succeed("ready"));
    expect(flatMapResult(fail("first"), () => fail("second"))).toEqual(
      fail("first"),
    );
  });

  it("constructs complete semantic value failures", () => {
    expect(
      semanticValueFailure("TIBER_VALUE_INVALID", "taskId", "corrected-value"),
    ).toEqual({
      code: "TIBER_VALUE_INVALID",
      message: "Invalid taskId",
      safeContext: { field: "taskId" },
      causes: [],
      retryability: "retry-after-input",
      requiredRecoveryEvidence: ["corrected-value"],
      redaction: "public",
    });
  });

  it.each([
    ["transient", "retry-operation"],
    ["retry-after-input", "corrected-input"],
    ["retry-after-state-change", "state-change"],
    ["not-retryable", undefined],
  ] as const)(
    "constructs complete %s operational failures",
    (retryability, evidence) => {
      expect(
        operationalFailure(
          "TIBER_OPERATION_FAILED",
          "test",
          "failed",
          retryability,
        ),
      ).toEqual({
        code: "TIBER_OPERATION_FAILED",
        message: "failed",
        safeContext: { domain: "test" },
        causes: [],
        retryability,
        requiredRecoveryEvidence: evidence === undefined ? [] : [evidence],
        redaction: "public",
      });
    },
  );

  it("maps only the failure rail", () => {
    expect(mapFailure(fail("bad"), (failure) => failure.length)).toEqual(
      fail(3),
    );
    const accepted = succeed("good");
    expect(mapFailure(accepted, () => 3)).toBe(accepted);
  });
});
