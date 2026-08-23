import { describe, expect, it } from "vitest";

import {
  BUILT_IN_SETTINGS,
  formatSettingsTable,
  parseSettingsDocument,
  resolveSettings,
  setSetting,
  type SettingsOverrides,
} from "../../src/core/configuration/settings.js";

describe("layered settings", () => {
  it("uses built-ins when both explicit layers inherit", () => {
    expect(resolveSettings({}, {})).toEqual({
      assuranceLevel: { value: "host-trusted", source: "built-in" },
      outputPreviewBytes: { value: 16_384, source: "built-in" },
      worktreeMode: { value: "isolated", source: "built-in" },
    });
  });

  it("prefers project values over user-global values", () => {
    expect(
      resolveSettings(
        {
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 8192,
          worktreeMode: "current",
        },
        {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 4096,
          worktreeMode: "isolated",
        },
      ),
    ).toEqual({
      assuranceLevel: { value: "hermetic", source: "project" },
      outputPreviewBytes: { value: 4096, source: "project" },
      worktreeMode: { value: "isolated", source: "project" },
    });
  });

  it("uses a user-global value when the project inherits", () => {
    expect(
      resolveSettings(
        {
          assuranceLevel: "workspace-and-network-isolated",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        },
        {},
      ),
    ).toEqual({
      assuranceLevel: {
        value: "workspace-and-network-isolated",
        source: "user-global",
      },
      outputPreviewBytes: { value: 32_768, source: "user-global" },
      worktreeMode: { value: "current", source: "user-global" },
    });
  });

  it("renders all columns and effective sources", () => {
    expect(
      formatSettingsTable(
        { assuranceLevel: "workspace-isolated" },
        { worktreeMode: "current" },
      ),
    ).toBe(
      [
        "Setting | Built-in | User global | Project | Effective (source)",
        "assuranceLevel | host-trusted | workspace-isolated | inherit | workspace-isolated (user-global)",
        "outputPreviewBytes | 16384 | inherit | inherit | 16384 (built-in)",
        "worktreeMode | isolated | inherit | current | current (project)",
      ].join("\n"),
    );
  });

  it("exposes the expected built-in defaults", () => {
    expect(BUILT_IN_SETTINGS).toEqual({
      assuranceLevel: "host-trusted",
      outputPreviewBytes: 16_384,
      worktreeMode: "isolated",
    });
  });
});

