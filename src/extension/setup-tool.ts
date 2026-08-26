import { execFileSync } from "node:child_process";
import {
  accessSync,
  constants,
  existsSync,
  readFileSync,
  statSync,
} from "node:fs";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

import { StringEnum } from "@earendil-works/pi-ai";
import {
  getAgentDir,
  withFileMutationQueue,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type, type Static } from "typebox";

import { FileCiAuthorityStore } from "../adapters/ci/file-ci-authority-store.js";
import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { verifyFileContainment } from "../adapters/containment/file-containment-verifier.js";
import { FileAuthorityStore } from "../adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import {
  applyAssuranceCeiling,
  formatAuthority,
} from "../core/configuration/authority.js";
import {
  ASSURANCE_LEVELS,
  BUILT_IN_SETTINGS,
  formatSettingsTable,
  resolveSettings,
} from "../core/configuration/settings.js";
import {
  parseSetupPlan,
  requiredSetupConfirmations,
  type SetupPlan,
} from "../core/configuration/setup.js";

type SetupEnvironment = Readonly<Record<string, string | undefined>>;

function setupAgentPrompt(): string {
  const text = readFileSync(
    fileURLToPath(
      new URL("../../prompts/tiber-setup-agent.md", import.meta.url),
    ),
    "utf8",
  );
  const frontmatterEnd = text.indexOf("\n---\n", 4);
  if (!text.startsWith("---\n") || frontmatterEnd < 0) {
    throw new Error("Tiber setup prompt frontmatter is invalid");
  }
  return text.slice(frontmatterEnd + 5).trim();
}

type SetupInspectionResult =
  | { readonly ok: true; readonly value: Readonly<Record<string, unknown>> }
  | {
      readonly ok: false;
      readonly code: "TIBER_SETUP_INSPECTION_FAILED";
      readonly message: string;
    };

