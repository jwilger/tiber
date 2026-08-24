import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseCanonicalCommandCatalogJson,
  parseCommandArgument,
  parseCommandCatalogDigest,
  parseCommandEnvironmentName,
  parseCommandEnvironmentValue,
  parseCommandExecutable,
  parseCommandMaximumOutputBytes,
  parseCommandName,
  parseCommandTimeoutMilliseconds,
  type CommandArgument,
  type CommandCatalogDigest,
  type CommandEnvironmentValue,
  type CommandMaximumOutputBytes,
  type CommandName,
  type CommandTimeoutMilliseconds,
} from "../../src/core/commands/command-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("command semantic values", () => {
  it("keeps command identities, digests, and distinct limits separate", () => {
    expectTypeOf<CommandName>().not.toEqualTypeOf<CommandCatalogDigest>();
    expectTypeOf<CommandTimeoutMilliseconds>().not.toEqualTypeOf<CommandMaximumOutputBytes>();
    expectTypeOf<CommandArgument>().not.toEqualTypeOf<CommandEnvironmentValue>();
  });

  it("parses values at the command catalog boundary", () => {
    expect(parseCommandName("unit-tests").ok).toBe(true);
    expect(parseCommandArgument("--run").ok).toBe(true);
    expect(parseCommandEnvironmentName("NODE_ENV").ok).toBe(true);
    expect(parseCommandEnvironmentValue("test").ok).toBe(true);
    expect(parseCanonicalCommandCatalogJson("{}").ok).toBe(true);
    expect(parseCommandExecutable("/usr/bin/npm").ok).toBe(true);
    expect(parseCommandTimeoutMilliseconds(60_000).ok).toBe(true);
    expect(parseCommandMaximumOutputBytes(65_536).ok).toBe(true);
    expect(parseCommandCatalogDigest(`sha256:${"a".repeat(64)}`).ok).toBe(true);
  });

  it("rejects coercible and out-of-bound command values", () => {
    expect(parseCommandName({ toString: () => "unit-tests" }).ok).toBe(false);
    expect(parseCommandArgument({ length: 1, includes: () => false }).ok).toBe(
      false,
    );
    expect(parseCommandArgument("x".repeat(4_097)).ok).toBe(false);
    expect(parseCommandEnvironmentName({ toString: () => "NODE_ENV" }).ok).toBe(
      false,
    );
    expect(
      parseCommandEnvironmentValue({ length: 1, includes: () => false }).ok,
    ).toBe(false);
    expect(parseCommandEnvironmentValue("x".repeat(4_097)).ok).toBe(false);
    expect(parseCanonicalCommandCatalogJson({ length: 1 }).ok).toBe(false);
    expect(parseCommandExecutable("").ok).toBe(false);
    expect(parseCommandExecutable("/" + "x".repeat(499)).ok).toBe(true);
    expect(parseCommandExecutable("/" + "x".repeat(500)).ok).toBe(false);
    expect(
      parseCommandExecutable({ length: 4, includes: () => false }).ok,
    ).toBe(false);
    expect(parseCommandTimeoutMilliseconds("1").ok).toBe(false);
    expect(parseCommandTimeoutMilliseconds(3_600_001).ok).toBe(false);
    expect(parseCommandMaximumOutputBytes("1").ok).toBe(false);
    expect(
      parseCommandCatalogDigest({
        toString: () => `sha256:${"a".repeat(64)}`,
      }).ok,
    ).toBe(false);
    expect(parseCommandCatalogDigest(`xsha256:${"a".repeat(64)}`).ok).toBe(
      false,
    );
    expect(parseCommandCatalogDigest(`sha256:${"a".repeat(64)}x`).ok).toBe(
      false,
    );
  });

  it.each([
    [parseCommandName, "Unit Tests", "commandName"],
    [parseCommandArgument, "bad\0argument", "commandArgument"],
    [parseCommandEnvironmentName, "lowercase", "commandEnvironmentName"],
    [parseCommandEnvironmentValue, "bad\0value", "commandEnvironmentValue"],
    [parseCanonicalCommandCatalogJson, "", "canonicalCommandCatalogJson"],
    [parseCommandExecutable, "npm", "commandExecutable"],
    [parseCommandTimeoutMilliseconds, 0, "commandTimeoutMilliseconds"],
    [parseCommandMaximumOutputBytes, 1_048_577, "commandMaximumOutputBytes"],
    [
      parseCommandCatalogDigest,
      `sha256:${"A".repeat(64)}`,
      "commandCatalogDigest",
    ],
  ])("rejects malformed command values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_COMMAND_VALUE_INVALID", field),
    });
  });
});
