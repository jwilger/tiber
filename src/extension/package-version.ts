import { readFileSync } from "node:fs";

import {
  parseDoctorPackageVersion,
  type DoctorPackageVersion,
} from "../core/doctor/doctor-values.js";

interface PackageMetadata {
  readonly version: string;
}

function isPackageMetadata(value: unknown): value is PackageMetadata {
  return (
    typeof value === "object" &&
    value !== null &&
    "version" in value &&
    typeof value.version === "string"
  );
}

export function readPackageVersion(): DoctorPackageVersion {
  const packageUrl = new URL("../../package.json", import.meta.url);
  const parsed: unknown = JSON.parse(readFileSync(packageUrl, "utf8"));

  if (!isPackageMetadata(parsed))
    throw new Error("package metadata omits a valid version");
  const version = parseDoctorPackageVersion(parsed.version);
  if (!version.ok) throw new Error("package version violates its invariant");
  return version.value;
}
