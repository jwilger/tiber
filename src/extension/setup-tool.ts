import { execFileSync } from "node:child_process";
import {
  accessSync,
  constants,
  existsSync,
  readFileSync,
  realpathSync,
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
import { FileSetupJournal } from "../adapters/settings/file-setup-journal.js";
import { FileWorkflowConfiguration } from "../adapters/workflows/file-workflow-configuration.js";
import {
  COMMAND_CATALOG_LIMITS,
  COMMAND_ENVIRONMENT_NAME_PATTERN,
  COMMAND_NAME_PATTERN,
  type CommandCatalogDigest,
  type CommandName,
} from "../core/commands/command-values.js";
import type { ContainmentStatus } from "../core/containment/containment.js";
import {
  applyAssuranceCeiling,
  formatAuthority,
  type AuthorityDocument,
} from "../core/configuration/authority.js";
import { OUTPUT_PREVIEW_BYTES_RANGE } from "../core/configuration/configuration-values.js";
import {
  ASSURANCE_LEVELS,
  BUILT_IN_SETTINGS,
  formatSettingsTable,
  resolveSettings,
  WORKTREE_MODES,
  type EffectiveSettings,
  type SettingsOverrides,
} from "../core/configuration/settings.js";
import {
  parseSetupAgentDirectoryPath,
  parseSetupRepositoryPath,
  type SetupAgentDirectoryPath,
  type SetupPlanDigest,
  type SetupRepositoryPath,
} from "../core/configuration/setup-values.js";
import {
  formatSetupPlan,
  parseSetupPlan,
  requiredSetupConfirmations,
  sameSetupAuthorityState,
  setupAuthorityStateCanReconcile,
  SETUP_PLAN_LIMITS,
  type SetupPlan,
} from "../core/configuration/setup.js";
import {
  operationalFailure,
  type FailureCause,
  type FailureRetryability,
  type TiberFailure,
  type TiberResult,
} from "../core/failures/tiber-failure.js";
import {
  BUILT_IN_WORKFLOW,
  compileWorkflow,
  POLICY_FLOOR_STAGES,
} from "../core/workflow/workflow.js";
import type {
  CompiledWorkflowDigest,
  WorkflowDefinitionId,
  WorkflowStepId,
} from "../core/workflow/workflow-values.js";

type SetupEnvironment = Readonly<Record<string, string | undefined>>;

type SetupHostFailure = TiberFailure<
  | "TIBER_SETUP_APPLY_FAILED"
  | "TIBER_SETUP_CONFIGURATION_CHANGED"
  | "TIBER_SETUP_INSPECTION_FAILED",
  { readonly domain: "setup-host" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type SetupHostResult<Value> = TiberResult<Value, SetupHostFailure>;

function hostFailure(
  code: SetupHostFailure["code"],
  message: string,
  retryability: FailureRetryability,
  cause?: FailureCause,
): SetupHostResult<never> {
  const failure = operationalFailure(code, "setup-host", message, retryability);
  return {
    ok: false,
    failure: {
      ...failure,
      causes: cause === undefined ? [] : [cause],
    },
  };
}

function causedFailure(
  code: SetupHostFailure["code"],
  message: string,
  retryability: FailureRetryability,
  cause: { readonly code: string; readonly message: string },
): SetupHostResult<never> {
  return hostFailure(code, message, retryability, {
    code: cause.code,
    safeSummary: cause.message,
  });
}

function observedSetupPaths(
  agentDirectoryInput: unknown,
  repositoryInput: unknown,
  failureCode: SetupHostFailure["code"],
): SetupHostResult<{
  readonly agentDirectory: SetupAgentDirectoryPath;
  readonly repository: SetupRepositoryPath;
}> {
  try {
    const agentDirectory = parseSetupAgentDirectoryPath(
      typeof agentDirectoryInput === "string"
        ? realpathSync(agentDirectoryInput)
        : agentDirectoryInput,
    );
    const repository = parseSetupRepositoryPath(
      typeof repositoryInput === "string"
        ? realpathSync(repositoryInput)
        : repositoryInput,
    );
    return agentDirectory.ok && repository.ok
      ? {
          ok: true,
          value: {
            agentDirectory: agentDirectory.value,
            repository: repository.value,
          },
        }
      : hostFailure(
          failureCode,
          "setup paths are invalid",
          "retry-after-input",
        );
  } catch {
    return hostFailure(
      failureCode,
      "setup paths are unavailable",
      "retry-after-state-change",
    );
  }
}

function setupAgentPrompt(): SetupHostResult<string> {
  try {
    const text = readFileSync(
      fileURLToPath(
        new URL("../../prompts/tiber-setup-agent.md", import.meta.url),
      ),
      "utf8",
    );
    const frontmatterEnd = text.indexOf("\n---\n", 4);
    return text.startsWith("---\n") && frontmatterEnd >= 0
      ? { ok: true, value: text.slice(frontmatterEnd + 5).trim() }
      : hostFailure(
          "TIBER_SETUP_INSPECTION_FAILED",
          "packaged setup guidance is invalid",
          "not-retryable",
        );
  } catch {
    return hostFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "packaged setup guidance is unavailable",
      "not-retryable",
    );
  }
}

function gitConfig(
  repository: SetupRepositoryPath,
  key: string,
): string | undefined {
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

type SetupExecutableInspection =
  | { readonly status: "configured"; readonly path: string }
  | { readonly status: "missing" };

function discoverExecutable(
  name: string,
  environment: SetupEnvironment,
): SetupExecutableInspection {
  for (const directory of (environment.PATH ?? "").split(delimiter)) {
    if (directory.length === 0) continue;
    const candidate = join(directory, name);
    try {
      accessSync(candidate, constants.X_OK);
      if (statSync(candidate).isFile())
        return { status: "configured", path: candidate };
    } catch {
      continue;
    }
  }
  return { status: "missing" };
}

function configuredEnvironmentNames(
  environment: SetupEnvironment,
  names: readonly string[],
): {
  readonly status: "disabled" | "partial" | "environment-present";
  readonly configured: readonly string[];
  readonly missing: readonly string[];
} {
  const configured = names.filter(
    (name) => (environment[name]?.length ?? 0) > 0,
  );
  const missing = names.filter((name) => !configured.includes(name));
  return {
    status:
      configured.length === 0
        ? "disabled"
        : missing.length === 0
          ? "environment-present"
          : "partial",
    configured,
    missing,
  };
}

type SetupSettingCatalogEntry =
  | {
      readonly key: "assuranceLevel";
      readonly layers: readonly ["user-global", "project"];
      readonly choices: readonly string[];
      readonly recommendation: string;
      readonly effect: string;
    }
  | {
      readonly key: "outputPreviewBytes";
      readonly layers: readonly ["user-global", "project"];
      readonly choices: readonly (string | number)[];
      readonly range: { readonly minimum: number; readonly maximum: number };
      readonly recommendation: number;
      readonly effect: string;
    }
  | {
      readonly key: "worktreeMode";
      readonly layers: readonly ["user-global", "project"];
      readonly choices: readonly string[];
      readonly recommendation: string;
      readonly effect: string;
    };

interface SetupConfigurationCatalog {
  readonly settings: readonly SetupSettingCatalogEntry[];
  readonly authority: {
    readonly minimumAssuranceLevel: {
      readonly choices: readonly string[];
      readonly effect: string;
    };
    readonly secretReferences: {
      readonly value: string;
      readonly maximumEntries: number;
      readonly effect: string;
    };
  };
  readonly projectCommands: {
    readonly choices: readonly ["keep", "remove", "replace"];
    readonly purposes: readonly ["test", "verification"];
    readonly constraints: {
      readonly commandNamePattern: string;
      readonly environmentNamePattern: string;
      readonly maximumCommands: number;
      readonly maximumArguments: number;
      readonly maximumEnvironmentEntries: number;
      readonly maximumTextLength: number;
      readonly timeoutMilliseconds: {
        readonly minimum: number;
        readonly maximum: number;
      };
      readonly outputBytes: {
        readonly minimum: number;
        readonly maximum: number;
      };
    };
    readonly effect: string;
  };
  readonly projectWorkflow: {
    readonly choices: readonly ["keep", "built-in", "replace"];
    readonly builtIn: typeof BUILT_IN_WORKFLOW;
    readonly requiredStageOrder: typeof POLICY_FLOOR_STAGES;
    readonly effect: string;
  };
  readonly externalCapabilities: {
    readonly signing: string;
    readonly origin: string;
    readonly containment: string;
    readonly ci: string;
    readonly githubReview: string;
    readonly context7: string;
    readonly hindsight: string;
  };
}

interface SetupContainmentInspection {
  readonly status: ContainmentStatus["state"];
  readonly level: ContainmentStatus["level"];
  readonly code: ContainmentStatus["code"];
  readonly detail: string;
}

type SetupCommandCatalogInspection =
  | { readonly status: "missing" }
  | { readonly status: "invalid"; readonly failure: string }
  | {
      readonly status: "granted" | "ungranted";
      readonly digest: CommandCatalogDigest;
      readonly commands: readonly {
        readonly name: CommandName;
        readonly purpose: "test" | "verification";
      }[];
    };

type SetupWorkflowInspection =
  | { readonly status: "built-in"; readonly digest: CompiledWorkflowDigest }
  | { readonly status: "invalid"; readonly failure: string }
  | {
      readonly status: "configured";
      readonly id: WorkflowDefinitionId;
      readonly digest: CompiledWorkflowDigest;
      readonly stages: readonly WorkflowStepId[];
    };

export interface SetupInspection {
  readonly schemaVersion: 1;
  readonly configurationCatalog: SetupConfigurationCatalog;
  readonly settings: {
    readonly builtIn: typeof BUILT_IN_SETTINGS;
    readonly global: SettingsOverrides;
    readonly project: SettingsOverrides;
    readonly effective: EffectiveSettings;
  };
  readonly authority: AuthorityDocument;
  readonly commandCatalog: SetupCommandCatalogInspection;
  readonly projectWorkflow: SetupWorkflowInspection;
  readonly recovery:
    | { readonly status: "clean" }
    | {
        readonly status: "recovery-required";
        readonly planDigest: SetupPlanDigest;
        readonly approvedPlan: ReturnType<typeof formatSetupPlan>;
      };
  readonly prerequisites: {
    readonly executables: {
      readonly node: SetupExecutableInspection;
      readonly git: SetupExecutableInspection;
      readonly npm: SetupExecutableInspection;
      readonly npx: SetupExecutableInspection;
    };
    readonly origin: { readonly status: "missing" | "configured" };
    readonly signing: {
      readonly status: "missing" | "configured";
      readonly missing: readonly string[];
    };
    readonly containment: SetupContainmentInspection;
  };
  readonly integrations: {
    readonly context7: {
      readonly network: "configured" | "disabled";
      readonly endpoint: "default" | "configured";
      readonly apiKey: "present" | "missing";
    };
    readonly hindsight: {
      readonly endpoint: "configured" | "disabled";
      readonly sharedBank: "configured" | "missing";
      readonly apiKey: "present" | "missing";
      readonly permissions: Readonly<
        Record<
          | "globalRecall"
          | "globalRetain"
          | "privateRecall"
          | "privateRetain"
          | "sharedRecall"
          | "sharedRetain",
          "configured" | "disabled"
        >
      >;
    };
    readonly githubReview: ReturnType<typeof configuredEnvironmentNames>;
    readonly ci: { readonly status: "configured" | "invalid" | "missing" };
  };
}

function inspectCommandCatalog(
  repository: SetupRepositoryPath,
): SetupHostResult<SetupCommandCatalogInspection> {
  const commandPath = join(repository, ".tiber", "commands.json");
  if (!existsSync(commandPath))
    return { ok: true, value: { status: "missing" } };
  try {
    const commandAuthority = new FileCommandAuthority(repository);
    const commandCatalog = commandAuthority.loadCatalog();
    if (!commandCatalog.ok)
      return {
        ok: true,
        value: {
          status: "invalid",
          failure: commandCatalog.failure.message,
        },
      };
    const grant = commandAuthority.readGrant();
    if (!grant.ok)
      return {
        ok: true,
        value: { status: "invalid", failure: grant.failure.message },
      };
    const granted =
      grant.value.kind === "some" &&
      grant.value.value === commandCatalog.value.digest;
    return {
      ok: true,
      value: {
        status: granted ? "granted" : "ungranted",
        digest: commandCatalog.value.digest,
        commands: commandCatalog.value.commands.map(({ name, purpose }) => ({
          name,
          purpose,
        })),
      },
    };
  } catch {
    return hostFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "project command authority could not be inspected",
      "transient",
    );
  }
}

function inspectProjectWorkflow(
  repository: SetupRepositoryPath,
): SetupHostResult<SetupWorkflowInspection> {
  const builtIn = compileWorkflow(BUILT_IN_WORKFLOW);
  if (!builtIn.ok)
    return causedFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "built-in workflow is invalid",
      "not-retryable",
      builtIn.failure,
    );
  const observed = new FileWorkflowConfiguration(repository).load();
  if (!observed.ok)
    return observed.failure.code === "TIBER_WORKFLOW_CONFIGURATION_INVALID"
      ? {
          ok: true,
          value: { status: "invalid", failure: observed.failure.message },
        }
      : causedFailure(
          "TIBER_SETUP_INSPECTION_FAILED",
          "project workflow could not be inspected",
          "transient",
          observed.failure,
        );
  if (observed.value.kind === "none")
    return {
      ok: true,
      value: { status: "built-in", digest: builtIn.value.digest },
    };
  return {
    ok: true,
    value: {
      status: "configured",
      id: observed.value.value.definition.id,
      digest: observed.value.value.digest,
      stages: observed.value.value.definition.stages,
    },
  };
}

