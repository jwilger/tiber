import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCommandAuthority } from "../../src/adapters/commands/file-command-authority.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

async function stopProcess(child: ReturnType<typeof spawn>): Promise<void> {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  await once(child, "exit");
}

function waitForOutput(
  child: ReturnType<typeof spawn>,
  marker: string,
): Promise<string> {
  return new Promise((resolvePromise, rejectPromise) => {
    let output = "";
    let errors = "";
    const { stdout, stderr } = child;
    if (stdout === null || stderr === null) {
      rejectPromise(new Error("Pi RPC pipes are unavailable"));
      return;
    }
    stdout.setEncoding("utf8");
    stderr.setEncoding("utf8");
    const timeout = setTimeout(() => {
      rejectPromise(new Error(`Pi RPC timed out: ${errors}`));
    }, 10_000);
    stdout.on("data", (chunk: string) => {
      output += chunk;
      if (output.includes(marker)) {
        clearTimeout(timeout);
        resolvePromise(output);
      }
    });
    stderr.on("data", (chunk: string) => {
      errors += chunk;
    });
  });
}

function waitForResponse(
  child: ReturnType<typeof spawn>,
  id: string,
): Promise<string> {
  return waitForOutput(child, `"id":"${id}","type":"response"`);
}

function waitForUiRequest(
  child: ReturnType<typeof spawn>,
  method: "select" | "confirm",
): Promise<{ readonly id: string; readonly output: string }> {
  return new Promise((resolvePromise, rejectPromise) => {
    let output = "";
    const { stdout } = child;
    if (stdout === null) {
      rejectPromise(new Error("Pi RPC output is unavailable"));
      return;
    }
    stdout.setEncoding("utf8");
    const timeout = setTimeout(() => {
      rejectPromise(new Error(`Pi RPC UI timed out: ${output}`));
    }, 10_000);
    const observe = (chunk: string) => {
      output += chunk;
      for (const line of output.split("\n")) {
        if (line.length === 0) continue;
        let parsed: unknown;
        try {
          parsed = JSON.parse(line);
        } catch {
          continue;
        }
        if (
          typeof parsed === "object" &&
          parsed !== null &&
          "type" in parsed &&
          parsed.type === "extension_ui_request" &&
          "method" in parsed &&
          parsed.method === method &&
          "id" in parsed &&
          typeof parsed.id === "string"
        ) {
          clearTimeout(timeout);
          stdout.off("data", observe);
          resolvePromise({ id: parsed.id, output });
          return;
        }
      }
    };
    stdout.on("data", observe);
  });
}

