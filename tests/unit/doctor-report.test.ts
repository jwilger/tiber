import { describe, expect, it } from "vitest";

import {
  BOOTSTRAP_MODE,
  createDoctorReport,
  formatDoctorReport,
} from "../../src/core/doctor/report.js";

describe("doctor report", () => {
  it("describes the installed package and bootstrap safety mode", () => {
    const report = createDoctorReport({
      cwd: "/workspace/tiber",
      nodeVersion: "v22.23.1",
      packageVersion: "0.0.0",
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
