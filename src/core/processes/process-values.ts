import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const processValuePurpose: unique symbol;
type ProcessValue<Value, Purpose extends string> = Value & {
  readonly [processValuePurpose]: Purpose;
};

export type ProcessId = ProcessValue<number, "process-id">;
export type ProcessGroupId = ProcessValue<number, "process-group-id">;
export type ProcessStartedAt = ProcessValue<string, "process-started-at">;

type Field = "processId" | "processGroupId" | "processStartedAt";
type Failure = TiberFailure<
  "TIBER_PROCESS_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_PROCESS_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

function positive<Purpose extends string>(
  value: unknown,
  field: Field,
): Result<ProcessValue<number, Purpose>> {
  // Stryker disable next-line ConditionalExpression: Number.isSafeInteger independently rejects every non-number; typeof establishes narrowing.
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? { ok: true, value: value as ProcessValue<number, Purpose> }
    : invalid(field);
}

export const parseProcessId = (value: unknown): Result<ProcessId> =>
  positive(value, "processId");
export const parseProcessGroupId = (value: unknown): Result<ProcessGroupId> =>
  positive(value, "processGroupId");

export function parseProcessStartedAt(
  value: unknown,
): Result<ProcessStartedAt> {
  // Stryker disable next-line ConditionalExpression: canonical ISO equality below independently rejects non-strings accepted by Date.parse; typeof establishes narrowing.
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value)))
    return invalid("processStartedAt");
  return new Date(value).toISOString() === value
    ? { ok: true, value: value as ProcessStartedAt }
    : invalid("processStartedAt");
}
