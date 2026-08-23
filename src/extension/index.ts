import {
  getAgentDir,
  type ExtensionAPI,
} from "@earendil-works/pi-coding-agent";

import { verifyFileContainment } from "../adapters/containment/file-containment-verifier.js";
import { FileAuthorityStore } from "../adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import {
  formatContainment,
  type ContainmentStatus,
} from "../core/containment/containment.js";
import { applyAssuranceCeiling } from "../core/configuration/authority.js";
import { authorizeBootstrapTool } from "../core/doctor/bootstrap-policy.js";
import { verifyToolInventory } from "../core/tools/tool-policy.js";
import {
  createDoctorReport,
  formatDoctorReport,
} from "../core/doctor/report.js";
import { registerGovernedTools } from "./governed-tools.js";
import { readPackageVersion } from "./package-version.js";
import { handleSettingsCommand } from "./settings-command.js";

export default function registerTiber(pi: ExtensionAPI): void {
  const packageVersion = readPackageVersion();
  registerGovernedTools(pi);
  let containment: ContainmentStatus = {
    state: "lockdown",
    level: "host-trusted",
    code: "TIBER_CONTAINMENT_NOT_INITIALIZED",
    detail: "Containment has not been evaluated",
  };

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

  pi.registerCommand("tiber:settings", {
    description: "Inspect or edit inherited Tiber settings",
    handler: handleSettingsCommand,
  });

  pi.registerCommand("tiber:containment", {
    description: "Show verified containment or lockdown evidence",
    handler: (_arguments, context) => {
      context.ui.notify(formatContainment(containment), "info");
      return Promise.resolve();
    },
  });

  pi.on("session_start", (_event, context) => {
    pi.setActiveTools(["read", "bash", "edit", "write"]);
    const agentDirectory = getAgentDir();
    const settings = new FileSettingsStore(agentDirectory, context.cwd).load();
    const authority = new FileAuthorityStore(agentDirectory).load();
    if (!settings.ok || !authority.ok) {
      containment = {
        state: "lockdown",
        level: "host-trusted",
        code: "TIBER_CONTAINMENT_CONFIGURATION_INVALID",
        detail: "Settings could not be parsed for containment evaluation",
      };
    } else {
      const requested =
        settings.value.projectValues.assuranceLevel ??
        settings.value.globalValues.assuranceLevel ??
        "host-trusted";
      const effective = applyAssuranceCeiling(
        requested,
        authority.value.ceilings.minimumAssuranceLevel,
      ).effective;
      containment = verifyFileContainment(
        effective,
        context.cwd,
        agentDirectory,
      );
      const inventory = verifyToolInventory(pi.getActiveTools());
      if (effective !== "host-trusted" && !inventory.allowed) {
        containment = {
          state: "lockdown",
          level: effective,
          code: inventory.code,
          detail: inventory.detail,
        };
      }
    }
    context.ui.setStatus(
      "tiber",
      containment.state === "verified"
        ? `Tiber: ${containment.level}`
        : "Tiber: containment lockdown",
    );
  });

  pi.on("before_agent_start", (_event, context) => {
    if (containment.level !== "host-trusted") {
      const inventory = verifyToolInventory(pi.getActiveTools());
      if (!inventory.allowed) {
        containment = {
          state: "lockdown",
          level: containment.level,
          code: inventory.code,
          detail: inventory.detail,
        };
      }
    }
    if (containment.state === "lockdown") {
      context.abort();
      context.ui.notify(formatContainment(containment), "error");
    }
  });

  pi.on("tool_call", (event) => {
    if (containment.state === "lockdown") {
      return {
        block: true,
        reason: `${containment.code}: effects are disabled during containment lockdown`,
      };
    }
    return authorizeBootstrapTool(event.toolName);
  });
}
