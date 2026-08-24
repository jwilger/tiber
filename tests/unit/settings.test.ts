import { describe, expect, it } from "vitest";

import {
  BUILT_IN_SETTINGS,
  formatSettingsTable,
  parseSettingsDocument,
  resolveSettings,
  setSetting,
  settingsFailure,
  type SettingsOverrides,
} from "../../src/core/configuration/settings.js";
import { none, some } from "../../src/core/types/option.js";
import { expectedSettingsFailure } from "../fixtures/failures.js";

function overrides(
  values: Readonly<Record<string, unknown>> = {},
): SettingsOverrides {
  const parsed = parseSettingsDocument({ schemaVersion: 1, values });
  if (!parsed.ok) throw new Error("invalid settings fixture");
  return parsed.value.values;
}

const inherited = (): SettingsOverrides => overrides();

describe("layered settings", () => {
  it("uses built-ins when both explicit layers inherit", () => {
    expect(resolveSettings(inherited(), inherited())).toEqual({
      assuranceLevel: { value: "host-trusted", source: "built-in" },
      outputPreviewBytes: { value: 16_384, source: "built-in" },
      worktreeMode: { value: "isolated", source: "built-in" },
    });
  });

  it("prefers project values over user-global values", () => {
    expect(
      resolveSettings(
        overrides({
          assuranceLevel: "workspace-isolated",
          outputPreviewBytes: 8192,
          worktreeMode: "current",
        }),
        overrides({
          assuranceLevel: "hermetic",
          outputPreviewBytes: 4096,
          worktreeMode: "isolated",
        }),
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
        overrides({
          assuranceLevel: "workspace-and-network-isolated",
          outputPreviewBytes: 32_768,
          worktreeMode: "current",
        }),
        inherited(),
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
        overrides({ assuranceLevel: "workspace-isolated" }),
        overrides({ worktreeMode: "current" }),
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
  it("parses explicit and inherited fields into Options", () => {
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
          assuranceLevel: some("hermetic"),
          outputPreviewBytes: some(1024),
          worktreeMode: some("current"),
        },
      },
    });
    expect(parseSettingsDocument({ schemaVersion: 1, values: {} })).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        values: {
          assuranceLevel: none,
          outputPreviewBytes: none,
          worktreeMode: none,
        },
      },
    });
  });

  it("accepts both preview boundaries", () => {
    expect(
      parseSettingsDocument({
        schemaVersion: 1,
        values: { outputPreviewBytes: 1024 },
      }).ok,
    ).toBe(true);
    expect(
      parseSettingsDocument({
        schemaVersion: 1,
        values: { outputPreviewBytes: 1_048_576 },
      }).ok,
    ).toBe(true);
  });

  it.each([
    [null, "settings must use schema version 1 and an object of values"],
    [{}, "settings must use schema version 1 and an object of values"],
    [
      { schemaVersion: 2, values: {} },
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
      { schemaVersion: 1, values: { outputPreviewBytes: 1023 } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
    [
      { schemaVersion: 1, values: { outputPreviewBytes: 1_048_577 } },
      "outputPreviewBytes must be an integer from 1024 to 1048576",
    ],
  ] as const)("rejects malformed input", (input, message) => {
    expect(parseSettingsDocument(input)).toEqual({
      ok: false,
      failure: expectedSettingsFailure(
        "TIBER_SETTINGS_INVALID_DOCUMENT",
        message,
      ),
    });
  });
});

describe("settings failures", () => {
  it.each([
    ["TIBER_SETTINGS_IO", "transient", ["retry-operation"]],
    [
      "TIBER_SETTINGS_REPOSITORY_REQUIRED",
      "retry-after-state-change",
      ["repository-required"],
    ],
    [
      "TIBER_SETTINGS_INVALID_VALUE",
      "retry-after-input",
      ["corrected-settings"],
    ],
  ] as const)("classifies %s recovery", (code, retryability, evidence) => {
    expect(settingsFailure(code, "failure")).toEqual({
      ...expectedSettingsFailure(code, "failure"),
      retryability,
      requiredRecoveryEvidence: evidence,
    });
  });
});

describe("setting updates", () => {
  it.each([
    ["assuranceLevel", "workspace-isolated"],
    ["worktreeMode", "current"],
    ["outputPreviewBytes", "1024"],
  ] as const)("sets %s to %s", (key, value) => {
    const result = setSetting(inherited(), key, value);
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.value[key].kind).toBe("some");
  });

  it("inherits one setting without clearing explicit siblings", () => {
    const explicit = overrides({
      assuranceLevel: "hermetic",
      outputPreviewBytes: 4096,
      worktreeMode: "current",
    });
    expect(setSetting(explicit, "assuranceLevel", "inherit")).toEqual({
      ok: true,
      value: {
        assuranceLevel: none,
        outputPreviewBytes: some(4096),
        worktreeMode: some("current"),
      },
    });
    expect(setSetting(explicit, "outputPreviewBytes", "inherit")).toEqual({
      ok: true,
      value: {
        assuranceLevel: some("hermetic"),
        outputPreviewBytes: none,
        worktreeMode: some("current"),
      },
    });
    expect(setSetting(explicit, "worktreeMode", "inherit")).toEqual({
      ok: true,
      value: {
        assuranceLevel: some("hermetic"),
        outputPreviewBytes: some(4096),
        worktreeMode: none,
      },
    });
  });

  it.each(["assuranceLevel", "outputPreviewBytes", "worktreeMode"] as const)(
    "represents inherited %s explicitly as none",
    (key) => {
      const result = setSetting(
        overrides({
          assuranceLevel: "hermetic",
          outputPreviewBytes: 4096,
          worktreeMode: "current",
        }),
        key,
        "inherit",
      );
      expect(result.ok).toBe(true);
      if (result.ok) expect(result.value[key]).toEqual(none);
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
  ] as const)("rejects %s=%s", (key, value, code, message) => {
    expect(setSetting(inherited(), key, value)).toEqual({
      ok: false,
      failure: expectedSettingsFailure(code, message),
    });
  });
});