function ciStatus(
  repository: SetupRepositoryPath,
  agentDirectory: SetupAgentDirectoryPath,
): "configured" | "invalid" | "missing" {
  if (!existsSync(join(agentDirectory, "tiber", "ci-authorities.v1.json")))
    return "missing";
  try {
    return new FileCiAuthorityStore(repository, agentDirectory).loadCatalog().ok
      ? "configured"
      : "invalid";
  } catch {
    return "invalid";
  }
}

function capabilityStatus(
  environment: SetupEnvironment,
  name: string,
): "configured" | "disabled" {
  return environment[name] === "enabled" ? "configured" : "disabled";
}

export function inspectSetup(
  agentDirectoryInput: unknown,
  repositoryInput: unknown,
  environment: SetupEnvironment = process.env,
): SetupHostResult<SetupInspection> {
  const paths = observedSetupPaths(
    agentDirectoryInput,
    repositoryInput,
    "TIBER_SETUP_INSPECTION_FAILED",
  );
  if (!paths.ok) return paths;
  const { agentDirectory, repository } = paths.value;
  try {
    const settings = new FileSettingsStore(agentDirectory, repository).load();
    if (!settings.ok)
      return causedFailure(
        "TIBER_SETUP_INSPECTION_FAILED",
        "layered settings could not be inspected",
        settings.failure.retryability,
        settings.failure,
      );
    const authority = new FileAuthorityStore(agentDirectory).load();
    if (!authority.ok)
      return causedFailure(
        "TIBER_SETUP_INSPECTION_FAILED",
        "user-local authority could not be inspected",
        authority.failure.retryability,
        authority.failure,
      );

    const pendingSetup = new FileSetupJournal(
      agentDirectory,
      repository,
      settings.value.projectId,
    ).loadPending();
    if (!pendingSetup.ok)
      return causedFailure(
        "TIBER_SETUP_INSPECTION_FAILED",
        "pending setup recovery state could not be inspected",
        pendingSetup.failure.retryability,
        pendingSetup.failure,
      );

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
    const commandCatalog = inspectCommandCatalog(repository);
    if (!commandCatalog.ok) return commandCatalog;
    const projectWorkflow = inspectProjectWorkflow(repository);
    if (!projectWorkflow.ok) return projectWorkflow;
    const origin = gitConfig(repository, "remote.origin.url");
    const signingKeys = [
      "user.name",
      "user.email",
      "user.signingkey",
      "gpg.format",
      "gpg.ssh.allowedSignersFile",
    ];
    const missingSigningKeys = signingKeys.filter(
      (key) => gitConfig(repository, key) === undefined,
    );
    const githubEnvironmentNames = [
      "TIBER_GITHUB_PR_TOKEN",
      "TIBER_GITHUB_REVIEW_TOKEN",
      "TIBER_GITHUB_CI_TOKEN",
      "TIBER_GITHUB_MERGE_TOKEN",
    ];

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
              range: OUTPUT_PREVIEW_BYTES_RANGE,
              choices: ["inherit", "integer-in-range"],
              recommendation: BUILT_IN_SETTINGS.outputPreviewBytes,
              effect:
                "bounds inline command and documentation previews before content-addressed artifact virtualization",
            },
            {
              key: "worktreeMode",
              layers: ["user-global", "project"],
              choices: ["inherit", ...WORKTREE_MODES],
              recommendation: BUILT_IN_SETTINGS.worktreeMode,
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
              maximumEntries: SETUP_PLAN_LIMITS.maximumSecretReferences,
              effect:
                "secret values remain externally provisioned and never enter setup conversation or persisted Tiber documents",
            },
          },
          projectCommands: {
            choices: ["keep", "remove", "replace"],
            purposes: ["test", "verification"],
            constraints: {
              commandNamePattern: COMMAND_NAME_PATTERN.source,
              environmentNamePattern: COMMAND_ENVIRONMENT_NAME_PATTERN.source,
              ...COMMAND_CATALOG_LIMITS,
            },
            effect:
              "a closed shell-free command catalog is compiled, written to .tiber/commands.json, and locally granted only after exact human digest confirmation",
          },
          projectWorkflow: {
            choices: ["keep", "built-in", "replace"],
            builtIn: BUILT_IN_WORKFLOW,
            requiredStageOrder: POLICY_FLOOR_STAGES,
            effect:
              "a project workflow may add bounded stages but must preserve the immutable policy-floor stage order; built-in removes the project override",
          },
          externalCapabilities: {
            signing:
              "signed shared tasks require Git name, email, signing key, signing format, and allowed-signers configuration",
            origin: "shared task publication requires an origin remote",
            containment:
              "strong assurance requires externally signed containment evidence",
            ci: "delivery completion requires a separately provisioned private digest-pinned CI authority catalog",
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
        commandCatalog: commandCatalog.value,
        projectWorkflow: projectWorkflow.value,
        recovery:
          pendingSetup.value.kind === "none"
            ? { status: "clean" }
            : {
                status: "recovery-required",
                planDigest: pendingSetup.value.value.planDigest,
                approvedPlan: formatSetupPlan(pendingSetup.value.value.plan),
              },
        prerequisites: {
          executables: {
            node: { status: "configured", path: process.execPath },
            git: discoverExecutable("git", environment),
            npm: discoverExecutable("npm", environment),
            npx: discoverExecutable("npx", environment),
          },
          origin: {
            status: origin === undefined ? "missing" : "configured",
          },
          signing: {
            status: missingSigningKeys.length === 0 ? "configured" : "missing",
            missing: missingSigningKeys,
          },
          containment: {
            status: containment.state,
            level: containment.level,
            code: containment.code,
            detail: containment.detail,
          },
        },
        integrations: {
          context7: {
            network: capabilityStatus(environment, "TIBER_CONTEXT7_NETWORK"),
            endpoint:
              (environment.TIBER_CONTEXT7_ENDPOINT?.length ?? 0) > 0
                ? "configured"
                : "default",
            apiKey:
              (environment.CONTEXT7_API_KEY?.length ?? 0) > 0
                ? "present"
                : "missing",
          },
          hindsight: {
            endpoint:
              (environment.TIBER_HINDSIGHT_ENDPOINT?.length ?? 0) > 0
                ? "configured"
                : "disabled",
            sharedBank:
              (environment.TIBER_HINDSIGHT_SHARED_BANK?.length ?? 0) > 0
                ? "configured"
                : "missing",
            apiKey:
              (environment.HINDSIGHT_API_KEY?.length ?? 0) > 0
                ? "present"
                : "missing",
            permissions: {
              globalRecall: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_GLOBAL_RECALL",
              ),
              globalRetain: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_GLOBAL_RETAIN",
              ),
              privateRecall: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_PRIVATE_RECALL",
              ),
              privateRetain: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_PRIVATE_RETAIN",
              ),
              sharedRecall: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_SHARED_RECALL",
              ),
              sharedRetain: capabilityStatus(
                environment,
                "TIBER_HINDSIGHT_SHARED_RETAIN",
              ),
            },
          },
          githubReview: configuredEnvironmentNames(
            environment,
            githubEnvironmentNames,
          ),
          ci: { status: ciStatus(repository, agentDirectory) },
        },
      },
    };
  } catch {
    return hostFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "setup inspection could not be completed",
      "transient",
    );
  }
}

