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

import { FilePermissionSettingsStore } from "../adapters/permissions/file-permission-settings-store.js";
import { FilePermissionStore } from "../adapters/permissions/file-permission-store.js";
import { FileRunJournal } from "../adapters/runs/file-run-journal.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import { GitTaskRemote } from "../adapters/tasks/git-task-remote.js";
import { GitOwnedWorktrees } from "../adapters/worktrees/git-owned-worktrees.js";
import {
  parsePermissionDecisionAt,
  permissionScope,
} from "../core/permissions/permission-values.js";
import { none, some } from "../core/types/option.js";
import type { TestMappingPath } from "../core/tasks/task-values.js";
import type { OwnedWorktreePath } from "../core/worktrees/worktree-values.js";
import { authorizeWorkflowMutation } from "../core/tools/red-mutation-policy.js";
import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../core/failures/tiber-failure.js";
import {
  authorizeMutation,
  authorizeReadPath,
} from "../core/tools/tool-policy.js";
import { authorizeRequestedEffect } from "./permission-authorization.js";
import {
  parseCanonicalReadTarget,
  parseClaimedWorkspaceRoot,
  parseRequestedWorkspacePath,
  type CanonicalReadTarget,
} from "../core/tools/tool-values.js";

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
  purpose: Type.Union([Type.Literal("production"), Type.Literal("refactor")]),
});
const writeParameters = Type.Object({
  path: Type.String(),
  content: Type.String(),
  purpose: Type.Union([Type.Literal("production"), Type.Literal("refactor")]),
});

interface ActiveMutationAuthority {
  readonly root: OwnedWorktreePath;
  readonly claimStatus: "published";
  readonly redStatus: "accepted" | "required";
  readonly refactorStatus: "allowed" | "revoked";
  readonly testMappings: readonly TestMappingPath[];
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
        candidate.claim.kind === "some" &&
        candidate.claim.value.claimId === entry.claimId,
    );
    if (task?.specification.kind !== "some") return [];
    const runResult = new FileRunJournal(getAgentDir()).read(task.id);
    if (
      !runResult.ok ||
      runResult.value.kind === "none" ||
      runResult.value.value.claimId !== entry.claimId
    )
      return [];
    const run = runResult.value.value;
    const authority: ActiveMutationAuthority = {
      root: entry.path,
      claimStatus: "published",
      redStatus:
        run.state === "red-accepted" ||
        run.state === "green-review-clean" ||
        run.state === "green-rework-required" ||
        run.state === "red-reinstated"
          ? "accepted"
          : "required",
      refactorStatus:
        run.state === "green-review-clean" ? "allowed" : "revoked",
      testMappings: task.specification.value.testMappings,
    };
    return [authority];
  });
  return matches.length === 1 ? matches[0] : undefined;
}

type GovernedPathFailure = TiberFailure<
  "TIBER_MUTATION_PATH_INVALID",
  { readonly domain: "governed-path" },
  "corrected-input" | "state-change" | "retry-operation"
>;

async function canonicalMutationTarget(
  rootValue: string,
  pathValue: string,
): Promise<TiberResult<CanonicalReadTarget, GovernedPathFailure>> {
  const root = parseClaimedWorkspaceRoot(rootValue);
  const path = parseRequestedWorkspacePath(pathValue);
  if (!root.ok || !path.ok)
    return {
      ok: false,
      failure: operationalFailure(
        "TIBER_MUTATION_PATH_INVALID",
        "governed-path",
        "mutation path values are invalid",
        "retry-after-input",
      ),
    };
  const lexical = resolve(root.value, path.value);
  const parent = await realpath(dirname(lexical));
  const canonical = parseCanonicalReadTarget(
    resolve(parent, basename(lexical)),
  );
  const observed = parseCanonicalReadTarget(
    existsSync(lexical)
      ? await realpath(lexical)
      : resolve(parent, basename(lexical)),
  );
  if (!canonical.ok || !observed.ok)
    return {
      ok: false,
      failure: operationalFailure(
        "TIBER_MUTATION_PATH_INVALID",
        "governed-path",
        "canonical mutation target is invalid",
        "retry-after-input",
      ),
    };
  const check = authorizeReadPath(root.value, path.value, observed.value);
  return check.allowed
    ? { ok: true, value: canonical.value }
    : {
        ok: false,
        failure: operationalFailure(
          "TIBER_MUTATION_PATH_INVALID",
          "governed-path",
          check.detail,
          "retry-after-input",
        ),
      };
}

