import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { access } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";

const PROTOCOL_VERSION = 1;
const MAX_RESPONSE_BYTES = 256 * 1024;
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const defaultBinary = resolve(
  packageRoot,
  ".runtime",
  "current",
  "bin",
  "tiber",
);

type ProtocolResult = {
  outcome: "ok" | "error";
  result?: any;
  error?: { code: string; class: string; message: string };
};

async function invokeRaw(
  operation: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<any> {
  const binary = process.env.TIBER_BIN || defaultBinary;
  try {
    await access(binary);
  } catch {
    throw new Error(
      `Rust runtime unavailable at ${binary}. Run npm run runtime:install. No TypeScript policy fallback is available.`,
    );
  }
  const correlationId = randomUUID();
  const request = JSON.stringify({
    protocol_version: PROTOCOL_VERSION,
    correlation_id: correlationId,
    ...operation,
  });
  return await new Promise((resolvePromise, reject) => {
    const child = spawn(binary, ["service", "stdio"], {
      stdio: ["pipe", "pipe", "pipe"],
      shell: false,
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill("SIGTERM");
      reject(new Error("Rust protocol request timed out"));
    }, 10_000);
    const abort = () => {
      child.kill("SIGTERM");
      reject(new Error("Rust protocol request cancelled"));
    };
    signal?.addEventListener("abort", abort, { once: true });
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (Buffer.byteLength(stdout) > MAX_RESPONSE_BYTES) child.kill("SIGTERM");
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      clearTimeout(timeout);
      signal?.removeEventListener("abort", abort);
      if (code !== 0)
        return reject(
          new Error(
            `Rust runtime failed (${code}): ${stderr.trim() || "no diagnostic"}`,
          ),
        );
      try {
        const response = JSON.parse(stdout.trim()) as ProtocolResult;
        if (response.outcome !== "ok")
          throw new Error(
            `${response.error?.code ?? "protocol.error"}: ${response.error?.message ?? "unknown Rust rejection"}`,
          );
        resolvePromise(response.result);
      } catch (error) {
        reject(
          new Error(`Malformed or rejected Rust response: ${String(error)}`),
        );
      }
    });
    child.stdin.end(`${request}\n`);
  });
}

async function invoke(
  operation: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<any> {
  const negotiation = await invokeRaw(
    { operation: "negotiate", supported_versions: [PROTOCOL_VERSION] },
    signal,
  );
  if (
    negotiation.executable !== "tiber" ||
    negotiation.version !== "0.1.0" ||
    negotiation.selected_version !== PROTOCOL_VERSION
  ) {
    throw new Error(
      `Incompatible Rust runtime: expected tiber 0.1.0 protocol ${PROTOCOL_VERSION}.`,
    );
  }
  return invokeRaw(operation, signal);
}

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => {
    const decision = await invoke(
      {
        operation: "authorize_tool_call",
        tool_name: event.toolName,
        input: event.input,
      },
      ctx.signal,
    );
    if (!decision.authorized)
      return { block: true, reason: decision.reason, terminate: true };
  });

  pi.registerTool({
    name: "tiber_route",
    label: "Tiber Route",
    description:
      "Ask the authoritative Rust policy engine to select and apply a Pi model for a semantic work role.",
    parameters: Type.Object({
      role: StringEnum([
        "bounded-helper",
        "substantive-worker",
        "independent-reviewer",
        "verifier",
      ] as const),
    }),
    async execute(_id, params, signal, _update, ctx) {
      const available = await ctx.modelRegistry.getAvailable();
      const catalog = available.map((model) => ({
        provider: model.provider,
        model: model.id,
        reasoning: model.reasoning,
        input: model.input,
        authenticated: true,
      }));
      const decision = await invoke(
        { operation: "resolve_role", role: params.role, catalog },
        signal,
      );
      const selected = ctx.modelRegistry.find(
        decision.selection.provider,
        decision.selection.model,
      );
      if (!selected || !(await pi.setModel(selected)))
        throw new Error(
          "Pi could not apply the exact Rust-authorized model; no fallback was attempted.",
        );
      return {
        content: [
          {
            type: "text",
            text: `Applied ${selected.provider}/${selected.id} for ${params.role} (fallback: ${decision.fallback_used}).`,
          },
        ],
        details: decision,
      };
    },
  });

  pi.registerCommand("tiber-doctor", {
    description: "Check the package-owned Rust runtime and protocol",
    handler: async (_args, ctx) => {
      const result = await invoke({ operation: "doctor" });
      ctx.ui.notify(
        `${result.executable} ${result.version}; protocol ${result.protocol_version}`,
        "info",
      );
    },
  });
}