export interface SetupApplicationReceipt {
  readonly commandCatalog: "unchanged" | "granted" | "removed";
  readonly projectWorkflow: "unchanged" | "built-in" | "replaced";
}

function observeCurrentPlan(
  agentDirectory: SetupAgentDirectoryPath,
  repository: SetupRepositoryPath,
): SetupHostResult<SetupPlan> {
  const settings = new FileSettingsStore(agentDirectory, repository).load();
  if (!settings.ok)
    return causedFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "current settings could not be observed",
      settings.failure.retryability,
      settings.failure,
    );
  const authority = new FileAuthorityStore(agentDirectory).load();
  if (!authority.ok)
    return causedFailure(
      "TIBER_SETUP_INSPECTION_FAILED",
      "current authority could not be observed",
      authority.failure.retryability,
      authority.failure,
    );
  return {
    ok: true,
    value: {
      globalSettings: settings.value.globalValues,
      projectSettings: settings.value.projectValues,
      authority: authority.value,
      commandCatalog: { kind: "keep" },
      projectWorkflow: { kind: "keep" },
    },
  };
}

function currentPlan(
  agentDirectoryInput: unknown,
  repositoryInput: unknown,
): SetupHostResult<SetupPlan> {
  const paths = observedSetupPaths(
    agentDirectoryInput,
    repositoryInput,
    "TIBER_SETUP_INSPECTION_FAILED",
  );
  return paths.ok
    ? observeCurrentPlan(paths.value.agentDirectory, paths.value.repository)
    : paths;
}

