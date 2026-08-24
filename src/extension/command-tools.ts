import {
  getAgentDir,
  type ExtensionAPI,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { FileArtifactStore } from "../adapters/artifacts/file-artifact-store.js";
import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../adapters/commands/structured-command-runner.js";
import { FileProcessGroupRegistry } from "../adapters/processes/file-process-group-registry.js";
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
import { decideCommandExecution } from "../core/commands/structured-command.js";

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
