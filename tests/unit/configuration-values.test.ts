import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseAuthorityUnlockConfirmation,
  parseOutputPreviewBytes,
  parseProjectId,
  parseSecretEnvironmentVariableName,
  parseSecretReferenceName,
  parseSettingsCommandValue,
  type AuthorityUnlockConfirmation,
  type OutputPreviewBytes,
  type ProjectId,
  type SecretEnvironmentVariableName,
  type SecretReferenceName,
  type SettingsCommandValue,
} from "../../src/core/configuration/configuration-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("configuration semantic values", () => {
  it("keeps project, secret, and limit purposes distinct", () => {
    expectTypeOf<ProjectId>().not.toEqualTypeOf<SecretReferenceName>();
    expectTypeOf<SecretEnvironmentVariableName>().not.toEqualTypeOf<SecretReferenceName>();
    expectTypeOf<OutputPreviewBytes>().not.toEqualTypeOf<ProjectId>();
    expectTypeOf<AuthorityUnlockConfirmation>().not.toEqualTypeOf<SettingsCommandValue>();
  });

  it("parses configuration values at persistence boundaries", () => {
    expect(parseProjectId("2424c876-6180-4c64-976e-9ea4bd540744").ok).toBe(
      true,
    );
    expect(parseOutputPreviewBytes(16_384).ok).toBe(true);
    expect(parseSettingsCommandValue("hermetic").ok).toBe(true);
    expect(parseAuthorityUnlockConfirmation("unlock exact state").ok).toBe(
      true,
    );
    expect(parseSecretReferenceName("context7").ok).toBe(true);
    expect(parseSecretEnvironmentVariableName("CONTEXT7_API_KEY").ok).toBe(
      true,
    );
  });

  it("rejects coercible and out-of-bound configuration values", () => {
    expect(
      parseProjectId({ toString: () => "2424c876-6180-4c64-976e-9ea4bd540744" })
        .ok,
    ).toBe(false);
    expect(parseProjectId("x2424c876-6180-4c64-976e-9ea4bd540744").ok).toBe(
      false,
    );
    expect(parseProjectId("2424c876-6180-4c64-976e-9ea4bd540744x").ok).toBe(
      false,
    );
    expect(parseOutputPreviewBytes("16384").ok).toBe(false);
    expect(parseOutputPreviewBytes(1_048_577).ok).toBe(false);
    expect(parseSettingsCommandValue({ length: 1 }).ok).toBe(false);
    expect(parseSettingsCommandValue("x".repeat(256)).ok).toBe(true);
    expect(parseSettingsCommandValue("x".repeat(257)).ok).toBe(false);
    expect(parseAuthorityUnlockConfirmation({ length: 1 }).ok).toBe(false);
    expect(parseAuthorityUnlockConfirmation("x".repeat(512)).ok).toBe(true);
    expect(parseAuthorityUnlockConfirmation("x".repeat(513)).ok).toBe(false);
    expect(parseSecretReferenceName({ toString: () => "valid" }).ok).toBe(
      false,
    );
    expect(
      parseSecretEnvironmentVariableName({ toString: () => "VALID" }).ok,
    ).toBe(false);
    expect(parseSecretReferenceName("xvalid").ok).toBe(true);
    expect(parseSecretReferenceName("valid!").ok).toBe(false);
    expect(parseSecretEnvironmentVariableName("VALID!").ok).toBe(false);
  });

  it.each([
    [parseProjectId, "bad", "projectId"],
    [parseSettingsCommandValue, "", "settingsCommandValue"],
    [parseAuthorityUnlockConfirmation, "", "authorityUnlockConfirmation"],
    [parseOutputPreviewBytes, 1023, "outputPreviewBytes"],
    [parseSecretReferenceName, "Context7", "secretReferenceName"],
    [
      parseSecretEnvironmentVariableName,
      "_SECRET",
      "secretEnvironmentVariableName",
    ],
  ])("rejects malformed configuration values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure(
        "TIBER_CONFIGURATION_VALUE_INVALID",
        field,
      ),
    });
  });
});
