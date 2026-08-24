import { describe, expect, it } from "vitest";

import {
  parseAuthorityUnlockConfirmation,
  parseSecretEnvironmentVariableName,
  parseSecretReferenceName,
} from "../../src/core/configuration/configuration-values.js";
import { expectedSettingsFailure } from "../fixtures/failures.js";
import { none, some } from "../../src/core/types/option.js";

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
  failure: expectedSettingsFailure("TIBER_SETTINGS_INVALID_DOCUMENT", message),
});

function authority(value: unknown): AuthorityDocument {
  const parsed = parseAuthorityDocument(value);
  if (!parsed.ok) throw new Error("invalid authority fixture");
  return parsed.value;
}

function unlockConfirmation(value: string) {
  const parsed = parseAuthorityUnlockConfirmation(value);
  if (!parsed.ok) throw new Error("invalid unlock confirmation fixture");
  return parsed.value;
}

describe("global authority ceilings", () => {
  it("leaves an unlocked request unchanged", () => {
    expect(applyAssuranceCeiling("host-trusted", none)).toEqual({
      requested: "host-trusted",
      effective: "host-trusted",
      conflict: none,
    });
  });

  it("prevents a project from weakening network isolation", () => {
    expect(
      applyAssuranceCeiling(
        "workspace-isolated",
        some("workspace-and-network-isolated"),
      ),
    ).toEqual({
      requested: "workspace-isolated",
      effective: "workspace-and-network-isolated",
      conflict: some(
        "project requested workspace-isolated, but the user-global ceiling requires workspace-and-network-isolated or stronger",
      ),
    });
  });

  it.each([
    ["workspace-and-network-isolated", "workspace-and-network-isolated"],
    ["hermetic", "workspace-and-network-isolated"],
  ] as const)("allows %s under %s", (requested, minimum) => {
    expect(applyAssuranceCeiling(requested, some(minimum))).toEqual({
      requested,
      effective: requested,
      conflict: none,
    });
  });

  it("requires an exact state-bound unlock confirmation", () => {
    const locked = lockMinimumAssurance(
      EMPTY_AUTHORITY,
      "workspace-and-network-isolated",
    );
    expect(unlockMinimumAssurance(locked, unlockConfirmation("yes"))).toEqual({
      ok: false,
      failure: expectedSettingsFailure(
        "TIBER_SETTINGS_INVALID_VALUE",
        "unlock requires exact confirmation: unlock minimumAssuranceLevel=workspace-and-network-isolated",
      ),
    });
    expect(
      unlockMinimumAssurance(
        locked,
        unlockConfirmation(
          "unlock minimumAssuranceLevel=workspace-and-network-isolated",
        ),
      ),
    ).toEqual({ ok: true, value: EMPTY_AUTHORITY });
  });

  it("treats unlocking an unlocked document as an idempotent no-op", () => {
    expect(
      unlockMinimumAssurance(EMPTY_AUTHORITY, unlockConfirmation("anything")),
    ).toEqual({
      ok: true,
      value: EMPTY_AUTHORITY,
    });
  });
});

function secretKey(value: string) {
  const parsed = parseSecretReferenceName(value);
  if (!parsed.ok) throw new Error("invalid secret key fixture");
  return parsed.value;
}

function environmentName(value: string) {
  const parsed = parseSecretEnvironmentVariableName(value);
  if (!parsed.ok) throw new Error("invalid environment name fixture");
  return parsed.value;
}

describe("secret references", () => {
  it("stores only an external environment reference", () => {
    expect(
      setSecretReference(
        EMPTY_AUTHORITY,
        secretKey("context7"),
        some(environmentName("CONTEXT7_API_KEY")),
      ),
    ).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        ceilings: { minimumAssuranceLevel: none },
        secretReferences: {
          context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        },
      },
    });
  });

  it.each([
    ["Context7", "CONTEXT7_API_KEY"],
    ["context7", "actual-secret-value"],
    ["context7", "_LEADING"],
    ["context7", "VALID-bad"],
    ["valid_BAD", "VALID"],
    ["context7", "A".repeat(129)],
  ])("rejects invalid reference %s=%s at its boundary", (key, name) => {
    const parsedKey = parseSecretReferenceName(key);
    const parsedName = parseSecretEnvironmentVariableName(name);
    expect(parsedKey.ok && parsedName.ok).toBe(false);
  });

  it("removes exactly the selected reference", () => {
    const current = authority({
      schemaVersion: 1,
      ceilings: {},
      secretReferences: {
        context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
        hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
      },
    });
    expect(setSecretReference(current, secretKey("context7"), none)).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        ceilings: { minimumAssuranceLevel: none },
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
    expect(parseAuthorityDocument(valid)).toEqual({
      ok: true,
      value: authority(valid),
    });
  });

  it("parses an empty unlocked document without undefined properties", () => {
    expect(
      parseAuthorityDocument({
        schemaVersion: 1,
        ceilings: {},
        secretReferences: {},
      }),
    ).toEqual({ ok: true, value: EMPTY_AUTHORITY });
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
    const configuredAuthority = authority({
      schemaVersion: 1,
      ceilings: {
        minimumAssuranceLevel: "workspace-and-network-isolated",
      },
      secretReferences: {
        hindsight: { provider: "environment", name: "HINDSIGHT_API_KEY" },
        context7: { provider: "environment", name: "CONTEXT7_API_KEY" },
      },
    });
    expect(formatAuthority(configuredAuthority, "host-trusted")).toBe(
      "Minimum assurance lock: workspace-and-network-isolated\n" +
        "Assurance after ceiling: workspace-and-network-isolated\n" +
        "Conflict: project requested host-trusted, but the user-global ceiling requires workspace-and-network-isolated or stronger\n" +
        "Secret references: context7=environment:CONTEXT7_API_KEY, hindsight=environment:HINDSIGHT_API_KEY",
    );
  });
});