function gitConfig(repository: string, key: string): string | undefined {
  try {
    const value = execFileSync("git", ["config", "--get", key], {
      cwd: repository,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    return value.length === 0 ? undefined : value;
  } catch {
    return undefined;
  }
}

function discoverExecutable(
  name: string,
  environment: SetupEnvironment,
): string | undefined {
  for (const directory of (environment.PATH ?? "").split(delimiter)) {
    if (directory.length === 0) continue;
    const candidate = join(directory, name);
    try {
      accessSync(candidate, constants.X_OK);
      if (statSync(candidate).isFile()) return candidate;
    } catch {
      continue;
    }
  }
  return undefined;
}

function hasEveryEnvironmentValue(
  environment: SetupEnvironment,
  names: readonly string[],
): "disabled" | "partial" | "configured" {
  const present = names.filter(
    (name) => (environment[name]?.length ?? 0) > 0,
  ).length;
  return present === 0
    ? "disabled"
    : present === names.length
      ? "configured"
      : "partial";
}

export function inspectSetup(
  agentDirectory: string,
  repository: string,
  environment: SetupEnvironment = process.env,
): SetupInspectionResult {
  const settings = new FileSettingsStore(agentDirectory, repository).load();
  if (!settings.ok) {
    return {
      ok: false,
      code: "TIBER_SETUP_INSPECTION_FAILED",
      message: settings.failure.message,
    };
  }
  const authority = new FileAuthorityStore(agentDirectory).load();
  if (!authority.ok) {
    return {
      ok: false,
      code: "TIBER_SETUP_INSPECTION_FAILED",
      message: authority.failure.message,
    };
  }

  const resolved = resolveSettings(
    settings.value.globalValues,
    settings.value.projectValues,
  );
  const assurance = applyAssuranceCeiling(
    resolved.assuranceLevel.value,
    authority.value.ceilings.minimumAssuranceLevel,
  );
  const containment = verifyFileContainment(
    assurance.effective,
    repository,
    agentDirectory,
  );
  const commandPath = join(repository, ".tiber", "commands.json");
  const commandAuthority = new FileCommandAuthority(repository);
  const commandCatalog = existsSync(commandPath)
    ? commandAuthority.loadCatalog()
    : undefined;
  const commandCatalogStatus = (() => {
    if (commandCatalog === undefined) return { status: "missing" } as const;
    if (!commandCatalog.ok)
      return {
        status: "invalid",
        failure: commandCatalog.failure.message,
      } as const;
    const grant = commandAuthority.readGrant();
    const granted =
      grant.ok &&
      grant.value.kind === "some" &&
      grant.value.value === commandCatalog.value.digest;
    return {
      status: granted ? "granted" : "ungranted",
      digest: commandCatalog.value.digest,
      commands: commandCatalog.value.commands.map(({ name, purpose }) => ({
        name,
        purpose,
      })),
    } as const;
  })();
  const origin = gitConfig(repository, "remote.origin.url");
  const signingKeys = [
    "user.name",
    "user.email",
    "user.signingkey",
    "gpg.format",
    "gpg.ssh.allowedSignersFile",
  ];
  const signingReady = signingKeys.every(
    (key) => gitConfig(repository, key) !== undefined,
  );
  const ciStatus = ((): "missing" | "configured" => {
    try {
      return new FileCiAuthorityStore(repository, agentDirectory).loadCatalog()
        .ok
        ? "configured"
        : "missing";
    } catch {
      return "missing";
    }
  })();

  return {
    ok: true,
    value: {
      schemaVersion: 1,
      configurationCatalog: {
        settings: [
          {
            key: "assuranceLevel",
            layers: ["user-global", "project"],
            choices: ["inherit", ...ASSURANCE_LEVELS],
            recommendation: "host-trusted",
            effect:
              "host-trusted governs effects without claiming OS isolation; stronger levels require matching external attestation and Linux corroboration or enter lockdown",
          },
          {
            key: "outputPreviewBytes",
            layers: ["user-global", "project"],
            range: { minimum: 1_024, maximum: 1_048_576 },
            choices: ["inherit", "integer-in-range"],
            recommendation: 16_384,
            effect:
              "bounds inline command and documentation previews before content-addressed artifact virtualization",
          },
          {
            key: "worktreeMode",
            layers: ["user-global", "project"],
            choices: ["inherit", "isolated", "current"],
            recommendation: "isolated",
            effect:
              "isolated creates owned task worktrees; current performs governed work in the current checkout",
          },
        ],
        authority: {
          minimumAssuranceLevel: {
            choices: ["unlocked", ...ASSURANCE_LEVELS],
            effect:
              "a user-global floor prevents projects from requesting weaker containment; lowering or unlocking it requires exact human confirmation",
          },
          secretReferences: {
            value: "environment variable name only",
            effect:
              "secret values remain externally provisioned and never enter setup conversation or persisted Tiber documents",
          },
        },
        projectCommands: {
          choices: ["keep", "replace"],
          purposes: ["test", "verification"],
          effect:
            "a closed shell-free command catalog is compiled, written to .tiber/commands.json, and locally granted only after exact human digest confirmation",
        },
        externalCapabilities: {
          signing:
            "signed shared tasks require Git name, email, signing key, signing format, and allowed-signers configuration",
          origin: "shared task publication requires an origin remote",
          containment:
            "strong assurance requires externally signed containment evidence",
          ci: "delivery completion requires a private digest-pinned CI authority catalog",
          githubReview:
            "review delivery uses four separately provisioned GitHub token capabilities",
          context7:
            "Context7 network access is optional and requires explicit environment authority",
          hindsight:
            "Hindsight is optional and each memory bank has separate recall and retain authority",
        },
      },
      settings: {
        builtIn: BUILT_IN_SETTINGS,
        global: settings.value.globalValues,
        project: settings.value.projectValues,
        effective: {
          ...resolved,
          assuranceLevel: {
            value: assurance.effective,
            source: resolved.assuranceLevel.source,
          },
        },
      },
      authority: authority.value,
      commandCatalog: commandCatalogStatus,
      prerequisites: {
        executables: {
          node: process.execPath,
          git: discoverExecutable("git", environment),
          npm: discoverExecutable("npm", environment),
          npx: discoverExecutable("npx", environment),
        },
        origin: {
          status: origin === undefined ? "missing" : "configured",
        },
        signing: { status: signingReady ? "configured" : "missing" },
        containment: {
          status: containment.state,
          level: containment.level,
          ...(containment.state === "lockdown"
            ? { code: containment.code, detail: containment.detail }
            : {}),
        },
      },
      integrations: {
        context7: {
          status:
            environment.TIBER_CONTEXT7_NETWORK === "enabled"
              ? "configured"
              : "disabled",
        },
        hindsight: {
          status:
            (environment.TIBER_HINDSIGHT_ENDPOINT?.length ?? 0) > 0
              ? "configured"
              : "disabled",
        },
        githubReview: {
          status: hasEveryEnvironmentValue(environment, [
            "TIBER_GITHUB_PR_TOKEN",
            "TIBER_GITHUB_REVIEW_TOKEN",
            "TIBER_GITHUB_CI_TOKEN",
            "TIBER_GITHUB_MERGE_TOKEN",
          ]),
        },
        ci: { status: ciStatus },
      },
    },
  };
}

export type SetupApplicationResult =
  | {
      readonly ok: true;
      readonly commandCatalog: "unchanged" | "granted";
    }
  | {
      readonly ok: false;
      readonly code: "TIBER_SETUP_APPLY_FAILED";
      readonly message: string;
    };

function failed(message: string): SetupApplicationResult {
  return { ok: false, code: "TIBER_SETUP_APPLY_FAILED", message };
}

export function applySetupPlan(
  agentDirectory: string,
  repository: string,
  plan: SetupPlan,
): SetupApplicationResult {
  const settingsStore = new FileSettingsStore(agentDirectory, repository);
  const current = settingsStore.load();
  if (!current.ok) return failed(current.failure.message);

  const savedGlobal = settingsStore.saveGlobal(plan.globalSettings);
  if (!savedGlobal.ok) return failed(savedGlobal.failure.message);
  const savedProject = settingsStore.saveProject(
    current.value.projectId,
    plan.projectSettings,
  );
  if (!savedProject.ok) return failed(savedProject.failure.message);

  const savedAuthority = new FileAuthorityStore(agentDirectory).save(
    plan.authority,
  );
  if (!savedAuthority.ok) return failed(savedAuthority.failure.message);

  if (plan.commandCatalog.kind === "none") {
    return { ok: true, commandCatalog: "unchanged" };
  }

  const commands = new FileCommandAuthority(repository);
  const savedCatalog = commands.saveCatalog(plan.commandCatalog.value);
  if (!savedCatalog.ok) return failed(savedCatalog.failure.message);
  const granted = commands.grant(plan.commandCatalog.value.digest);
  return granted.ok
    ? { ok: true, commandCatalog: "granted" }
    : failed(granted.failure.message);
}

const setupToolSchema = Type.Object({
  operation: StringEnum(["inspect", "apply"] as const),
  plan: Type.Optional(Type.Unknown()),
});
type SetupToolInput = Static<typeof setupToolSchema>;

function setupResponse(
  text: string,
  disposition: "inspected" | "applied" | "denied" | "cancelled",
) {
  return {
    content: [{ type: "text" as const, text }],
    details: { disposition },
  };
}

function planSummary(plan: SetupPlan): string {
  return [
    formatSettingsTable(plan.globalSettings, plan.projectSettings),
    "",
    formatAuthority(
      plan.authority,
      resolveSettings(plan.globalSettings, plan.projectSettings).assuranceLevel
        .value,
    ),
    "",
    plan.commandCatalog.kind === "none"
      ? "Project command catalog: keep current declaration and local grant state"
      : `Project command catalog: replace with ${String(plan.commandCatalog.value.commands.length)} command(s)\n${plan.commandCatalog.value.commands
          .map((command) => `- ${command.name} (${command.purpose})`)
          .join("\n")}`,
  ].join("\n");
}

async function confirmAuthorityLoosening(
  current: SetupPlan,
  proposed: SetupPlan,
  context: ExtensionContext,
): Promise<boolean> {
  for (const phrase of requiredSetupConfirmations(current, proposed)) {
    const entered = await context.ui.input(
      "Confirm weaker Tiber authority",
      `${planSummary(proposed)}\n\nType: ${phrase}`,
    );
    if (entered !== phrase) return false;
  }
  return true;
}

function currentPlan(
  agentDirectory: string,
  repository: string,
): SetupPlan | undefined {
  const settings = new FileSettingsStore(agentDirectory, repository).load();
  const authority = new FileAuthorityStore(agentDirectory).load();
  if (!settings.ok || !authority.ok) return undefined;
  return {
    globalSettings: settings.value.globalValues,
    projectSettings: settings.value.projectValues,
    authority: authority.value,
    commandCatalog: { kind: "none" },
  };
}

export interface SetupToolHost {
  readonly beginConversation: () => void;
  readonly setupApplied: (repository: string) => {
    readonly state: "verified" | "lockdown";
    readonly level: string;
  };
}

async function applyRequestedSetup(
  parameters: SetupToolInput,
  context: ExtensionContext,
  agentDirectory: string,
  host: SetupToolHost,
) {
  if (!context.hasUI || !context.isProjectTrusted()) {
    return setupResponse(
      "TIBER_SETUP_HUMAN_REQUIRED: applying setup requires an interactive trusted project",
      "denied",
    );
  }
  const parsed = parseSetupPlan(parameters.plan);
  if (!parsed.ok) {
    return setupResponse(
      `${parsed.failure.code}: ${parsed.failure.message}`,
      "denied",
    );
  }
  const current = currentPlan(agentDirectory, context.cwd);
  if (
    current === undefined ||
    !(await confirmAuthorityLoosening(current, parsed.value, context))
  ) {
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  }
  const approved = await context.ui.confirm(
    "Apply complete Tiber setup?",
    planSummary(parsed.value),
  );
  if (!approved) {
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  }
  if (parsed.value.commandCatalog.kind === "some") {
    const phrase = `grant commands ${parsed.value.commandCatalog.value.digest}`;
    const entered = await context.ui.input(
      "Grant exact project commands",
      `${parsed.value.commandCatalog.value.canonicalJson}\n\nType: ${phrase}`,
    );
    if (entered !== phrase) {
      return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
    }
  }

  const applied = await withFileMutationQueue(
    join(context.cwd, ".tiber", "commands.json"),
    () =>
      Promise.resolve(
        applySetupPlan(agentDirectory, context.cwd, parsed.value),
      ),
  );
  if (!applied.ok) {
    return setupResponse(`${applied.code}: ${applied.message}`, "denied");
  }
  const observed = inspectSetup(agentDirectory, context.cwd);
  if (!observed.ok) {
    return setupResponse(`${observed.code}: ${observed.message}`, "denied");
  }
  const containment = host.setupApplied(context.cwd);
  context.ui.setStatus(
    "tiber",
    containment.state === "verified"
      ? `Tiber: ${containment.level}`
      : "Tiber: containment lockdown",
  );
  return setupResponse(
    `Tiber setup applied and observed\n${JSON.stringify(observed.value, null, 2)}`,
    "applied",
  );
}

export function registerSetupTool(pi: ExtensionAPI, host: SetupToolHost): void {
  let applyInProgress = false;
  pi.registerCommand("tiber-setup", {
    description: "Configure or reconfigure Tiber in one guided conversation",
    handler: (_arguments, context) => {
      if (!context.hasUI) {
        context.ui.notify(
          "TIBER_SETUP_HUMAN_REQUIRED: guided setup requires interactive Pi",
          "error",
        );
        return Promise.resolve();
      }
      host.beginConversation();
      context.ui.notify("Starting guided Tiber setup", "info");
      pi.setActiveTools(["read", "tiber_setup"]);
      pi.sendUserMessage(setupAgentPrompt());
      return Promise.resolve();
    },
  });

  pi.registerTool({
    name: "tiber_setup",
    label: "Tiber guided setup",
    description:
      "Inspect the complete closed Tiber configuration catalog or apply one complete user-approved setup plan. Inspection exposes choices, effects, current layered values, declarations, and external blockers without secret values. Apply requires project trust and independent interactive host confirmation; command grants and assurance loosening require exact human phrases.",
    promptSnippet:
      "Inspect and apply complete conversational Tiber setup without manual file editing",
    promptGuidelines: [
      "Use tiber_setup only for guided Tiber setup or reconfiguration. Inspect before proposing changes, ask about every catalog area, never request secret values, and call apply only after the user approves a complete preview.",
    ],
    parameters: setupToolSchema,
    async execute(
      _toolCallId,
      parameters: SetupToolInput,
      _signal,
      _onUpdate,
      context,
    ) {
      const agentDirectory = getAgentDir();
      if (parameters.operation === "inspect") {
        const inspected = inspectSetup(agentDirectory, context.cwd);
        return inspected.ok
          ? setupResponse(JSON.stringify(inspected.value, null, 2), "inspected")
          : setupResponse(`${inspected.code}: ${inspected.message}`, "denied");
      }

      if (applyInProgress) {
        return setupResponse(
          "TIBER_SETUP_IN_PROGRESS: wait for the active setup confirmation",
          "denied",
        );
      }
      applyInProgress = true;
      try {
        return await applyRequestedSetup(
          parameters,
          context,
          agentDirectory,
          host,
        );
      } finally {
        applyInProgress = false;
      }
    },
  });
}
