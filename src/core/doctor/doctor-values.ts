import { isAbsolute } from "node:path";

import {
  semanticValueFailure,
  type TiberFailure,
  type TiberResult,
} from "../failures/tiber-failure.js";

declare const doctorValuePurpose: unique symbol;
type DoctorValue<Purpose extends string> = string & {
  readonly [doctorValuePurpose]: Purpose;
};

export type DoctorRepositoryPath = DoctorValue<"doctor-repository-path">;
export type DoctorNodeVersion = DoctorValue<"doctor-node-version">;
export type DoctorPackageVersion = DoctorValue<"doctor-package-version">;

type Field =
  "doctorNodeVersion" | "doctorPackageVersion" | "doctorRepositoryPath";
type Failure = TiberFailure<
  "TIBER_DOCTOR_VALUE_INVALID",
  { readonly field: Field },
  "corrected-value"
>;
type Result<Value> = TiberResult<Value, Failure>;

function invalid(field: Field): Result<never> {
  return {
    ok: false,
    failure: semanticValueFailure(
      "TIBER_DOCTOR_VALUE_INVALID",
      field,
      "corrected-value",
    ),
  };
}

export function parseDoctorRepositoryPath(
  value: unknown,
): Result<DoctorRepositoryPath> {
  return typeof value === "string" &&
    isAbsolute(value) &&
    value.length <= 4_096 &&
    !value.includes("\0")
    ? { ok: true, value: value as DoctorRepositoryPath }
    : invalid("doctorRepositoryPath");
}

export function parseDoctorNodeVersion(
  value: unknown,
): Result<DoctorNodeVersion> {
  return typeof value === "string" &&
    /^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(value)
    ? { ok: true, value: value as DoctorNodeVersion }
    : invalid("doctorNodeVersion");
}

export function parseDoctorPackageVersion(
  value: unknown,
): Result<DoctorPackageVersion> {
  return typeof value === "string" &&
    /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(value)
    ? { ok: true, value: value as DoctorPackageVersion }
    : invalid("doctorPackageVersion");
}
