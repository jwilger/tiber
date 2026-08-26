import { realpathSync } from "node:fs";

import {
  getAgentDir,
  type ExtensionAPI,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileCampaignStore } from "../adapters/campaigns/file-campaign-store.js";
import { FileCiAuthorityStore } from "../adapters/ci/file-ci-authority-store.js";
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
import {
  parseSetupRepositoryPath,
  type SetupRepositoryPath,
} from "../core/configuration/setup-values.js";
import { parseCampaignCheckpointTime } from "../core/campaigns/campaign.js";
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
import {
  handleAttentionCommand,
  handleCampaignCommand,
} from "./campaign-command.js";
import { handleCiCommand } from "./ci-command.js";
import { handleCommandGrant } from "./command-grant.js";
import { handleDeliveryCommand } from "./delivery-command.js";
import { handleDoneCommand } from "./done-command.js";
import { handleFinalReviewCommand } from "./final-review-command.js";
import { handleExceptionCommand } from "./exception-command.js";
import { registerExceptionRequestTool } from "./exception-request-tool.js";
import { handleGreenCommand } from "./green-command.js";
import { registerHeadroomCompaction } from "./headroom-compaction.js";
import { registerHindsightMemory } from "./hindsight-memory.js";
import { registerCommandTools } from "./command-tools.js";
import { registerContext7Tools } from "./context7-tools.js";
import { registerGovernedTools } from "./governed-tools.js";
import { readPackageVersion } from "./package-version.js";
import { handleRedCommand } from "./red-command.js";
import { handleReviewCommand } from "./review-command.js";
import { handleSettingsCommand } from "./settings-command.js";
import { reconcilePendingSetup, registerSetupTool } from "./setup-tool.js";
import { registerTaskCommands } from "./task-commands.js";
import { handleWorkCommand } from "./work-command.js";
import { registerAutomaticWorkflowOrchestration } from "./workflow-orchestration.js";
import { registerWorkflowRequestTool } from "./workflow-request-tool.js";

const TIBER_LOCKDOWN_COMMANDS = new Set([
  "tiber-setup",
  "tiber:attention",
  "tiber:commands",
  "tiber:containment",
  "tiber:doctor",
  "tiber:settings",
  "tiber:tasks",
]);

const ACTIVE_TIBER_TOOLS = [
  "read",
  "bash",
  "edit",
  "write",
  "tiber_command",
  "tiber_process",
  "tiber_artifact_range",
  "tiber_artifact_search",
  "tiber_exception_request",
  "tiber_workflow_request",
] as const;

function setupRepositoryMatches(
  active: SetupRepositoryPath | undefined,
  cwd: string,
): boolean {
  if (active === undefined) return false;
  try {
    const repository = parseSetupRepositoryPath(realpathSync(cwd));
    return repository.ok && repository.value === active;
  } catch {
    return false;
  }
}

function evaluateContainment(pi: ExtensionAPI, cwd: string): ContainmentStatus {
  const agentDirectory = getAgentDir();
  const settings = new FileSettingsStore(agentDirectory, cwd).load();
  const authority = new FileAuthorityStore(agentDirectory).load();
  if (!settings.ok || !authority.ok) {
    return {
      state: "lockdown",
      level: "host-trusted",
      code: "TIBER_CONTAINMENT_CONFIGURATION_INVALID",
      detail: "Settings could not be parsed for containment evaluation",
    };
  }
  const requested = resolveSettings(
    settings.value.globalValues,
    settings.value.projectValues,
  ).assuranceLevel.value;
  const effective = applyAssuranceCeiling(
    requested,
    authority.value.ceilings.minimumAssuranceLevel,
  ).effective;
  const evaluated = verifyFileContainment(effective, cwd, agentDirectory);
  const inventory = verifyToolInventory(pi.getActiveTools());
  return effective !== "host-trusted" && !inventory.allowed
    ? {
        state: "lockdown",
        level: effective,
        code: inventory.code,
        detail: inventory.detail,
      }
    : evaluated;
}