function mapApplyFailure(cause: {
  readonly code: string;
  readonly message: string;
  readonly retryability: FailureRetryability;
}): SetupHostResult<never> {
  return causedFailure(
    "TIBER_SETUP_APPLY_FAILED",
    "complete setup could not be applied",
    cause.retryability,
    cause,
  );
}

function keptDeclarationsAreValid(
  repository: SetupRepositoryPath,
  plan: SetupPlan,
): boolean {
  if (plan.commandCatalog.kind === "keep") {
    const commandPath = join(repository, ".tiber", "commands.json");
    if (existsSync(commandPath)) {
      const commands = new FileCommandAuthority(repository);
      if (!commands.loadCatalog().ok || !commands.readGrant().ok) return false;
    }
  }
  return (
    plan.projectWorkflow.kind !== "keep" ||
    new FileWorkflowConfiguration(repository).load().ok
  );
}

function declarationsMatchPlan(
  repository: SetupRepositoryPath,
  plan: SetupPlan,
): boolean {
  const commandMatches = (() => {
    if (plan.commandCatalog.kind === "keep") return true;
    const commandPath = join(repository, ".tiber", "commands.json");
    if (plan.commandCatalog.kind === "remove") return !existsSync(commandPath);
    const commands = new FileCommandAuthority(repository);
    const catalog = commands.loadCatalog();
    const grant = commands.readGrant();
    return (
      catalog.ok &&
      catalog.value.digest === plan.commandCatalog.catalog.digest &&
      grant.ok &&
      grant.value.kind === "some" &&
      grant.value.value === plan.commandCatalog.catalog.digest
    );
  })();
  if (!commandMatches) return false;
  if (plan.projectWorkflow.kind === "keep") return true;
  const workflow = new FileWorkflowConfiguration(repository).load();
  return plan.projectWorkflow.kind === "built-in"
    ? workflow.ok && workflow.value.kind === "none"
    : workflow.ok &&
        workflow.value.kind === "some" &&
        workflow.value.value.digest === plan.projectWorkflow.workflow.digest;
}

