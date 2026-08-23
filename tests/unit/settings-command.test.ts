import { describe, expect, it } from "vitest";

import { parseSettingsCommand } from "../../src/core/configuration/settings-command.js";

const usage =
  "usage: /tiber:settings [show | set <global|project> <setting> <value|inherit>]";

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

  it.each(["set", "set global", "set global assuranceLevel", "show extra"])(
    "rejects incomplete input %j with usage",
    (input) => {
      expect(parseSettingsCommand(input)).toEqual({
        ok: false,
        failure: {
          code: "TIBER_SETTINGS_INVALID_VALUE",
          message: usage,
          retryable: false,
        },
      });
    },
  );

  it("rejects an unknown operation", () => {
    expect(parseSettingsCommand("remove global assuranceLevel value")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_VALUE",
        message: usage,
        retryable: false,
      },
    });
  });

  it("rejects an invalid scope precisely", () => {
    expect(parseSettingsCommand("set user assuranceLevel hermetic")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_VALUE",
        message: "settings scope must be global or project",
        retryable: false,
      },
    });
  });

  it("rejects an invalid key precisely", () => {
    expect(parseSettingsCommand("set global unknown value")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_VALUE",
        message: "unknown setting: unknown",
        retryable: false,
      },
    });
  });
});