export default function registerTiber(pi: ExtensionAPI): void {
  const packageVersion = readPackageVersion();
  let setupConversationRepository: SetupRepositoryPath | undefined;
  let containment: ContainmentStatus = {
    state: "lockdown",
    level: "host-trusted",
    code: "TIBER_CONTAINMENT_NOT_INITIALIZED",
    detail: "Containment has not been evaluated",
  };
  const commandAllowed = (
    command: string,
    context: ExtensionCommandContext,
  ): boolean => {
    if (setupConversationRepository !== undefined) {
      context.ui.notify(
        "TIBER_SETUP_IN_PROGRESS: finish or cancel guided setup before another command",
        "error",
      );
      return false;
    }
    if (
      containment.state === "lockdown" &&
      !TIBER_LOCKDOWN_COMMANDS.has(command)
    ) {
      context.ui.notify(formatContainment(containment), "error");
      return false;
    }
    return true;
  };
  const guardedCommand =
    (
      command: string,
      handler: (
        argumentsText: string,
        context: ExtensionCommandContext,
      ) => Promise<void>,
    ) =>
    (argumentsText: string, context: ExtensionCommandContext): Promise<void> =>
      commandAllowed(command, context)
        ? handler(argumentsText, context)
        : Promise.resolve();

  registerGovernedTools(pi);
  registerCommandTools(pi);
  registerContext7Tools(pi);
  registerTaskCommands(pi, commandAllowed);
  registerSetupTool(pi, {
    beginConversation(repository) {
      setupConversationRepository = repository;
    },
    endConversation(repository) {
      setupConversationRepository = undefined;
      pi.setActiveTools([...ACTIVE_TIBER_TOOLS]);
      containment = evaluateContainment(pi, repository);
      return containment;
    },
    isConversationActive(repository) {
      return setupConversationRepository === repository;
    },
  });
  registerWorkflowRequestTool(pi);
  registerExceptionRequestTool(pi);
  const ordinaryAgentContextAllowed = () =>
    containment.state === "verified" &&
    setupConversationRepository === undefined;
  registerAutomaticWorkflowOrchestration(pi, ordinaryAgentContextAllowed);
  registerHeadroomCompaction(pi, ordinaryAgentContextAllowed);
  registerHindsightMemory(pi, ordinaryAgentContextAllowed);

  pi.registerCommand("tiber:doctor", {
    description: "Show Tiber installation and safety status",
    handler: guardedCommand("tiber:doctor", (_args, context) => {
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
    }),
  });

  pi.registerCommand("tiber:settings", {
    description: "Inspect or edit inherited Tiber settings",
    handler: guardedCommand("tiber:settings", handleSettingsCommand),
  });

  pi.registerCommand("tiber:campaign", {
    description: "Start, advance, or inspect a bounded autonomous campaign",
    handler: guardedCommand("tiber:campaign", handleCampaignCommand),
  });

  pi.registerCommand("tiber:attention", {
    description: "Show non-modal campaign blocker attention",
    handler: guardedCommand("tiber:attention", handleAttentionCommand),
  });

  pi.registerCommand("tiber:ci", {
    description: "Observe every required exact-revision CI authority",
    handler: guardedCommand("tiber:ci", handleCiCommand),
  });

  pi.registerCommand("tiber:commands", {
    description: "Grant the exact project structured command catalog",
    handler: guardedCommand("tiber:commands", handleCommandGrant),
  });

  pi.registerCommand("tiber:deliver", {
    description: "Create and optionally push an exact signed Git delivery",
    handler: guardedCommand("tiber:deliver", handleDeliveryCommand),
  });

  pi.registerCommand("tiber:done", {
    description: "Release, clean up, and mark an exactly reviewed task Done",
    handler: guardedCommand("tiber:done", handleDoneCommand),
  });

  pi.registerCommand("tiber:exception", {
    description: "Inspect and approve one exact short-lived human exception",
    handler: guardedCommand("tiber:exception", handleExceptionCommand),
  });

  pi.registerCommand("tiber:final-review", {
    description:
      "Run full verification and one complete final review iteration",
    handler: guardedCommand("tiber:final-review", handleFinalReviewCommand),
  });

  pi.registerCommand("tiber:green", {
    description:
      "Observe exact GREEN, run fresh review, and preserve a signed increment",
    handler: guardedCommand("tiber:green", handleGreenCommand),
  });

  pi.registerCommand("tiber:review", {
    description: "Open and observe an exact review-service delivery",
    handler: guardedCommand("tiber:review", handleReviewCommand),
  });

  pi.registerCommand("tiber:red", {
    description: "Observe and independently classify one exact scenario RED",
    handler: guardedCommand("tiber:red", handleRedCommand),
  });

  pi.registerCommand("tiber:work", {
    description: "Claim a Ready task and pin its baseline workflow run",
    handler: guardedCommand("tiber:work", handleWorkCommand),
  });

  pi.registerCommand("tiber:containment", {
    description: "Show verified containment or lockdown evidence",
    handler: guardedCommand("tiber:containment", (_arguments, context) => {
      context.ui.notify(formatContainment(containment), "info");
      return Promise.resolve();
    }),
  });

  pi.on("session_start", (_event, context) => {
    setupConversationRepository = undefined;
    pi.setActiveTools([...ACTIVE_TIBER_TOOLS]);
    const agentDirectory = getAgentDir();
    const processes = new FileProcessGroupRegistry(agentDirectory).reconcile();
    if (!processes.ok) {
      context.ui.notify(
        `${processes.failure.code}: ${processes.failure.message}`,
        "error",
      );
    }
    const setupRecovery = context.isProjectTrusted()
      ? reconcilePendingSetup(agentDirectory, context.cwd)
      : ({ ok: true, value: "none" } as const);
    const evaluatedContainment = evaluateContainment(pi, context.cwd);
    containment = setupRecovery.ok
      ? evaluatedContainment
      : {
          state: "lockdown",
          level: evaluatedContainment.level,
          code: "TIBER_CONTAINMENT_CONFIGURATION_INVALID",
          detail: "A confirmed setup application could not be recovered",
        };
    if (!setupRecovery.ok) {
      context.ui.notify(
        [
          `${setupRecovery.failure.code}: ${setupRecovery.failure.message}`,
          ...setupRecovery.failure.causes.map(
            (cause) => `${cause.code}: ${cause.safeSummary}`,
          ),
        ].join("\n"),
        "error",
      );
    } else if (setupRecovery.value === "recovered") {
      context.ui.notify("Recovered confirmed Tiber setup application", "info");
    }
    context.ui.setStatus(
      "tiber",
      containment.state === "verified"
        ? `Tiber: ${containment.level}`
        : "Tiber: containment lockdown",
    );
    try {
      const ciHold = new FileCiAuthorityStore(
        context.cwd,
        agentDirectory,
      ).readHold();
      if (!ciHold.ok) {
        context.ui.notify(ciHold.failure.code, "error");
        context.ui.setStatus("tiber", "Tiber: invalid CI hold state");
      } else if (ciHold.value.kind === "some") {
        context.ui.setStatus("tiber", "Tiber: CI delivery hold");
      }
    } catch {
      // A non-repository session has no repository-wide CI state.
    }
  });

  pi.on("session_shutdown", (_event, context) => {
    const agentDirectory = getAgentDir();
    const shutdownTime = parseCampaignCheckpointTime(new Date().toISOString());
    if (shutdownTime.ok)
      new FileCampaignStore(agentDirectory, context.cwd).shutdown(
        shutdownTime.value,
      );
    new FileProcessGroupRegistry(agentDirectory).terminateAll();
  });

  pi.on("input", (event, context) => {
    const command = /^\/([^\s]+)/u.exec(event.text.trim())?.[1];
    if (setupRepositoryMatches(setupConversationRepository, context.cwd)) {
      if (command === undefined || command === "tiber-setup") return undefined;
      context.ui.notify(
        "TIBER_SETUP_IN_PROGRESS: finish or cancel guided setup before another command",
        "error",
      );
      return { action: "handled" };
    }
    if (containment.state !== "lockdown") return undefined;
    if (command !== undefined && TIBER_LOCKDOWN_COMMANDS.has(command))
      return undefined;
    context.ui.notify(formatContainment(containment), "error");
    return { action: "handled" };
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
    if (
      containment.state === "lockdown" &&
      !setupRepositoryMatches(setupConversationRepository, context.cwd)
    ) {
      context.abort();
      context.ui.notify(formatContainment(containment), "error");
    }
  });

  pi.on("tool_call", (event, context) => {
    if (
      setupConversationRepository !== undefined &&
      !setupRepositoryMatches(setupConversationRepository, context.cwd)
    )
      return {
        block: true,
        reason:
          "TIBER_SETUP_REPOSITORY_CHANGED: setup tools remain bound to the invoking repository",
      };
    if (containment.state === "lockdown") {
      if (
        setupRepositoryMatches(setupConversationRepository, context.cwd) &&
        (event.toolName === "read" || event.toolName === "tiber_setup")
      ) {
        return undefined;
      }
      return {
        block: true,
        reason: `${containment.code}: effects are disabled during containment lockdown`,
      };
    }
    const authorization = authorizeBootstrapTool(event.toolName);
    return authorization.kind === "some" ? authorization.value : undefined;
  });
}