function applyObservedSetupPlan(
  agentDirectory: SetupAgentDirectoryPath,
  repository: SetupRepositoryPath,
  expectedCurrent: SetupPlan,
  plan: SetupPlan,
  mode: "confirmed-current" | "recovery",
): SetupHostResult<SetupApplicationReceipt> {
  const settingsStore = new FileSettingsStore(agentDirectory, repository);
  const observedCurrent = observeCurrentPlan(agentDirectory, repository);
  if (!observedCurrent.ok) return observedCurrent;
  const currentMatches =
    mode === "confirmed-current"
      ? sameSetupAuthorityState(expectedCurrent, observedCurrent.value)
      : setupAuthorityStateCanReconcile(
          expectedCurrent,
          plan,
          observedCurrent.value,
        );
  if (!currentMatches)
    return hostFailure(
      "TIBER_SETUP_CONFIGURATION_CHANGED",
      "setup authority changed after confirmation; inspect and confirm the new state",
      "retry-after-state-change",
    );
  if (!keptDeclarationsAreValid(repository, plan))
    return hostFailure(
      "TIBER_SETUP_APPLY_FAILED",
      "invalid project declarations must be replaced or removed during setup",
      "retry-after-input",
    );
  const settings = settingsStore.load();
  if (!settings.ok) return mapApplyFailure(settings.failure);
  const journal = new FileSetupJournal(
    agentDirectory,
    repository,
    settings.value.projectId,
  );
  const intent = journal.begin(expectedCurrent, plan);
  if (!intent.ok) return mapApplyFailure(intent.failure);

  const savedGlobal = settingsStore.saveGlobal(plan.globalSettings);
  if (!savedGlobal.ok) return mapApplyFailure(savedGlobal.failure);
  const savedProject = settingsStore.saveProject(
    settings.value.projectId,
    plan.projectSettings,
  );
  if (!savedProject.ok) return mapApplyFailure(savedProject.failure);

  const savedAuthority = new FileAuthorityStore(agentDirectory).save(
    plan.authority,
  );
  if (!savedAuthority.ok) return mapApplyFailure(savedAuthority.failure);

  let commandCatalog: SetupApplicationReceipt["commandCatalog"] = "unchanged";
  const commands = new FileCommandAuthority(repository);
  if (plan.commandCatalog.kind === "remove") {
    const removed = commands.removeCatalog();
    if (!removed.ok) return mapApplyFailure(removed.failure);
    commandCatalog = "removed";
  } else if (plan.commandCatalog.kind === "replace") {
    const savedCatalog = commands.saveCatalog(plan.commandCatalog.catalog);
    if (!savedCatalog.ok) return mapApplyFailure(savedCatalog.failure);
    const granted = commands.grant(plan.commandCatalog.catalog.digest);
    if (!granted.ok) return mapApplyFailure(granted.failure);
    commandCatalog = "granted";
  }

  let projectWorkflow: SetupApplicationReceipt["projectWorkflow"] = "unchanged";
  const workflows = new FileWorkflowConfiguration(repository);
  if (plan.projectWorkflow.kind === "built-in") {
    const selected = workflows.useBuiltIn();
    if (!selected.ok) return mapApplyFailure(selected.failure);
    projectWorkflow = "built-in";
  } else if (plan.projectWorkflow.kind === "replace") {
    const saved = workflows.save(plan.projectWorkflow.workflow);
    if (!saved.ok) return mapApplyFailure(saved.failure);
    projectWorkflow = "replaced";
  }

  const observed = observeCurrentPlan(agentDirectory, repository);
  if (!observed.ok) return observed;
  if (
    !sameSetupAuthorityState(plan, observed.value) ||
    !declarationsMatchPlan(repository, plan)
  )
    return hostFailure(
      "TIBER_SETUP_APPLY_FAILED",
      "durable setup observation did not match the approved plan",
      "retry-after-state-change",
    );
  const completed = journal.complete(intent.value);
  return completed.ok
    ? { ok: true, value: { commandCatalog, projectWorkflow } }
    : mapApplyFailure(completed.failure);
}