describe("settings document parsing", () => {
  it("parses a complete versioned document", () => {
    expect(
      parseSettingsDocument({
        schemaVersion: 1,
        values: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 1024,
          worktreeMode: "current",
        },
      }),
    ).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        values: {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 1024,
          worktreeMode: "current",
        },
      },
    });
  });

  it("parses an empty values object without undefined properties", () => {
    const result = parseSettingsDocument({ schemaVersion: 1, values: {} });
    expect(result).toStrictEqual({
      ok: true,
      value: { schemaVersion: 1, values: {} },
    });
    if (result.ok) {
      expect(Object.keys(result.value.values)).toStrictEqual([]);
    }
  });

  it("accepts both preview boundaries", () => {
    expect(
      parseSettingsDocument({
        schemaVersion: 1,
        values: { outputPreviewBytes: 1024 },
      }),
    ).toMatchObject({ ok: true });
    expect(
      parseSettingsDocument({
        schemaVersion: 1,
        values: { outputPreviewBytes: 1_048_576 },
      }),
    ).toMatchObject({ ok: true });
  });

  it.each([
    [null, "settings must use schema version 1 and an object of values"],
    [[], "settings must use schema version 1 and an object of values"],
    [7, "settings must use schema version 1 and an object of values"],
    ["settings", "settings must use schema version 1 and an object of values"],
    [{}, "settings must use schema version 1 and an object of values"],
    [
      { schemaVersion: 2, values: {} },
      "settings must use schema version 1 and an object of values",
    ],
    [
      { schemaVersion: 1, values: [] },
      "settings must use schema version 1 and an object of values",
    ],
    [
      { schemaVersion: 1, values: { unknown: true } },
      "unknown setting: unknown",
    ],
    [
      { schemaVersion: 1, values: { assuranceLevel: "unsafe" } },
      "assuranceLevel is invalid",
    ],
    [
      { schemaVersion: 1, values: { worktreeMode: "shared" } },
      "worktreeMode is invalid",
    ],
    [
      { schemaVersion: 1, values: { outputPreviewBytes: "4096" } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
    [
      { schemaVersion: 1, values: { outputPreviewBytes: 1023 } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
    [
      { schemaVersion: 1, values: { outputPreviewBytes: 1_048_577 } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
    [
      { schemaVersion: 1, values: { outputPreviewBytes: 1024.5 } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
  ] as const)("rejects malformed input %#", (input, message) => {
    expect(parseSettingsDocument(input)).toEqual({
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_DOCUMENT",
        message,
        retryable: false,
      },
    });
  });
});

describe("setting updates", () => {
  it.each([
    [
      "assuranceLevel",
      "workspace-isolated",
      { assuranceLevel: "workspace-isolated" },
    ],
    [
      "assuranceLevel",
      "workspace-and-network-isolated",
      { assuranceLevel: "workspace-and-network-isolated" },
    ],
    ["assuranceLevel", "hermetic", { assuranceLevel: "hermetic" }],
    ["assuranceLevel", "host-trusted", { assuranceLevel: "host-trusted" }],
    ["worktreeMode", "isolated", { worktreeMode: "isolated" }],
    ["worktreeMode", "current", { worktreeMode: "current" }],
    ["outputPreviewBytes", "1024", { outputPreviewBytes: 1024 }],
    ["outputPreviewBytes", "1048576", { outputPreviewBytes: 1_048_576 }],
  ] as const)("sets %s to %s", (key, value, expected) => {
    expect(setSetting({}, key, value)).toEqual({ ok: true, value: expected });
  });

  it.each(["assuranceLevel", "outputPreviewBytes", "worktreeMode"] as const)(
    "removes %s when inheriting without disturbing other values",
    (key) => {
      const result = setSetting(
        {
          assuranceLevel: "hermetic",
          outputPreviewBytes: 4096,
          worktreeMode: "current",
        },
        key,
        "inherit",
      );
      expect(result).toMatchObject({ ok: true });
      if (result.ok) {
        expect(result.value).not.toHaveProperty(key);
        expect(Object.keys(result.value)).toHaveLength(2);
      }
    },
  );

  it.each([
    [
      { assuranceLevel: "hermetic" } satisfies SettingsOverrides,
      "worktreeMode",
      { assuranceLevel: "hermetic" },
    ],
    [
      { outputPreviewBytes: 4096 } satisfies SettingsOverrides,
      "worktreeMode",
      { outputPreviewBytes: 4096 },
    ],
    [
      { assuranceLevel: "hermetic" } satisfies SettingsOverrides,
      "outputPreviewBytes",
      { assuranceLevel: "hermetic" },
    ],
  ] as const)(
    "does not add undefined properties while %s inherits",
    (current, key, expected) => {
      const result = setSetting(current, key, "inherit");
      expect(result).toStrictEqual({ ok: true, value: expected });
      if (result.ok) {
        expect(Object.keys(result.value)).toStrictEqual(Object.keys(expected));
      }
    },
  );

  it.each([
    [
      "unknown",
      "value",
      "TIBER_SETTINGS_INVALID_KEY",
      "unknown setting: unknown",
    ],
    [
      "assuranceLevel",
      "unsafe",
      "TIBER_SETTINGS_INVALID_VALUE",
      "invalid value for assuranceLevel: unsafe",
    ],
    [
      "worktreeMode",
      "shared",
      "TIBER_SETTINGS_INVALID_VALUE",
      "invalid value for worktreeMode: shared",
    ],
    [
      "outputPreviewBytes",
      "1023",
      "TIBER_SETTINGS_INVALID_VALUE",
      "invalid value for outputPreviewBytes: 1023",
    ],
    [
      "outputPreviewBytes",
      "1048577",
      "TIBER_SETTINGS_INVALID_VALUE",
      "invalid value for outputPreviewBytes: 1048577",
    ],
    [
      "outputPreviewBytes",
      "1.5",
      "TIBER_SETTINGS_INVALID_VALUE",
      "invalid value for outputPreviewBytes: 1.5",
    ],
  ] as const)("rejects %s=%s", (key, value, code, message) => {
    expect(setSetting({}, key, value)).toEqual({
      ok: false,
      failure: { code, message, retryable: false },
    });
  });
});
