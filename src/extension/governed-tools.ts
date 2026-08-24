import { randomUUID } from "node:crypto";
import {
  mkdir,
  readFile,
  realpath,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { existsSync, realpathSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";

import {
  getAgentDir,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import { authorizeWorkflowMutation } from "../core/tools/red-mutation-policy.js";
import {
  authorizeMutation,
  authorizeReadPath,
} from "../core/tools/tool-policy.js";

const readParameters = Type.Object({
  path: Type.String({ description: "Claimed-worktree-relative file path" }),
  offset: Type.Optional(Type.Integer({ minimum: 1 })),
  limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 2000 })),
});
const bashParameters = Type.Object({
  command: Type.String({ description: "Always denied; use tiber_command" }),
});
const editParameters = Type.Object({
  path: Type.String(),
  oldText: Type.String(),
  newText: Type.String(),
});
const writeParameters = Type.Object({
  path: Type.String(),
  content: Type.String(),
});

interface ActiveMutationAuthority {
  readonly root: string;
  readonly activeClaim: boolean;
  readonly redAccepted: boolean;
  readonly testMappings: readonly string[];
}

function activeAuthority(
  context: ExtensionContext,
): ActiveMutationAuthority | undefined {
  const board = new GitTaskRemote(context.cwd).read();
  if (board.mode !== "writable") return undefined;
  const worktrees = new GitOwnedWorktrees(context.cwd, getAgentDir()).read();
  if (!worktrees.ok) return undefined;
  const matches = worktrees.value.worktrees.flatMap((entry) => {
    const task = board.tasks.find(
      (candidate) =>
        candidate.id === entry.taskId &&
        candidate.state === "In Progress" &&
        candidate.claim?.claimId === entry.claimId,
    );
    if (task?.specification === undefined) return [];
    const run = new FileRunJournal(getAgentDir()).read(task.id);
    if (run?.claimId !== entry.claimId) return [];
    return [
      {
        root: entry.path,
        activeClaim: true,
        redAccepted: run.state === "red-accepted",
        testMappings: task.specification.testMappings,
      },
    ];
  });
  return matches.length === 1 ? matches[0] : undefined;
}

async function canonicalMutationTarget(
  root: string,
  path: string,
): Promise<string> {
  const lexical = resolve(root, path);
  const parent = await realpath(dirname(lexical));
  const canonical = resolve(parent, basename(lexical));
  const check = authorizeReadPath(
    root,
    path,
    existsSync(lexical) ? await realpath(lexical) : canonical,
  );
  if (!check.allowed) throw new Error(check.code);
  return canonical;
}

function deniedMutation() {
  const denial = authorizeMutation(false);
  return {
    content: [
      { type: "text" as const, text: `${denial.code}: ${denial.detail}` },
    ],
    details: { allowed: false, code: denial.code },
  };
}