export function applySetupPlan(
  agentDirectoryInput: unknown,
  repositoryInput: unknown,
  expectedCurrent: SetupPlan,
  plan: SetupPlan,
): SetupHostResult<SetupApplicationReceipt> {
  const paths = observedSetupPaths(
    agentDirectoryInput,
    repositoryInput,
    "TIBER_SETUP_APPLY_FAILED",
  );
  if (!paths.ok) return paths;
  try {
    return applyObservedSetupPlan(
      paths.value.agentDirectory,
      paths.value.repository,
      expectedCurrent,
      plan,
      "confirmed-current",
    );
  } catch {
    return hostFailure(
      "TIBER_SETUP_APPLY_FAILED",
      "complete setup could not be applied",
      "transient",
    );
  }
}

export function reconcilePendingSetup(
  agentDirectoryInput: unknown,
  repositoryInput: unknown,
): SetupHostResult<"none" | "recovered"> {
  const paths = observedSetupPaths(
    agentDirectoryInput,
    repositoryInput,
    "TIBER_SETUP_APPLY_FAILED",
  );
  if (!paths.ok) return paths;
  const { agentDirectory, repository } = paths.value;
  try {
    const settings = new FileSettingsStore(agentDirectory, repository).load();
    if (!settings.ok) return mapApplyFailure(settings.failure);
    const pending = new FileSetupJournal(
      agentDirectory,
      repository,
      settings.value.projectId,
    ).loadPending();
    if (!pending.ok) return mapApplyFailure(pending.failure);
    if (pending.value.kind === "none") return { ok: true, value: "none" };
    const reconciled = applyObservedSetupPlan(
      agentDirectory,
      repository,
      pending.value.value.expectedCurrent,
      pending.value.value.plan,
      "recovery",
    );
    return reconciled.ok ? { ok: true, value: "recovered" } : reconciled;
  } catch {
    return hostFailure(
      "TIBER_SETUP_APPLY_FAILED",
      "pending setup recovery could not be completed",
      "transient",
    );
  }
}

