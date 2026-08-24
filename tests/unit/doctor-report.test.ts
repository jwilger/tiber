import { describe, expect, it } from "vitest";

import {
  parseDoctorNodeVersion,
  parseDoctorPackageVersion,
  parseDoctorRepositoryPath,
} from "../../src/core/doctor/doctor-values.js";
import {
  BOOTSTRAP_MODE,
  createDoctorReport,
  formatDoctorReport,
} from "../../src/core/doctor/report.js";

describe("doctor report", () => {
  it("describes the installed package and bootstrap safety mode", () => {
    const cwd = parseDoctorRepositoryPath("/workspace/tiber");
    const nodeVersion = parseDoctorNodeVersion("v22.23.1");
    const packageVersion = parseDoctorPackageVersion("0.0.0");
    if (!cwd.ok || !nodeVersion.ok || !packageVersion.ok)
      throw new Error("invalid doctor fixture");
    const report = createDoctorReport({
      cwd: cwd.value,
      nodeVersion: nodeVersion.value,
      packageVersion: packageVersion.value,
    });

    expect(report.mode).toBe(BOOTSTRAP_MODE);
    expect(report.mutationPolicy).toBe("known-mutation-tools-blocked");
    expect(formatDoctorReport(report)).toBe(
      [
        "@jwilger/tiber 0.0.0",
        "Mode: read-only-bootstrap",
        "Node: v22.23.1",
        "Repository: /workspace/tiber",
        "Mutation policy: known-mutation-tools-blocked",
      ].join("\n"),
    );
  });
});