describe("stock Pi provider veto", () => {
  it("blocks ordinary inference but admits deterministic setup without provider dispatch", async () => {
    const root = resolve(import.meta.dirname, "../..");
    const temporaryDirectory = mkdtempSync(join(tmpdir(), "tiber-veto-"));
    temporaryDirectories.push(temporaryDirectory);
    const workspace = join(temporaryDirectory, "workspace");
    const home = join(temporaryDirectory, "home");
    const agentDirectory = join(temporaryDirectory, "agent");
    for (const directory of [workspace, home, agentDirectory])
      mkdirSync(directory);
    execFileSync("git", ["init", "--quiet"], { cwd: workspace });
    writeFileSync(
      join(workspace, "package.json"),
      `${JSON.stringify({ scripts: { test: "node --test", lint: "eslint ." } })}\n`,
    );

    let providerRequests = 0;
    const providerBodies: string[] = [];
    let observeProviderRequest: (() => void) | undefined;
    const providerRequested = new Promise<void>((resolvePromise) => {
      observeProviderRequest = resolvePromise;
    });
    const server = createServer((request, response) => {
      providerRequests += 1;
      const chunks: Buffer[] = [];
      request.on("data", (chunk: Buffer) => chunks.push(chunk));
      request.on("end", () => {
        providerBodies.push(Buffer.concat(chunks).toString("utf8"));
        observeProviderRequest?.();
        response.writeHead(200, { "content-type": "text/event-stream" });
        const delta = {
          role: "assistant",
          content: "Provider reached after setup.",
        };
        response.write(
          `data: ${JSON.stringify({
            id: `response-${String(providerRequests)}`,
            object: "chat.completion.chunk",
            created: 1,
            model: "test-model",
            choices: [{ index: 0, delta, finish_reason: null }],
          })}\n\n`,
        );
        response.write(
          `data: ${JSON.stringify({
            id: `response-${String(providerRequests)}`,
            object: "chat.completion.chunk",
            created: 1,
            model: "test-model",
            choices: [
              {
                index: 0,
                delta: {},
                finish_reason: "stop",
              },
            ],
          })}\n\n`,
        );
        response.end("data: [DONE]\n\n");
      });
    });
    await new Promise<void>((resolvePromise) =>
      server.listen(0, "127.0.0.1", resolvePromise),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("fake provider did not bind");

    writeFileSync(
      join(agentDirectory, "models.json"),
      `${JSON.stringify(
        {
          providers: {
            test: {
              baseUrl: `http://127.0.0.1:${String(address.port)}/v1`,
              api: "openai-completions",
              apiKey: "test-only",
              models: [{ id: "test-model" }],
            },
          },
        },
        null,
        2,
      )}\n`,
    );
    const environment = {
      ...process.env,
      HOME: home,
      PI_CODING_AGENT_DIR: agentDirectory,
    };
    const args = [
      "--mode",
      "rpc",
      "--no-session",
      "--approve",
      "--provider",
      "test",
      "--model",
      "test-model",
      "-e",
      resolve(root, "src/extension/index.ts"),
    ];
    const launch = () =>
      spawn(resolve(root, "node_modules/.bin/pi"), args, {
        cwd: workspace,
        env: environment,
        stdio: ["pipe", "pipe", "pipe"],
      });

    const settingsProcess = launch();
    settingsProcess.stdin.write(
      `${JSON.stringify({ id: "settings", type: "prompt", message: "/tiber:settings set project assuranceLevel workspace-isolated" })}\n`,
    );
    await waitForResponse(settingsProcess, "settings");
    await stopProcess(settingsProcess);

    const promptProcess = launch();
    promptProcess.stdin.write(
      `${JSON.stringify({ id: "prompt", type: "prompt", message: "Attempt provider dispatch" })}\n`,
    );
    const output = await waitForResponse(promptProcess, "prompt");
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 200));
    await stopProcess(promptProcess);

    expect(output).toContain("TIBER_CONTAINMENT_ATTESTATION_MISSING");
    expect(providerRequests).toBe(0);

    const cancelledSetupProcess = launch();
    const cancelledSetupResponse = waitForResponse(
      cancelledSetupProcess,
      "setup-to-cancel",
    );
    cancelledSetupProcess.stdin.write(
      `${JSON.stringify({ id: "setup-to-cancel", type: "prompt", message: "/tiber-setup" })}\n`,
    );
    const cancelledRequest = await waitForUiRequest(
      cancelledSetupProcess,
      "select",
    );
    cancelledSetupProcess.stdin.write(
      `${JSON.stringify({
        type: "extension_ui_response",
        id: cancelledRequest.id,
        cancelled: true,
      })}\n`,
    );
    const cancelledOutput = await cancelledSetupResponse;
    await stopProcess(cancelledSetupProcess);
    expect(cancelledOutput).toContain("TIBER_SETUP_CANCELLED");
    expect(providerBodies).toEqual([]);

    const setupProcess = launch();
    const setupResponse = waitForResponse(setupProcess, "setup");
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "setup", type: "prompt", message: "/tiber-setup" })}\n`,
    );
    const autonomyRequest = await waitForUiRequest(setupProcess, "select");
    setupProcess.stdin.write(
      `${JSON.stringify({
        type: "extension_ui_response",
        id: autonomyRequest.id,
        value:
          "Handle routine work, ask before risky or unfamiliar actions (recommended)",
      })}\n`,
    );
    const isolationRequest = await waitForUiRequest(setupProcess, "select");
    setupProcess.stdin.write(
      `${JSON.stringify({
        type: "extension_ui_response",
        id: isolationRequest.id,
        value: "Use this repository with Tiber guardrails (recommended)",
      })}\n`,
    );
    const confirmationRequest = await waitForUiRequest(setupProcess, "confirm");
    setupProcess.stdin.write(
      `${JSON.stringify({
        type: "extension_ui_response",
        id: confirmationRequest.id,
        confirmed: true,
      })}\n`,
    );
    const setupOutput = await setupResponse;
    expect(providerBodies).toEqual([]);

    const ordinaryResponse = waitForResponse(setupProcess, "ordinary");
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "ordinary", type: "prompt", message: "Attempt provider dispatch after setup" })}\n`,
    );
    const ordinaryOutput = await ordinaryResponse;
    await providerRequested;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
    await stopProcess(setupProcess);
    await new Promise<void>((resolvePromise, rejectPromise) => {
      server.close((error) => {
        if (error) rejectPromise(error);
        else resolvePromise();
      });
    });

    expect(setupOutput).toContain("Tiber is ready");
    expect(setupOutput).toContain("routine work");
    const commands = new FileCommandAuthority(workspace);
    const catalog = commands.loadCatalog();
    const grant = commands.readGrant();
    expect(catalog.ok).toBe(true);
    expect(grant.ok).toBe(true);
    if (catalog.ok && grant.ok) {
      expect(catalog.value.commands.map(({ name }) => name)).toEqual([
        "test",
        "lint",
      ]);
      expect(grant.value).toEqual({
        kind: "some",
        value: catalog.value.digest,
      });
    }
    expect(ordinaryOutput).toContain('"success":true');
    expect(providerRequests).toBe(1);
  }, 30_000);
});