const setupToolSchema = Type.Object({
  operation: StringEnum(["inspect", "apply", "cancel"] as const),
  plan: Type.Optional(Type.Unknown()),
});
type SetupToolInput = Static<typeof setupToolSchema>;

function renderSetupFailure(failure: {
  readonly code: string;
  readonly message: string;
  readonly causes: readonly FailureCause[];
}): string {
  return [
    `${failure.code}: ${failure.message}`,
    ...failure.causes.map((cause) => `${cause.code}: ${cause.safeSummary}`),
  ].join("\n");
}

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
  const commandSummary =
    plan.commandCatalog.kind === "keep"
      ? "Project command catalog: keep current declaration and local grant state"
      : plan.commandCatalog.kind === "remove"
        ? "Project command catalog: remove project declaration"
        : `Project command catalog: replace with ${String(plan.commandCatalog.catalog.commands.length)} command(s)\n${plan.commandCatalog.catalog.commands
            .map((command) => `- ${command.name} (${command.purpose})`)
            .join("\n")}`;
  const workflowSummary =
    plan.projectWorkflow.kind === "keep"
      ? "Project workflow: keep current declaration"
      : plan.projectWorkflow.kind === "built-in"
        ? "Project workflow: use Tiber built-in workflow"
        : `Project workflow: replace with ${plan.projectWorkflow.workflow.definition.id} (${String(plan.projectWorkflow.workflow.definition.stages.length)} stages, ${plan.projectWorkflow.workflow.digest})`;
  return [
    formatSettingsTable(plan.globalSettings, plan.projectSettings),
    "",
    formatAuthority(
      plan.authority,
      resolveSettings(plan.globalSettings, plan.projectSettings).assuranceLevel
        .value,
    ),
    "",
    commandSummary,
    workflowSummary,
  ].join("\n");
}

function setupCancelled(signal: AbortSignal): boolean {
  return signal.aborted;
}

async function confirmAuthorityLoosening(
  current: SetupPlan,
  proposed: SetupPlan,
  context: ExtensionContext,
  signal: AbortSignal,
): Promise<boolean> {
  for (const phrase of requiredSetupConfirmations(current, proposed)) {
    if (setupCancelled(signal)) return false;
    const entered = await context.ui.input(
      "Confirm weaker Tiber authority",
      `${planSummary(proposed)}\n\nType: ${phrase}`,
    );
    if (setupCancelled(signal) || entered !== phrase) return false;
  }
  return true;
}

export interface SetupToolHost {
  readonly beginConversation: (repository: SetupRepositoryPath) => void;
  readonly endConversation: (
    repository: SetupRepositoryPath,
  ) => ContainmentStatus;
  readonly isConversationActive: (repository: SetupRepositoryPath) => boolean;
}