export function registerGovernedTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "read",
    label: "read (Tiber governed)",
    description: "Read a bounded regular file from the active claimed worktree",
    parameters: readParameters,
    async execute(_id, parameters, _signal, _update, context) {
      try {
        const authority = activeAuthority(context);
        const root = realpathSync(authority?.root ?? context.cwd);
        const lexical = resolve(root, parameters.path);
        const canonical = realpathSync(lexical);
        const decision = authorizeReadPath(root, parameters.path, canonical);
        if (!decision.allowed)
          return {
            content: [
              { type: "text", text: `${decision.code}: ${decision.detail}` },
            ],
            details: { allowed: false, code: decision.code },
          };
        const text = await readFile(canonical, "utf8");
        const lines = text.split("\n");
        const start = (parameters.offset ?? 1) - 1;
        const selected = lines
          .slice(start, start + (parameters.limit ?? 2000))
          .join("\n");
        const bounded =
          Buffer.byteLength(selected) <= 50 * 1024
            ? selected
            : `${Buffer.from(selected)
                .subarray(0, 50 * 1024)
                .toString("utf8")}\n[Tiber preview truncated]`;
        return {
          content: [{ type: "text", text: bounded }],
          details: { allowed: true, path: canonical },
        };
      } catch {
        return {
          content: [
            { type: "text", text: "TIBER_READ_FAILED: file is unavailable" },
          ],
          details: { allowed: false, code: "TIBER_READ_FAILED" },
        };
      }
    },
  });

  pi.registerTool({
    name: "bash",
    label: "bash (Tiber governed)",
    description:
      "Arbitrary shell execution is denied; use a granted tiber_command",
    parameters: bashParameters,
    execute: () => Promise.resolve(deniedMutation()),
  });

  pi.registerTool({
    name: "edit",
    label: "edit (Tiber governed)",
    description: "Apply an exact edit under claim and RED workflow authority",
    parameters: editParameters,
    async execute(_id, parameters, _signal, _update, context) {
      const authority = activeAuthority(context);
      if (authority === undefined) return deniedMutation();
      const decision = authorizeWorkflowMutation(parameters.path, authority);
      if (!decision.allowed)
        return {
          content: [
            { type: "text", text: `${decision.code}: ${decision.detail}` },
          ],
          details: { allowed: false, code: decision.code },
        };
      try {
        const target = await canonicalMutationTarget(
          authority.root,
          parameters.path,
        );
        const before = await readFile(target, "utf8");
        const first = before.indexOf(parameters.oldText);
        if (first < 0 || before.includes(parameters.oldText, first + 1))
          throw new Error("oldText must match exactly once");
        const after = `${before.slice(0, first)}${parameters.newText}${before.slice(first + parameters.oldText.length)}`;
        const temporary = `${target}.${randomUUID()}.tmp`;
        await writeFile(temporary, after, {
          encoding: "utf8",
          mode: 0o600,
          flag: "wx",
        });
        try {
          await rename(temporary, target);
        } catch (error) {
          await rm(temporary, { force: true });
          throw error;
        }
        return {
          content: [{ type: "text", text: `Updated ${parameters.path}` }],
          details: { allowed: true, code: decision.code, path: target },
        };
      } catch (error) {
        return {
          content: [
            {
              type: "text",
              text: `TIBER_EDIT_FAILED: ${error instanceof Error ? error.message : "edit failed"}`,
            },
          ],
          details: { allowed: false, code: "TIBER_EDIT_FAILED" },
          isError: true,
        };
      }
    },
  });

  pi.registerTool({
    name: "write",
    label: "write (Tiber governed)",
    description: "Write a file under claim and RED workflow authority",
    parameters: writeParameters,
    async execute(_id, parameters, _signal, _update, context) {
      const authority = activeAuthority(context);
      if (authority === undefined) return deniedMutation();
      const decision = authorizeWorkflowMutation(parameters.path, authority);
      if (!decision.allowed)
        return {
          content: [
            { type: "text", text: `${decision.code}: ${decision.detail}` },
          ],
          details: { allowed: false, code: decision.code },
        };
      try {
        const lexical = resolve(authority.root, parameters.path);
        await mkdir(dirname(lexical), { recursive: true, mode: 0o700 });
        const target = await canonicalMutationTarget(
          authority.root,
          parameters.path,
        );
        const temporary = `${target}.${randomUUID()}.tmp`;
        await writeFile(temporary, parameters.content, {
          encoding: "utf8",
          mode: 0o600,
          flag: "wx",
        });
        try {
          await rename(temporary, target);
        } catch (error) {
          await rm(temporary, { force: true });
          throw error;
        }
        return {
          content: [{ type: "text", text: `Wrote ${parameters.path}` }],
          details: { allowed: true, code: decision.code, path: target },
        };
      } catch (error) {
        return {
          content: [
            {
              type: "text",
              text: `TIBER_WRITE_FAILED: ${error instanceof Error ? error.message : "write failed"}`,
            },
          ],
          details: { allowed: false, code: "TIBER_WRITE_FAILED" },
          isError: true,
        };
      }
    },
  });
}
