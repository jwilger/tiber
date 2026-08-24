import type {
  DoctorNodeVersion,
  DoctorPackageVersion,
  DoctorRepositoryPath,
} from "./doctor-values.js";

export const BOOTSTRAP_MODE = "read-only-bootstrap" as const;

export interface DoctorInput {
  readonly cwd: DoctorRepositoryPath;
  readonly nodeVersion: DoctorNodeVersion;
  readonly packageVersion: DoctorPackageVersion;
}

export interface DoctorReport {
  readonly product: "@jwilger/tiber";
  readonly version: DoctorPackageVersion;
  readonly nodeVersion: DoctorNodeVersion;
  readonly repositoryPath: DoctorRepositoryPath;
  readonly mode: typeof BOOTSTRAP_MODE;
  readonly mutationPolicy: "known-mutation-tools-blocked";
}

export function createDoctorReport(input: DoctorInput): DoctorReport {
  return {
    product: "@jwilger/tiber",
    version: input.packageVersion,
    nodeVersion: input.nodeVersion,
    repositoryPath: input.cwd,
    mode: BOOTSTRAP_MODE,
    mutationPolicy: "known-mutation-tools-blocked",
  };
}

export function formatDoctorReport(report: DoctorReport): string {
  return [
    `${report.product} ${report.version}`,
    `Mode: ${report.mode}`,
    `Node: ${report.nodeVersion}`,
    `Repository: ${report.repositoryPath}`,
    `Mutation policy: ${report.mutationPolicy}`,
  ].join("\n");
}
