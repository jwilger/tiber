import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
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
    const server = createServer((_request, response) => {
      providerRequests += 1;
      response.writeHead(500).end();
    });
    await new Promise<void>((resolvePromise) =>
      server.listen(0, "127.0.0.1", resolvePromise),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("fake provider did not bind");

    const environment = {
      ...process.env,
      HOME: home,
      PI_CODING_AGENT_DIR: agentDirectory,
      OPENAI_API_KEY: "test-only",
      OPENAI_BASE_URL: `http://127.0.0.1:${String(address.port)}/v1`,
    };
    const args = [
      "--mode",
      "rpc",
      "--no-session",
      "--provider",
      "openai",
      "--model",
      "gpt-4o-mini",
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
    setupProcess.stdin.write(
      `${JSON.stringify({ id: "setup", type: "prompt", message: "/tiber-setup" })}\n`,
    );
    await waitForResponse(setupProcess, "setup");
    const setupOutput = await setupStarted;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 200));
    await stopProcess(setupProcess);
    await new Promise<void>((resolvePromise, rejectPromise) => {
      server.close((error) => {
        if (error) rejectPromise(error);
        else resolvePromise();
      });
    });

    expect(setupOutput).toContain('"type":"agent_start"');
    expect(providerRequests).toBe(0);
  }, 30_000);
});
