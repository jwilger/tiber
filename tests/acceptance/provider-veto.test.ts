import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

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

describe("stock Pi provider veto", () => {
  it("blocks ordinary inference but admits the explicit bounded setup conversation", async () => {
    const root = resolve(import.meta.dirname, "../..");
    const temporaryDirectory = mkdtempSync(join(tmpdir(), "tiber-veto-"));
    temporaryDirectories.push(temporaryDirectory);
    const workspace = join(temporaryDirectory, "workspace");
    const home = join(temporaryDirectory, "home");
    const agentDirectory = join(temporaryDirectory, "agent");
    for (const directory of [workspace, home, agentDirectory])
      mkdirSync(directory);
    execFileSync("git", ["init", "--quiet"], { cwd: workspace });

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
        const delta =
          providerRequests === 1
            ? {
                role: "assistant",
                tool_calls: [
                  {
                    index: 0,
                    id: "call_setup_inspect",
                    type: "function",
                    function: {
                      name: "tiber_setup",
                      arguments: '{"operation":"inspect"}',
                    },
                  },
                ],
              }
            : { role: "assistant", content: "Setup inspection completed." };
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
                finish_reason: providerRequests === 1 ? "tool_calls" : "stop",
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

    const setupProcess = launch();
    const setupStarted = waitForOutput(setupProcess, '"type":"agent_start"');
    const setupCompleted = waitForOutput(setupProcess, '"type":"agent_end"');
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "setup", type: "prompt", message: "/tiber-setup" })}\n`,
    );
    await waitForResponse(setupProcess, "setup");
    const setupOutput = await setupStarted;
    const setupReachedProvider = await Promise.race([
      providerRequested.then(() => true),
      new Promise<false>((resolvePromise) =>
        setTimeout(() => {
          resolvePromise(false);
        }, 3_000),
      ),
    ]);
    const completedSetupOutput = await setupCompleted;
    const commandDuringSetup = waitForResponse(setupProcess, "other-command");
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "other-command", type: "prompt", message: "/tiber:settings show" })}\n`,
    );
    const commandDuringSetupOutput = await commandDuringSetup;
    const cancelResponse = waitForResponse(setupProcess, "cancel");
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "cancel", type: "prompt", message: "/tiber-setup cancel" })}\n`,
    );
    const cancelOutput = await cancelResponse;
    const requestsAfterCancel = providerRequests;
    const blockedAfterCancel = waitForResponse(setupProcess, "blocked");
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "blocked", type: "prompt", message: "Attempt provider dispatch after setup cancellation" })}\n`,
    );
    const blockedOutput = await blockedAfterCancel;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 200));
    await stopProcess(setupProcess);
    await new Promise<void>((resolvePromise, rejectPromise) => {
      server.close((error) => {
        if (error) rejectPromise(error);
        else resolvePromise();
      });
    });

    expect(setupOutput).toContain('"type":"agent_start"');
    expect(setupReachedProvider).toBe(true);
    expect(providerRequests).toBeGreaterThan(1);
    expect(completedSetupOutput).toContain('"toolName":"tiber_setup"');
    expect(completedSetupOutput).toContain('"disposition":"inspected"');
    expect(providerBodies.some((body) => body.includes('"name":"read"'))).toBe(
      true,
    );
    expect(
      providerBodies.some((body) => body.includes('"name":"tiber_setup"')),
    ).toBe(true);
    expect(providerBodies.join("\n")).not.toContain('"name":"bash"');
    expect(providerBodies.join("\n")).not.toContain("TIBER_WORKFLOW_STATE");
    expect(commandDuringSetupOutput).toContain("TIBER_SETUP_IN_PROGRESS");
    expect(cancelOutput).toContain("TIBER_SETUP_CANCELLED");
    expect(blockedOutput).toContain("TIBER_CONTAINMENT_ATTESTATION_MISSING");
    expect(providerRequests).toBe(requestsAfterCancel);
  }, 30_000);
});
