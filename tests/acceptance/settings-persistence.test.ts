import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

async function invokeSettings(
  root: string,
  cwd: string,
  environment: NodeJS.ProcessEnv,
  command: string,
): Promise<string> {
  const piBinary = resolve(root, "node_modules/.bin/pi");
  const child = spawn(
    piBinary,
    [
      "--mode",
      "rpc",
      "--no-session",
      "-e",
      resolve(root, "src/extension/index.ts"),
    ],
    {
      cwd,
      env: environment,
      stdio: ["pipe", "pipe", "pipe"],
    },
  );

  let output = "";
  let errorOutput = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    output += chunk;
  });
  child.stderr.on("data", (chunk: string) => {
    errorOutput += chunk;
  });

  child.stdin.write(
    `${JSON.stringify({ id: "settings", type: "prompt", message: command })}\n`,
  );

  await new Promise<void>((resolvePromise, rejectPromise) => {
    const timeout = setTimeout(() => {
      clearInterval(poll);
      child.kill("SIGTERM");
      rejectPromise(new Error(`Pi RPC timed out: ${errorOutput}`));
    }, 10_000);
    const poll = setInterval(() => {
      if (!output.includes('"id":"settings","type":"response"')) {
        return;
      }
      clearTimeout(timeout);
      clearInterval(poll);
      child.kill("SIGTERM");
      resolvePromise();
    }, 20);
  });

  return output;
}

describe("layered settings persistence", () => {
  it("shares user-global values while keeping project overrides separate", async () => {
    const root = resolve(import.meta.dirname, "../..");
    const temporaryDirectory = mkdtempSync(join(tmpdir(), "tiber-settings-"));
    temporaryDirectories.push(temporaryDirectory);

    const home = join(temporaryDirectory, "home");
    const agentDirectory = join(temporaryDirectory, "pi-agent");
    const repositoryA = join(temporaryDirectory, "repository-a");
    const repositoryB = join(temporaryDirectory, "repository-b");
    for (const directory of [home, agentDirectory, repositoryA, repositoryB]) {
      mkdirSync(directory);
    }
    for (const repository of [repositoryA, repositoryB]) {
      execFileSync("git", ["init", "--quiet"], { cwd: repository });
    }

    const environment = {
      ...process.env,
      HOME: home,
      PI_CODING_AGENT_DIR: agentDirectory,
    };

    const invoke = (repository: string, command: string): Promise<string> =>
      invokeSettings(root, repository, environment, command);

    await invoke(
      repositoryA,
      "/tiber:settings set global assuranceLevel workspace-isolated",
    );
    await invoke(
      repositoryA,
      "/tiber:settings set project assuranceLevel hermetic",
    );

    const repositoryAView = await invoke(repositoryA, "/tiber:settings show");
    const repositoryBView = await invoke(repositoryB, "/tiber:settings show");

    expect(repositoryAView).toContain(
      "assuranceLevel | host-trusted | workspace-isolated | hermetic | hermetic (project)",
    );
    expect(repositoryBView).toContain(
      "assuranceLevel | host-trusted | workspace-isolated | inherit | workspace-isolated (user-global)",
    );

    await invoke(
      repositoryB,
      "/tiber:settings lock assuranceLevel workspace-and-network-isolated",
    );
    await invoke(
      repositoryB,
      "/tiber:settings set project assuranceLevel host-trusted",
    );
    await invoke(
      repositoryB,
      "/tiber:settings secret context7 environment CONTEXT7_API_KEY",
    );
    const lockedView = await invoke(repositoryB, "/tiber:settings show");
    expect(lockedView).toContain(
      "Assurance after ceiling: workspace-and-network-isolated",
    );
    expect(lockedView).toContain(
      "Conflict: project requested host-trusted, but the user-global ceiling requires workspace-and-network-isolated or stronger",
    );
    expect(lockedView).toContain("context7=environment:CONTEXT7_API_KEY");

    const identityA = readFileSync(
      join(repositoryA, ".git", "tiber", "project-id"),
      "utf8",
    ).trim();
    const identityB = readFileSync(
      join(repositoryB, ".git", "tiber", "project-id"),
      "utf8",
    ).trim();
    expect(identityA).not.toBe(identityB);

    const globalDocument = readFileSync(
      join(agentDirectory, "tiber", "settings.json"),
      "utf8",
    );
    expect(globalDocument).toContain('"assuranceLevel": "workspace-isolated"');
  }, 30_000);
});