async function applyRequestedSetup(
  parameters: SetupToolInput,
  context: ExtensionContext,
  agentDirectory: SetupAgentDirectoryPath,
  repository: SetupRepositoryPath,
  host: SetupToolHost,
  signal: AbortSignal,
) {
  if (!context.hasUI || !context.isProjectTrusted()) {
    return setupResponse(
      "TIBER_SETUP_HUMAN_REQUIRED: applying setup requires an interactive trusted project",
      "denied",
    );
  }
  const parsed = parseSetupPlan(parameters.plan);
  if (!parsed.ok) {
    return setupResponse(renderSetupFailure(parsed.failure), "denied");
  }
  const current = currentPlan(agentDirectory, repository);
  if (!current.ok)
    return setupResponse(renderSetupFailure(current.failure), "denied");
  if (
    !(await confirmAuthorityLoosening(
      current.value,
      parsed.value,
      context,
      signal,
    ))
  )
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  if (setupCancelled(signal))
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  const approved = await context.ui.confirm(
    "Apply complete Tiber setup?",
    planSummary(parsed.value),
  );
  if (setupCancelled(signal) || !approved) {
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  }
  if (parsed.value.commandCatalog.kind === "replace") {
    const phrase = `grant commands ${parsed.value.commandCatalog.catalog.digest}`;
    const entered = await context.ui.input(
      "Grant exact project commands",
      `${parsed.value.commandCatalog.catalog.canonicalJson}\n\nType: ${phrase}`,
    );
    if (setupCancelled(signal) || entered !== phrase) {
      return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
    }
  }

  const applied = await withFileMutationQueue(
    join(agentDirectory, "tiber", "setup-application"),
    () =>
      setupCancelled(signal)
        ? Promise.resolve(undefined)
        : Promise.resolve(
            applySetupPlan(
              agentDirectory,
              repository,
              current.value,
              parsed.value,
            ),
          ),
  );
  if (applied === undefined)
    return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
  if (!applied.ok) {
    return setupResponse(renderSetupFailure(applied.failure), "denied");
  }
  const observed = inspectSetup(agentDirectory, repository);
  if (!observed.ok) {
    return setupResponse(renderSetupFailure(observed.failure), "denied");
  }
  const containment = host.endConversation(repository);
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
  let applyAbortController: AbortController | undefined;
  pi.registerCommand("tiber-setup", {
    description: "Configure or reconfigure Tiber in one guided conversation",
    handler: (argumentsText, context) => {
      const paths = observedSetupPaths(
        getAgentDir(),
        context.cwd,
        "TIBER_SETUP_INSPECTION_FAILED",
      );
      if (!paths.ok) {
        context.ui.notify(renderSetupFailure(paths.failure), "error");
        return Promise.resolve();
      }
      const { repository } = paths.value;
      if (argumentsText.trim() === "cancel") {
        if (host.isConversationActive(repository)) {
          applyAbortController?.abort();
          const containment = host.endConversation(repository);
          context.ui.setStatus(
            "tiber",
            containment.state === "verified"
              ? `Tiber: ${containment.level}`
              : "Tiber: containment lockdown",
          );
          context.ui.notify("TIBER_SETUP_CANCELLED", "info");
        } else {
          context.ui.notify("TIBER_SETUP_NOT_ACTIVE", "info");
        }
        return Promise.resolve();
      }
      if (applyInProgress) {
        context.ui.notify(
          "TIBER_SETUP_IN_PROGRESS: wait for or cancel the active setup application",
          "error",
        );
        return Promise.resolve();
      }
      if (!context.hasUI || !context.isProjectTrusted()) {
        context.ui.notify(
          "TIBER_SETUP_HUMAN_REQUIRED: guided setup requires an interactive trusted project",
          "error",
        );
        return Promise.resolve();
      }
      const prompt = setupAgentPrompt();
      if (!prompt.ok) {
        context.ui.notify(renderSetupFailure(prompt.failure), "error");
        return Promise.resolve();
      }
      host.beginConversation(repository);
      try {
        pi.setActiveTools(["read", "tiber_setup"]);
        context.ui.notify("Starting guided Tiber setup", "info");
        pi.sendUserMessage(prompt.value);
      } catch {
        host.endConversation(repository);
        context.ui.notify(
          "TIBER_SETUP_INSPECTION_FAILED: guided setup could not start",
          "error",
        );
      }
      return Promise.resolve();
    },
  });

  pi.registerTool({
    name: "tiber_setup",
    label: "Tiber guided setup",
    description:
      "Inspect the complete closed Tiber configuration catalog, apply one complete user-approved setup plan, or end the active setup conversation. Inspection exposes choices, effects, current layered values, declarations, and external blockers without secret values. Apply requires project trust and independent interactive host confirmation; command grants and assurance loosening require exact human phrases.",
    promptSnippet:
      "Inspect and apply complete conversational Tiber setup without manual file editing",
    promptGuidelines: [
      "Use tiber_setup only during an explicit guided Tiber setup conversation. Inspect before proposing changes, ask about every catalog area, never request secret values, call apply only after the user approves a complete preview, and call cancel if the user stops setup.",
    ],
    parameters: setupToolSchema,
    async execute(
      _toolCallId,
      parameters: SetupToolInput,
      signal,
      _onUpdate,
      context,
    ) {
      const paths = observedSetupPaths(
        getAgentDir(),
        context.cwd,
        "TIBER_SETUP_INSPECTION_FAILED",
      );
      if (!paths.ok)
        return setupResponse(renderSetupFailure(paths.failure), "denied");
      const { agentDirectory, repository } = paths.value;
      if (!host.isConversationActive(repository)) {
        return setupResponse(
          "TIBER_SETUP_NOT_ACTIVE: invoke /tiber-setup in this repository first",
          "denied",
        );
      }
      if (parameters.operation === "inspect") {
        const inspected = inspectSetup(agentDirectory, repository);
        return inspected.ok
          ? setupResponse(JSON.stringify(inspected.value, null, 2), "inspected")
          : setupResponse(renderSetupFailure(inspected.failure), "denied");
      }
      if (parameters.operation === "cancel") {
        applyAbortController?.abort();
        host.endConversation(repository);
        return setupResponse("TIBER_SETUP_CANCELLED", "cancelled");
      }
      if (applyInProgress) {
        return setupResponse(
          "TIBER_SETUP_IN_PROGRESS: wait for the active setup confirmation",
          "denied",
        );
      }
      applyInProgress = true;
      const localAbortController = new AbortController();
      applyAbortController = localAbortController;
      const executionSignal =
        signal === undefined
          ? localAbortController.signal
          : AbortSignal.any([signal, localAbortController.signal]);
      try {
        return await applyRequestedSetup(
          parameters,
          context,
          agentDirectory,
          repository,
          host,
          executionSignal,
        );
      } catch {
        return setupCancelled(executionSignal)
          ? setupResponse("TIBER_SETUP_CANCELLED", "cancelled")
          : setupResponse(
              "TIBER_SETUP_APPLY_FAILED: setup execution could not be completed",
              "denied",
            );
      } finally {
        if (applyAbortController === localAbortController)
          applyAbortController = undefined;
        applyInProgress = false;
      }
    },
  });
}
