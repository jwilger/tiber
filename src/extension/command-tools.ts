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
import { virtualizeCommandOutput } from "../core/artifacts/output-virtualization.js";
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
              entry.claimId === task?.claim?.claimId,
          )
        : undefined;
      const decision = decideCommandExecution(catalog.value, parameters.name, {
        activeClaim:
          board.mode === "writable" &&
          task?.state === "In Progress" &&
          worktree !== undefined,
        grantedCatalogDigest: authority.readGrant(),
      });
      if (!decision.ok || worktree === undefined || task?.claim === undefined)
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
        { taskId: task.id, claimId: task.claim.claimId },
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
      const result = virtualizeCommandOutput(
        run.output,
        decision.command.maxOutputBytes,
      );
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
      artifacts.reap(Date.now());
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
      const result = new FileArtifactStore(getAgentDir()).range(
        parameters.digest,
        parameters.offset,
        parameters.limit,
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
      const result = new FileArtifactStore(getAgentDir()).search(
        parameters.digest,
        parameters.query,
        parameters.maxMatches ?? 20,
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
