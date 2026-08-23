import { describe, expect, it } from "vitest";

import {
  applyAssuranceCeiling,
  EMPTY_AUTHORITY,
  formatAuthority,
  lockMinimumAssurance,
  parseAuthorityDocument,
  setSecretReference,
  unlockMinimumAssurance,
  type AuthorityDocument,
} from "../../src/core/configuration/authority.js";

const invalidDocument = (message: string) => ({
  ok: false,
  failure: {
    code: "TIBER_SETTINGS_INVALID_DOCUMENT",
    message,
    retryable: false,
  },
});

describe("global authority ceilings", () => {
  it("leaves an unlocked request unchanged", () => {
    expect(applyAssuranceCeiling("host-trusted", undefined)).toEqual({
      requested: "host-trusted",
      effective: "host-trusted",
    });
  });

  it("prevents a project from weakening network isolation", () => {
    expect(
      applyAssuranceCeiling(
        "workspace-isolated",
        "workspace-and-network-isolated",
      ),
    ).toEqual({
      requested: "workspace-isolated",
      effective: "workspace-and-network-isolated",
      conflict:
        "project requested workspace-isolated, but the user-global ceiling requires workspace-and-network-isolated or stronger",
    });
  });

  it.each([
    ["workspace-and-network-isolated", "workspace-and-network-isolated"],
    ["hermetic", "workspace-and-network-isolated"],
  ] as const)("allows %s under %s", (requested, minimum) => {
    expect(applyAssuranceCeiling(requested, minimum)).toEqual({
      requested,
      effective: requested,
    });
  });

  it("requires an exact state-bound unlock confirmation", () => {
    const locked = lockMinimumAssurance(
      EMPTY_AUTHORITY,
      "workspace-and-network-isolated",
    );
    expect(unlockMinimumAssurance(locked, "yes")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_SETTINGS_INVALID_VALUE",
        message:
          "unlock requires exact confirmation: unlock minimumAssuranceLevel=workspace-and-network-isolated",
        retryable: false,
      },
    });
    expect(
      unlockMinimumAssurance(
        locked,
        "unlock minimumAssuranceLevel=workspace-and-network-isolated",
      ),
    ).toEqual({ ok: true, value: EMPTY_AUTHORITY });
  });

  it("treats unlocking an unlocked document as an idempotent no-op", () => {
    expect(unlockMinimumAssurance(EMPTY_AUTHORITY, "anything")).toEqual({
      ok: true,
      value: EMPTY_AUTHORITY,
    });
  });
});

describe("secret references", () => {
  it("stores only an external environment reference", () => {
    expect(
      setSecretReference(EMPTY_AUTHORITY, "context7", "CONTEXT7_API_KEY"),
    ).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        ceilings: {},
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      },
    });
  });

  it.each([
    [
      "Context7",
      "CONTEXT7_API_KEY",
      "secret reference key is invalid: Context7",
    ],
    [
      "context7",
      "actual-secret-value",
      "environment variable name is invalid: actual-secret-value",
    ],
    ["context7", "_LEADING", "environment variable name is invalid: _LEADING"],
    [
      "context7",
      "VALID-bad",
      "environment variable name is invalid: VALID-bad",
    ],
    ["valid_BAD", "VALID", "secret reference key is invalid: valid_BAD"],
    [
      "context7",
      "A".repeat(129),
      `environment variable name is invalid: ${"A".repeat(129)}`,
    ],
  ])("rejects invalid reference %s=%s", (key, name, message) => {
    expect(setSecretReference(EMPTY_AUTHORITY, key, name)).toEqual(
      invalidDocument(message),
    );
  });

  it("removes exactly the selected reference", () => {
    const current: AuthorityDocument = {
      schemaVersion: 1,
      ceilings: {},
      secretReferences: {
        context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
      },
    };
    expect(setSecretReference(current, "context7", undefined)).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        ceilings: {},
        secretReferences: {
          hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        },
      },
    });
  });
});

describe("authority document boundary", () => {
  const valid = {
    schemaVersion: 1,
    ceilings: { minimumAssuranceLevel: "workspace-and-network-isolated" },
    secretReferences: {
      context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
    },
  };

  it("parses a complete document into semantic values", () => {
    expect(parseAuthorityDocument(valid)).toEqual({ ok: true, value: valid });
  });

  it("parses an empty unlocked document without undefined properties", () => {
    expect(parseAuthorityDocument(EMPTY_AUTHORITY)).toEqual({
      ok: true,
      value: EMPTY_AUTHORITY,
    });
  });

  it.each([
    null,
    [],
    "document",
    {},
    { ...valid, schemaVersion: 2 },
    { ...valid, ceilings: null },
    { ...valid, secretReferences: null },
  ])("rejects malformed document shape %j", (input) => {
    expect(parseAuthorityDocument(input)).toEqual(
      invalidDocument("authority settings must use schema version 1"),
    );
  });

  it.each([
    { ...valid, ceilings: { unexpected: "value" } },
    { ...valid, ceilings: { minimumAssuranceLevel: "none" } },
  ])("rejects malformed ceiling %j", (input) => {
    expect(parseAuthorityDocument(input)).toEqual(
      invalidDocument("minimum assurance ceiling is invalid"),
    );
  });

  it("rejects invalid secret keys and representations precisely", () => {
    expect(
      parseAuthorityDocument({
        ...valid,
        secretReferences: {
          Context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      }),
    ).toEqual(invalidDocument("secret reference key is invalid: Context7"));
    expect(
      parseAuthorityDocument({
        ...valid,
        secretReferences: {
          valid_BAD: { provider: "environment", name: "VALID" },
        },
      }),
    ).toEqual(invalidDocument("secret reference key is invalid: valid_BAD"));
    for (const reference of [
      null,
      [],
      { provider: "literal", name: "CONTEXT7_API_KEY" },
      { provider: "environment", name: 1 },
      { provider: "environment", name: "bad-value" },
      { provider: "environment", name: "_LEADING" },
      { provider: "environment", name: "VALID-bad" },
    ]) {
      expect(
        parseAuthorityDocument({
          ...valid,
          secretReferences: { context7: reference },
        }),
      ).toEqual(invalidDocument("secret reference is invalid: context7"));
    }
  });

  it("renders unlocked authority exactly", () => {
    expect(formatAuthority(EMPTY_AUTHORITY, "host-trusted")).toBe(
      "Minimum assurance lock: unlocked\nAssurance after ceiling: host-trusted\nSecret references: none",
    );
  });

  it("renders sorted references and conflict preview without secret values", () => {
    const authority: AuthorityDocument = {
      schemaVersion: 1,
      ceilings: {
        minimumAssuranceLevel: "workspace-and-network-isolated",
      },
      secretReferences: {
        hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
      },
    };
    expect(formatAuthority(authority, "host-trusted")).toBe(
      "Minimum assurance lock: workspace-and-network-isolated\n" +
        "Assurance after ceiling: workspace-and-network-isolated\n" +
        "Conflict: project requested host-trusted, but the user-global ceiling requires workspace-and-network-isolated or stronger\n" +
        "Secret references: context7=environment:CONTEXT7_API_KEY, hindsight=environment:HINDSIGHT_API_KEY",
    );
  });
});
