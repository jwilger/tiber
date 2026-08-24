import {
  parseCommandCatalogDigest,
  parseCommandName,
} from "../../src/core/commands/command-values.js";

function required<Value>(
  result: { readonly ok: true; readonly value: Value } | { readonly ok: false },
): Value {
  if (!result.ok) throw new Error("invalid command semantic fixture");
  return result.value;
}

export const commandName = (value: string) => required(parseCommandName(value));
export const commandCatalogDigest = (value: string) =>
  required(parseCommandCatalogDigest(value));
