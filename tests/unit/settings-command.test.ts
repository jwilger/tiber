import { describe, expect, it } from "vitest";

import { none, some } from "../../src/core/types/option.js";

import { parseSettingsCommand } from "../../src/core/configuration/settings-command.js";
import { expectedSettingsFailure } from "../fixtures/failures.js";

const usage =
  "usage: /tiber:settings [show | set <global|project> <setting> <value|inherit> | lock assuranceLevel <value> | unlock assuranceLevel <exact-confirmation> | secret <key> <environment NAME|inherit>]";

describe("settings command", () => {
  it.each(["", "show", "   show   "])("parses show from %j", (input) => {
    expect(parseSettingsCommand(input)).toEqual({
      ok: true,
      value: { kind: "show" },
    });
  });

  it.each([
    [
      "set project assuranceLevel workspace-and-network-isolated",
      "project",
      "assuranceLevel",
      "workspace-and-network-isolated",
    ],
    [
      "  set   global   outputPreviewBytes   4096  ",
      "global",
      "outputPreviewBytes",
      "4096",
    ],
    ["set global worktreeMode current", "global", "worktreeMode", "current"],
  ] as const)("parses %s", (input, scope, key, value) => {
    expect(parseSettingsCommand(input)).toEqual({
      ok: true,
      value: { kind: "set", scope, key, value },
    });
  });

  it("parses authority commands", () => {
    expect(
      parseSettingsCommand(
        "lock assuranceLevel workspace-and-network-isolated",
      ),
    ).toEqual({
      ok: true,
      value: { kind: "lock", value: "workspace-and-network-isolated" },
    });
    expect(
      parseSettingsCommand(
        "unlock assuranceLevel unlock minimumAssuranceLevel=hermetic",
      ),
    ).toEqual({
      ok: true,
      value: {
        kind: "unlock",
        confirmation: "unlock minimumAssuranceLevel=hermetic",
      },
    });
    expect(
      parseSettingsCommand("secret context7 environment CONTEXT7_API_KEY"),
    ).toEqual({
      ok: true,
      value: {
        kind: "secret",
        key: "context7",
        environmentName: some("CONTEXT7_API_KEY"),
      },
    });
    expect(parseSettingsCommand("secret context7 inherit")).toEqual({
      ok: true,
      value: { kind: "secret", key: "context7", environmentName: none },
    });
  });

  const invalid = (message: string) => ({
    ok: false,
    failure: expectedSettingsFailure("TIBER_SETTINGS_INVALID_VALUE", message),
  });

  it.each(["set", "set global", "set global assuranceLevel", "show extra"])(
    "rejects incomplete input %j with usage",
    (input) => {
      expect(parseSettingsCommand(input)).toEqual(invalid(usage));
    },
  );

  it.each([
    "set assuranceLevel inherit",
    "lock other hermetic",
    "lock assuranceLevel hermetic extra",
    "unlock other unlock minimumAssuranceLevel=hermetic",
    "unlock assuranceLevel minimumAssuranceLevel=hermetic",
    "secret context7",
    "secret context7 literal value",
    "secret context7 environment",
    "secret context7 environment invalid-value",
    "secret Invalid environment VALID",
    "secret context7 other VALUE",
    "secret context7 environment VALUE extra",
    "secret context7 inherit extra",
    "other context7 inherit",
    "other context7 environment VALUE",
  ])("rejects malformed authority command %j", (input) => {
    expect(parseSettingsCommand(input)).toEqual(invalid(usage));
  });

  it("rejects an unknown operation", () => {
    expect(parseSettingsCommand("remove global assuranceLevel value")).toEqual(
      invalid(usage),
    );
  });

  it("rejects an invalid scope precisely", () => {
    expect(parseSettingsCommand("set user assuranceLevel hermetic")).toEqual(
      invalid("settings scope must be global or project"),
    );
  });

  it("rejects an invalid key precisely", () => {
    expect(parseSettingsCommand("set global unknown value")).toEqual(
      invalid("unknown setting: unknown"),
    );
  });
});
