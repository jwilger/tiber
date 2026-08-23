import { readFile } from "node:fs/promises";
import { realpathSync } from "node:fs";
import { resolve } from "node:path";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import {
  authorizeMutation,
  authorizeReadPath,
} from "../core/tools/tool-policy.js";

const readParameters = Type.Object({
  path: Type.String({ description: "Repository-relative file path" }),
  offset: Type.Optional(Type.Integer({ minimum: 1 })),
  limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 2000 })),
});

const bashParameters = Type.Object({
  command: Type.String({
    description: "Denied until a governed named-command grant exists",
  }),
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

function denialText(): string {
  const denial = authorizeMutation(false);
  return `${denial.code}: ${denial.detail}`;
}

export function registerGovernedTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "read",
    label: "read (Tiber governed)",
    description:
      "Read a bounded regular repository file after canonical path checks",
    parameters: readParameters,
    async execute(_id, parameters, _signal, _update, context) {
      try {
        const root = realpathSync(context.cwd);
        const lexical = resolve(root, parameters.path);
        const canonical = realpathSync(lexical);
        const decision = authorizeReadPath(root, parameters.path, canonical);
        if (!decision.allowed) {
          return {
            content: [
              { type: "text", text: `${decision.code}: ${decision.detail}` },
            ],
            details: { allowed: false, code: decision.code },
          };
        }
        const text = await readFile(canonical, "utf8");
        const lines = text.split("\n");
        const start = (parameters.offset ?? 1) - 1;
        const limit = parameters.limit ?? 2000;
        const selected = lines.slice(start, start + limit).join("\n");
        const bounded =
          Buffer.byteLength(selected, "utf8") <= 50 * 1024
            ? selected
            : `${Buffer.from(selected, "utf8")
                .subarray(0, 50 * 1024)
                .toString("utf8")}\n[Tiber preview truncated]`;
        return {
          content: [{ type: "text", text: bounded }],
          details: { allowed: true, path: canonical },
        };
      } catch {
        return {
          content: [
            {
              type: "text",
              text: "TIBER_READ_FAILED: file is unavailable or not a readable regular path",
            },
          ],
          details: { allowed: false, code: "TIBER_READ_FAILED" },
        };
      }
    },
  });

  for (const tool of [
    {
      name: "bash",
      label: "bash (Tiber governed)",
      parameters: bashParameters,
    },
    {
      name: "edit",
      label: "edit (Tiber governed)",
      parameters: editParameters,
    },
    {
      name: "write",
      label: "write (Tiber governed)",
      parameters: writeParameters,
    },
  ] as const) {
    pi.registerTool({
      name: tool.name,
      label: tool.label,
      description:
        "Mutation is denied until Tiber publishes an exclusive task claim",
      parameters: tool.parameters,
      execute: () =>
        Promise.resolve({
          content: [{ type: "text" as const, text: denialText() }],
          details: { allowed: false, code: "TIBER_MUTATION_CLAIM_REQUIRED" },
        }),
    });
  }
}
