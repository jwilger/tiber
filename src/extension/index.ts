import {
  getAgentDir,
  type ExtensionAPI,
} from "@earendil-works/pi-coding-agent";

import { verifyFileContainment } from "../adapters/containment/file-containment-verifier.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { FileAuthorityStore } from "../adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import {
  formatContainment,
  type ContainmentStatus,
} from "../core/containment/containment.js";
import { applyAssuranceCeiling } from "../core/configuration/authority.js";
import { resolveSettings } from "../core/configuration/settings.js";
import { authorizeBootstrapTool } from "../core/doctor/bootstrap-policy.js";
import {
  parseDoctorNodeVersion,
  parseDoctorRepositoryPath,
} from "../core/doctor/doctor-values.js";
import { verifyToolInventory } from "../core/tools/tool-policy.js";
import {
  createDoctorReport,
  formatDoctorReport,
} from "../core/doctor/report.js";
import { handleCommandGrant } from "./command-grant.js";
import { handleDeliveryCommand } from "./delivery-command.js";
import { handleDoneCommand } from "./done-command.js";
import { handleFinalReviewCommand } from "./final-review-command.js";
import { handleGreenCommand } from "./green-command.js";
import { registerCommandTools } from "./command-tools.js";
import { registerGovernedTools } from "./governed-tools.js";
import { readPackageVersion } from "./package-version.js";
import { handleRedCommand } from "./red-command.js";
import { handleSettingsCommand } from "./settings-command.js";
import { registerTaskCommands } from "./task-commands.js";
import { handleWorkCommand } from "./work-command.js";

export default function registerTiber(pi: ExtensionAPI): void {
  const packageVersion = readPackageVersion();
  registerGovernedTools(pi);
  registerCommandTools(pi);
  registerTaskCommands(pi);
  let containment: ContainmentStatus = {
    state: "lockdown",
    level: "host-trusted",
    code: "TIBER_CONTAINMENT_NOT_INITIALIZED",
    detail: "Containment has not been evaluated",
  };

  pi.registerCommand("tiber:doctor", {
    description: "Show Tiber installation and safety status",
    handler: (_args, context) => {
      const cwd = parseDoctorRepositoryPath(context.cwd);
      const nodeVersion = parseDoctorNodeVersion(process.version);
      if (!cwd.ok || !nodeVersion.ok) {
        context.ui.notify("TIBER_DOCTOR_VALUE_INVALID", "error");
        return Promise.resolve();
      }
      const report = createDoctorReport({
        cwd: cwd.value,
        nodeVersion: nodeVersion.value,
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

  pi.registerCommand("tiber:commands", {
    description: "Grant the exact project structured command catalog",
    handler: handleCommandGrant,
  });

  pi.registerCommand("tiber:deliver", {
    description: "Create and optionally push an exact signed Git delivery",
    handler: handleDeliveryCommand,
  });

  pi.registerCommand("tiber:done", {
    description: "Release, clean up, and mark an exactly reviewed task Done",
    handler: handleDoneCommand,
  });

  pi.registerCommand("tiber:final-review", {
    description:
      "Run full verification and one complete final review iteration",
    handler: handleFinalReviewCommand,
  });

  pi.registerCommand("tiber:green", {
    description:
      "Observe exact GREEN, run fresh review, and preserve a signed increment",
    handler: handleGreenCommand,
  });

  pi.registerCommand("tiber:red", {
    description: "Observe and independently classify one exact scenario RED",
    handler: handleRedCommand,
  });

  pi.registerCommand("tiber:work", {
    description: "Claim a Ready task and pin its baseline workflow run",
    handler: handleWorkCommand,
  });

  pi.registerCommand("tiber:containment", {
    description: "Show verified containment or lockdown evidence",
    handler: (_arguments, context) => {
      context.ui.notify(formatContainment(containment), "info");
      return Promise.resolve();
    },
  });

  pi.on("session_start", (_event, context) => {
    pi.setActiveTools([
      "read",
      "bash",
      "edit",
      "write",
      "tiber_command",
      "tiber_artifact_range",
      "tiber_artifact_search",
    ]);
    const agentDirectory = getAgentDir();
    const processes = new FileProcessGroupRegistry(agentDirectory).reconcile();
    if (!processes.ok) {
      context.ui.notify(
        `${processes.failure.code}: ${processes.failure.message}`,
        "error",
      );
    }
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
      const requested = resolveSettings(
        settings.value.globalValues,
        settings.value.projectValues,
      ).assuranceLevel.value;
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

  pi.on("session_shutdown", () => {
    new FileProcessGroupRegistry(getAgentDir()).terminateAll();
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
    const authorization = authorizeBootstrapTool(event.toolName);
    return authorization.kind === "some" ? authorization.value : undefined;
  });
}
