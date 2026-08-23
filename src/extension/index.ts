import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { authorizeBootstrapTool } from "../core/doctor/bootstrap-policy.js";
import {
  createDoctorReport,
  formatDoctorReport,
} from "../core/doctor/report.js";
import { readPackageVersion } from "./package-version.js";

export default function registerTiber(pi: ExtensionAPI): void {
  const packageVersion = readPackageVersion();

  pi.registerCommand("tiber:doctor", {
    description: "Show Tiber installation and safety status",
    handler: (_args, context) => {
      const report = createDoctorReport({
        cwd: context.cwd,
        nodeVersion: process.version,
        packageVersion,
      });

      context.ui.notify(formatDoctorReport(report), "info");
      return Promise.resolve();
    },
  });

  pi.on("session_start", (_event, context) => {
    context.ui.setStatus("tiber", "Tiber: read-only bootstrap");
  });

  pi.on("tool_call", (event) => authorizeBootstrapTool(event.toolName));
}
