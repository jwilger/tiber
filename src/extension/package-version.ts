import { readFileSync } from "node:fs";

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

export function readPackageVersion(): string {
  const packageUrl = new URL("../../package.json", import.meta.url);
  const parsed: unknown = JSON.parse(readFileSync(packageUrl, "utf8"));

  if (!isPackageMetadata(parsed)) {
    return "unknown";
  }

  return parsed.version;
}
