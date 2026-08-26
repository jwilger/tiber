import { accessSync, constants, statSync } from "node:fs";
import { delimiter, join } from "node:path";

import { StringEnum } from "@earendil-works/pi-ai";
import {
  getAgentDir,
  type ExtensionAPI,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { FileArtifactStore } from "../adapters/artifacts/file-artifact-store.js";
import { FilePermissionSettingsStore } from "../adapters/permissions/file-permission-settings-store.js";
import { FilePermissionStore } from "../adapters/permissions/file-permission-store.js";
import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../adapters/commands/structured-command-runner.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import {
  parseArtifactDigest,
  parseArtifactRangeLimit,
  parseArtifactRangeOffset,
  parseArtifactReapAtMilliseconds,
  parseArtifactSearchMaximumMatches,
  parseArtifactSearchQuery,
  parseInlineOutputMaximumBytes,
} from "../core/artifacts/artifact-values.js";
import { virtualizeCommandOutput } from "../core/artifacts/output-virtualization.js";
import { parseCommandName } from "../core/commands/command-values.js";
import {
  compileCommandCatalog,
  decideCommandExecution,
} from "../core/commands/structured-command.js";
import {
  parsePermissionDecisionAt,
  permissionScope,
} from "../core/permissions/permission-values.js";
import { none, some } from "../core/types/option.js";
import { authorizeRequestedEffect } from "./permission-authorization.js";

function resolveExecutable(name: string): string | undefined {
  if (!/^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/u.test(name)) return undefined;
  for (const directory of (process.env.PATH ?? "").split(delimiter)) {
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

function isInterpreter(name: string): boolean {
  return new Set([
    "bash",
    "cmd",
    "dash",
    "deno",
    "fish",
    "node",
    "perl",
    "powershell",
    "pwsh",
    "python",
    "python3",
    "ruby",
    "sh",
    "zsh",
  ]).has(name);
}

function text(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

export function registerCommandTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "tiber_command",
    label: "Tiber Command",
    description:
      "Run one locally granted named command in the exact claimed task worktree without a shell.",
    parameters: Type.Object(
      {
        taskId: Type.String({ description: "Claimed task UUID" }),
        name: Type.String({ description: "Name from .tiber/commands.json" }),
      },
      { additionalProperties: false },
    ),
    async execute(_id, parameters, signal, _update, context) {
      const authority = new FileCommandAuthority(context.cwd);
      const catalog = authority.loadCatalog();
      if (!catalog.ok)
        return {
          content: [
            {
              type: "text",
              text: `${catalog.failure.code}: ${catalog.failure.message}`,
            },
          ],
          details: {},
          isError: true,
        };
      const commandName = parseCommandName(parameters.name);
      if (!commandName.ok)
        return {
          content: [{ type: "text", text: commandName.failure.code }],
          details: {},
          isError: true,
        };
      const board = new GitTaskRemote(context.cwd).read();
      const task = board.tasks.find(
        (candidate) => candidate.id === parameters.taskId,
      );
      const worktrees = new GitOwnedWorktrees(
        context.cwd,
        getAgentDir(),
      ).read();
      const worktree = worktrees.ok
        ? worktrees.value.worktrees.find(
            (entry) =>
              entry.taskId === parameters.taskId &&
              entry.claimId ===
                (task?.claim.kind === "some"
                  ? task.claim.value.claimId
                  : undefined),
          )
        : undefined;
      const grant = authority.readGrant();
      if (!grant.ok)
        return {
          content: [{ type: "text", text: grant.failure.code }],
          details: {},
          isError: true,
        };
      const decision = decideCommandExecution(
        catalog.value,
        commandName.value,
        {
          claimStatus:
            board.mode === "writable" &&
            task?.state === "In Progress" &&
            worktree !== undefined
              ? "published"
              : "absent",
          grantedCatalogDigest: grant.value,
        },
      );
      if (!decision.ok || worktree === undefined || task?.claim.kind !== "some")
        return {
          content: [
            {
              type: "text",
              text: decision.ok ? "TIBER_COMMAND_DENIED" : decision.code,
            },
          ],
          details: {},
          isError: true,
        };
      const settings = new FileSettingsStore(getAgentDir(), context.cwd).load();
      const decidedAt = parsePermissionDecisionAt(new Date().toISOString());
      if (!settings.ok || !decidedAt.ok)
        return {
          content: [{ type: "text", text: "TIBER_PERMISSION_STATE_INVALID" }],
          details: {},
          isError: true,
        };
      const permissionSettings = new FilePermissionSettingsStore(
        getAgentDir(),
        settings.value.projectId,
      ).load();
      if (!permissionSettings.ok)
        return {
          content: [{ type: "text", text: permissionSettings.failure.code }],
          details: {},
          isError: true,
        };
      const labels = {
        "deny-once": "Deny this time",
        "deny-always": "Always deny this action",
        "allow-once": "Allow this time",
        "allow-always": "Always allow this action",
      } as const;
      const permission = await authorizeRequestedEffect(
        {
          autonomy: permissionSettings.value.autonomy,
          role: "implementation",
          effect: "process",
          risk: "routine",
          boundary: "repository",
          workflow: "authorized",
          interactive: context.hasUI,
          persistable: true,
        },
        permissionScope({
          role: "implementation",
          effect: "process",
          executable: decision.command.executable,
          purpose: decision.command.purpose,
        }),
        `Run ${decision.command.name} (${decision.command.purpose}) in the task worktree`,
        new FilePermissionStore(getAgentDir(), settings.value.projectId),
        {
          async choose(description, choices) {
            const availableLabels = choices.map((choice) => labels[choice]);
            const selected = await context.ui.select(
              description,
              availableLabels,
            );
            const selectedIndex = availableLabels.findIndex(
              (label) => label === selected,
            );
            const selectedChoice = choices[selectedIndex];
            return selectedChoice === undefined ? none : some(selectedChoice);
          },
        },
        decidedAt.value,
      );
      if (permission.status === "denied")
        return {
          content: [{ type: "text", text: permission.code }],
          details: {},
          isError: true,
        };
      const runner = new StructuredCommandRunner(
        new FileProcessGroupRegistry(getAgentDir()),
      );
      const run = await runner.run(
        decision.command,
        worktree.path,
        { taskId: task.id, claimId: task.claim.value.claimId },
        signal,
      );
      if (!run.ok)
        return {
          content: [
            {
              type: "text",
              text: `${run.failure.code}: ${run.failure.message}`,
            },
          ],
          details: {},
          isError: true,
        };
      const inlineLimit = parseInlineOutputMaximumBytes(
        decision.command.maxOutputBytes,
      );
      if (!inlineLimit.ok)
        return {
          content: [{ type: "text", text: inlineLimit.failure.code }],
          details: {},
          isError: true,
        };
      const result = virtualizeCommandOutput(run.output, inlineLimit.value);
      const artifacts = new FileArtifactStore(getAgentDir());
      const stored = artifacts.put(result);
      if (!stored.ok)
        return {
          content: [
            {
              type: "text",
              text: `${stored.failure.code}: ${stored.failure.message}`,
            },
          ],
          details: {},
          isError: true,
        };
      const reapAt = parseArtifactReapAtMilliseconds(Date.now());
      if (reapAt.ok) artifacts.reap(reapAt.value);
      if (result.kind === "inline")
        return {
          content: [{ type: "text", text: text(result.output) }],
          details: result,
        };
      return {
        content: [
          {
            type: "text",
            text: text({
              artifact: result.digest,
              byteLength: result.byteLength,
              preview: result.preview,
            }),
          },
        ],
        details: {
          artifact: result.digest,
          byteLength: result.byteLength,
          preview: result.preview,
        },
      };
    },
  });

  pi.registerTool({
    name: "tiber_process",
    label: "Tiber Process",
    description:
      "Request one shell-free executable and argv operation in an active task worktree. Tiber resolves the executable, enforces workflow and role limits, and asks on first use when required.",
    parameters: Type.Object(
      {
        taskId: Type.String({ description: "Claimed task UUID" }),
        executable: Type.String({
          minLength: 1,
          maxLength: 128,
          pattern: "^[A-Za-z0-9][A-Za-z0-9._+-]*$",
        }),
        argv: Type.Array(Type.String({ maxLength: 4_096 }), {
          maxItems: 64,
        }),
        purpose: StringEnum(["test", "verification"] as const),
      },
      { additionalProperties: false },
    ),
    async execute(_id, parameters, signal, _update, context) {
      const executable = resolveExecutable(parameters.executable);
      const board = new GitTaskRemote(context.cwd).read();
      const task = board.tasks.find(
        (candidate) => candidate.id === parameters.taskId,
      );
      const worktrees = new GitOwnedWorktrees(
        context.cwd,
        getAgentDir(),
      ).read();
      const worktree = worktrees.ok
        ? worktrees.value.worktrees.find(
            (entry) =>
              entry.taskId === parameters.taskId &&
              entry.claimId ===
                (task?.claim.kind === "some"
                  ? task.claim.value.claimId
                  : undefined),
          )
        : undefined;
      if (
        executable === undefined ||
        board.mode !== "writable" ||
        task?.state !== "In Progress" ||
        task.claim.kind !== "some" ||
        worktree === undefined
      )
        return {
          content: [{ type: "text", text: "TIBER_PROCESS_AUTHORITY_DENIED" }],
          details: {},
          isError: true,
        };
      const compiled = compileCommandCatalog({
        schemaVersion: 1,
        commands: [
          {
            name: "requested-process",
            executable,
            purpose: parameters.purpose,
            argv: parameters.argv,
            cwd: "worktree",
            environment: {},
            timeoutMs: 900_000,
            maxOutputBytes: 1_048_576,
          },
        ],
      });
      if (!compiled.ok)
        return {
          content: [{ type: "text", text: compiled.failure.code }],
          details: {},
          isError: true,
        };
      const command = compiled.value.commands[0];
      const settings = new FileSettingsStore(getAgentDir(), context.cwd).load();
      const decidedAt = parsePermissionDecisionAt(new Date().toISOString());
      if (command === undefined || !settings.ok || !decidedAt.ok)
        return {
          content: [{ type: "text", text: "TIBER_PERMISSION_STATE_INVALID" }],
          details: {},
          isError: true,
        };
      const permissionSettings = new FilePermissionSettingsStore(
        getAgentDir(),
        settings.value.projectId,
      ).load();
      if (!permissionSettings.ok)
        return {
          content: [{ type: "text", text: permissionSettings.failure.code }],
          details: {},
          isError: true,
        };
      const interpreter = isInterpreter(parameters.executable);
      const labels = {
        "deny-once": "Deny this time",
        "deny-always": "Always deny this action",
        "allow-once": "Allow this time",
        "allow-always": "Always allow this action",
      } as const;
      const permission = await authorizeRequestedEffect(
        {
          autonomy: permissionSettings.value.autonomy,
          role: "implementation",
          effect: interpreter ? "arbitrary-shell" : "process",
          risk: "unfamiliar",
          boundary: "repository",
          workflow: "authorized",
          interactive: context.hasUI,
          persistable: false,
        },
        permissionScope({
          role: "implementation",
          effect: interpreter ? "arbitrary-shell" : "process",
          executable: parameters.executable,
          purpose: JSON.stringify({
            argv: parameters.argv,
            purpose: parameters.purpose,
          }),
        }),
        `Run this exact shell-free process in the task worktree:\n${JSON.stringify({ executable: parameters.executable, argv: parameters.argv })}`,
        new FilePermissionStore(getAgentDir(), settings.value.projectId),
        {
          async choose(description, choices) {
            const availableLabels = choices.map((choice) => labels[choice]);
            const selected = await context.ui.select(
              description,
              availableLabels,
            );
            const selectedIndex = availableLabels.findIndex(
              (label) => label === selected,
            );
            const selectedChoice = choices[selectedIndex];
            return selectedChoice === undefined ? none : some(selectedChoice);
          },
        },
        decidedAt.value,
      );
      if (permission.status === "denied")
        return {
          content: [{ type: "text", text: permission.code }],
          details: {},
          isError: true,
        };
      const run = await new StructuredCommandRunner(
        new FileProcessGroupRegistry(getAgentDir()),
      ).run(
        command,
        worktree.path,
        { taskId: task.id, claimId: task.claim.value.claimId },
        signal,
      );
      if (!run.ok)
        return {
          content: [
            {
              type: "text",
              text: `${run.failure.code}: ${run.failure.message}`,
            },
          ],
          details: {},
          isError: true,
        };
      const inlineLimit = parseInlineOutputMaximumBytes(command.maxOutputBytes);
      if (!inlineLimit.ok)
        return {
          content: [{ type: "text", text: inlineLimit.failure.code }],
          details: {},
          isError: true,
        };
      const result = virtualizeCommandOutput(run.output, inlineLimit.value);
      const stored = new FileArtifactStore(getAgentDir()).put(result);
      return stored.ok
        ? {
            content: [
              {
                type: "text",
                text:
                  result.kind === "inline"
                    ? text(result.output)
                    : text({
                        artifact: result.digest,
                        byteLength: result.byteLength,
                        preview: result.preview,
                      }),
              },
            ],
            details: result,
          }
        : {
            content: [{ type: "text", text: stored.failure.code }],
            details: {},
            isError: true,
          };
    },
  });

  pi.registerTool({
    name: "tiber_artifact_range",
    label: "Tiber Artifact Range",
    description:
      "Read one bounded byte range from a verified command artifact.",
    parameters: Type.Object(
      {
        digest: Type.String(),
        offset: Type.Integer({ minimum: 0 }),
        limit: Type.Integer({ minimum: 1, maximum: 65_536 }),
      },
      { additionalProperties: false },
    ),
    execute(_id, parameters) {
      const digest = parseArtifactDigest(parameters.digest);
      const offset = parseArtifactRangeOffset(parameters.offset);
      const limit = parseArtifactRangeLimit(parameters.limit);
      if (!digest.ok || !offset.ok || !limit.ok)
        return Promise.resolve({
          content: [
            { type: "text" as const, text: "TIBER_ARTIFACT_RANGE_INVALID" },
          ],
          details: {},
          isError: true,
        });
      const result = new FileArtifactStore(getAgentDir()).range(
        digest.value,
        offset.value,
        limit.value,
      );
      return Promise.resolve(
        result.ok
          ? {
              content: [{ type: "text" as const, text: result.value.text }],
              details: result.value,
            }
          : {
              content: [
                {
                  type: "text" as const,
                  text: `${result.failure.code}: ${result.failure.message}`,
                },
              ],
              details: {},
              isError: true,
            },
      );
    },
  });

  pi.registerTool({
    name: "tiber_artifact_search",
    label: "Tiber Artifact Search",
    description:
      "Search a verified command artifact with bounded literal matches.",
    parameters: Type.Object(
      {
        digest: Type.String(),
        query: Type.String({ minLength: 1, maxLength: 256 }),
        maxMatches: Type.Optional(Type.Integer({ minimum: 1, maximum: 100 })),
      },
      { additionalProperties: false },
    ),
    execute(_id, parameters) {
      const digest = parseArtifactDigest(parameters.digest);
      const query = parseArtifactSearchQuery(parameters.query);
      const maximumMatches = parseArtifactSearchMaximumMatches(
        parameters.maxMatches ?? 20,
      );
      if (!digest.ok || !query.ok || !maximumMatches.ok)
        return Promise.resolve({
          content: [
            { type: "text" as const, text: "TIBER_ARTIFACT_SEARCH_INVALID" },
          ],
          details: {},
          isError: true,
        });
      const result = new FileArtifactStore(getAgentDir()).search(
        digest.value,
        query.value,
        maximumMatches.value,
      );
      return Promise.resolve(
        result.ok
          ? {
              content: [{ type: "text" as const, text: text(result.value) }],
              details: result.value,
            }
          : {
              content: [
                {
                  type: "text" as const,
                  text: `${result.failure.code}: ${result.failure.message}`,
                },
              ],
              details: {},
              isError: true,
            },
      );
    },
  });
}
