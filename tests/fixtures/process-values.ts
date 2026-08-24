import {
  parseProcessGroupId,
  parseProcessId,
  parseProcessStartedAt,
} from "../../src/core/processes/process-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid process semantic fixture");
  return result.value;
}

export const processId = (value: number) => required(parseProcessId(value));
export const processGroupId = (value: number) =>
  required(parseProcessGroupId(value));
export const processStartedAt = (value: string) =>
  required(parseProcessStartedAt(value));
