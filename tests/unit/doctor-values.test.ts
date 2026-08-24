import { describe, expect, expectTypeOf, it } from "vitest";

import {
  parseDoctorNodeVersion,
  parseDoctorPackageVersion,
  parseDoctorRepositoryPath,
  type DoctorNodeVersion,
  type DoctorPackageVersion,
  type DoctorRepositoryPath,
} from "../../src/core/doctor/doctor-values.js";
import { expectedSemanticFailure } from "../fixtures/failures.js";

describe("doctor semantic values", () => {
  it("keeps runtime, package, and repository purposes distinct", () => {
    expectTypeOf<DoctorNodeVersion>().not.toEqualTypeOf<DoctorPackageVersion>();
    expectTypeOf<DoctorRepositoryPath>().not.toEqualTypeOf<DoctorPackageVersion>();
  });

  it("parses observed doctor values", () => {
    expect(parseDoctorNodeVersion("v22.23.1").ok).toBe(true);
    expect(parseDoctorNodeVersion("v10.20.30-beta.12").ok).toBe(true);
    expect(parseDoctorPackageVersion("0.1.0-beta.1").ok).toBe(true);
    expect(parseDoctorPackageVersion("10.20.30").ok).toBe(true);
    expect(parseDoctorRepositoryPath("/workspace/tiber").ok).toBe(true);
  });

  it.each([
    [parseDoctorNodeVersion, "22", "doctorNodeVersion"],
    [parseDoctorPackageVersion, "unknown", "doctorPackageVersion"],
    [parseDoctorRepositoryPath, "relative", "doctorRepositoryPath"],
  ])("rejects malformed doctor values", (parse, value, field) => {
    expect(parse(value)).toEqual({
      ok: false,
      failure: expectedSemanticFailure("TIBER_DOCTOR_VALUE_INVALID", field),
    });
  });

  it("rejects coercible doctor values", () => {
    expect(parseDoctorNodeVersion({ toString: () => "v22.23.1" }).ok).toBe(
      false,
    );
    expect(parseDoctorPackageVersion({ toString: () => "0.1.0" }).ok).toBe(
      false,
    );
    expect(parseDoctorNodeVersion("xv22.23.1").ok).toBe(false);
    expect(parseDoctorNodeVersion("v22.23.1x!").ok).toBe(false);
    expect(parseDoctorPackageVersion("x0.1.0").ok).toBe(false);
    expect(parseDoctorPackageVersion("0.1.0x!").ok).toBe(false);
    expect(parseDoctorNodeVersion("v10.20.30_beta").ok).toBe(false);
    expect(parseDoctorNodeVersion("v10.20.30-!").ok).toBe(false);
    expect(parseDoctorPackageVersion("10.20.30_1").ok).toBe(false);
    expect(parseDoctorRepositoryPath({ toString: () => "/repo" }).ok).toBe(
      false,
    );
    expect(parseDoctorRepositoryPath("/" + "x".repeat(4_095)).ok).toBe(true);
    expect(parseDoctorRepositoryPath("/" + "x".repeat(4_096)).ok).toBe(false);
    expect(parseDoctorRepositoryPath("/bad\0path").ok).toBe(false);
  });
});