function deniedMutation() {
  const denial = authorizeMutation("absent");
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
        const root = parseClaimedWorkspaceRoot(
          realpathSync(authority?.root ?? context.cwd),
        );
        const requested = parseRequestedWorkspacePath(parameters.path);
        if (!root.ok || !requested.ok) throw new Error("invalid read path");
        const lexical = resolve(root.value, requested.value);
        const canonical = parseCanonicalReadTarget(realpathSync(lexical));
        if (!canonical.ok) throw new Error("invalid canonical read path");
        const decision = authorizeReadPath(
          root.value,
          requested.value,
          canonical.value,
        );
        if (!decision.allowed)
          return {
            content: [
              { type: "text", text: `${decision.code}: ${decision.detail}` },
            ],
            details: { allowed: false, code: decision.code },
          };
        const text = await readFile(canonical.value, "utf8");
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
      "Run an exact shell command only after explicit human permission and eligible workflow authority",
    parameters: bashParameters,
    async execute(_id, parameters, signal, _update, context) {
      const authority = activeAuthority(context);
      const settings = new FileSettingsStore(getAgentDir(), context.cwd).load();
      if (!settings.ok)
        return {
          content: [{ type: "text", text: settings.failure.code }],
          details: { allowed: false, code: settings.failure.code },
          isError: true,
        };
      const permissionSettings = new FilePermissionSettingsStore(
        getAgentDir(),
        settings.value.projectId,
      ).load();
      const decidedAt = parsePermissionDecisionAt(new Date().toISOString());
      if (!permissionSettings.ok || !decidedAt.ok)
        return {
          content: [{ type: "text", text: "TIBER_PERMISSION_STATE_INVALID" }],
          details: {
            allowed: false,
            code: "TIBER_PERMISSION_STATE_INVALID",
          },
          isError: true,
        };
      const choices = {
        "deny-once": "Deny this time",
        "deny-always": "Always deny this action",
        "allow-once": "Allow this time",
        "allow-always": "Always allow this action",
      } as const;
      const authorization = await authorizeRequestedEffect(
        {
          autonomy: permissionSettings.value.autonomy,
          role: authority === undefined ? "coordinator" : "implementation",
          effect: "arbitrary-shell",
          risk: "unfamiliar",
          boundary: "repository",
          workflow: authority === undefined ? "denied" : "authorized",
          interactive: context.hasUI,
          persistable: false,
        },
        permissionScope({
          role: "implementation",
          effect: "arbitrary-shell",
          executable: "bash",
          argv: ["-lc", parameters.command],
          purpose: "exact shell command",
          cwd: "task-worktree",
          environment: {},
        }),
        `Tiber requested this exact shell command:\n\n${JSON.stringify(parameters.command)}`,
        new FilePermissionStore(getAgentDir(), settings.value.projectId),
        {
          async choose(description, available) {
            const labels = available.map((choice) => choices[choice]);
            const selected = await context.ui.select(description, labels);
            const selectedIndex = labels.findIndex(
              (label) => label === selected,
            );
            const selectedChoice = available[selectedIndex];
            return selectedChoice === undefined ? none : some(selectedChoice);
          },
        },
        decidedAt.value,
      );
      if (authorization.status === "denied")
        return {
          content: [{ type: "text", text: authorization.code }],
          details: { allowed: false, code: authorization.code },
          isError: true,
        };
      if (authority === undefined) return deniedMutation();
      const result = await pi.exec("bash", ["-lc", parameters.command], {
        cwd: authority.root,
        ...(signal === undefined ? {} : { signal }),
        timeout: 60_000,
      });
      const output = `${result.stdout}${result.stderr}`;
      const bounded =
        Buffer.byteLength(output) <= 50 * 1024
          ? output
          : `${Buffer.from(output)
              .subarray(0, 50 * 1024)
              .toString("utf8")}\n[Tiber output truncated]`;
      return {
        content: [{ type: "text", text: bounded }],
        details: { allowed: true, exitCode: result.code },
        isError: result.code !== 0,
      };
    },
  });

  pi.registerTool({
    name: "edit",
    label: "edit (Tiber governed)",
    description: "Apply an exact edit under claim and RED workflow authority",
    parameters: editParameters,
    async execute(_id, parameters, _signal, _update, context) {
      const authority = activeAuthority(context);
      if (authority === undefined) return deniedMutation();
      const decision = authorizeWorkflowMutation(
        parameters.path,
        parameters.purpose,
        authority,
      );
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
        if (!target.ok)
          return {
            content: [{ type: "text", text: target.failure.code }],
            details: { allowed: false, code: target.failure.code },
            isError: true,
          };
        const targetPath = target.value;
        const before = await readFile(targetPath, "utf8");
        const first = before.indexOf(parameters.oldText);
        if (first < 0 || before.includes(parameters.oldText, first + 1))
          throw new Error("oldText must match exactly once");
        const after = `${before.slice(0, first)}${parameters.newText}${before.slice(first + parameters.oldText.length)}`;
        const temporary = `${targetPath}.${randomUUID()}.tmp`;
        await writeFile(temporary, after, {
          encoding: "utf8",
          mode: 0o600,
          flag: "wx",
        });
        try {
          await rename(temporary, targetPath);
        } catch (error) {
          await rm(temporary, { force: true });
          throw error;
        }
        return {
          content: [{ type: "text", text: `Updated ${parameters.path}` }],
          details: { allowed: true, code: decision.code, path: targetPath },
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
      const decision = authorizeWorkflowMutation(
        parameters.path,
        parameters.purpose,
        authority,
      );
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
        if (!target.ok)
          return {
            content: [{ type: "text", text: target.failure.code }],
            details: { allowed: false, code: target.failure.code },
            isError: true,
          };
        const targetPath = target.value;
        const temporary = `${targetPath}.${randomUUID()}.tmp`;
        await writeFile(temporary, parameters.content, {
          encoding: "utf8",
          mode: 0o600,
          flag: "wx",
        });
        try {
          await rename(temporary, targetPath);
        } catch (error) {
          await rm(temporary, { force: true });
          throw error;
        }
        return {
          content: [{ type: "text", text: `Wrote ${parameters.path}` }],
          details: { allowed: true, code: decision.code, path: targetPath },
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
