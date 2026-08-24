import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseProcessGroupId,
  parseProcessId,
  parseProcessStartedAt,
  type ProcessGroupId,
  type ProcessId,
  type ProcessStartedAt,
} from "../../src/core/processes/process-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("process semantic values", () => {
  it("keeps process and process-group identities distinct", () => {
    expectTypeOf<ProcessId>().not.toEqualTypeOf<ProcessGroupId>();
    expectTypeOf<ProcessStartedAt>().not.toEqualTypeOf<ProcessId>();
  });

  it("parses process boundary values", () => {
    expect(parseProcessId(42).ok).toBe(true);
    expect(parseProcessGroupId(42).ok).toBe(true);
    expect(parseProcessStartedAt("2026-08-23T16:00:00.000Z").ok).toBe(true);
  });

  it("rejects coercible process values", () => {
    expect(parseProcessId("42").ok).toBe(false);
    expect(parseProcessGroupId(1.5).ok).toBe(false);
    expect(parseProcessStartedAt(1_787_507_200_000).ok).toBe(false);
    expect(parseProcessStartedAt("invalid")).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_PROCESS_VALUE_INVALID",
        "processStartedAt",
      ),
    });
  });

  it.each([
    [parseProcessId, 0, "processId"],
    [parseProcessGroupId, -1, "processGroupId"],
    [parseProcessStartedAt, "2026-08-23", "processStartedAt"],
  ])("rejects malformed process values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_PROCESS_VALUE_INVALID", field),
    });
  });
});
