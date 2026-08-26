import { execFileSync, spawn } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

interface RpcMessage {
  readonly type?: unknown;
  readonly id?: unknown;
  readonly method?: unknown;
  readonly message?: unknown;
  readonly success?: unknown;
  readonly data?: unknown;
}

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

function toRpcMessage(value: unknown): RpcMessage {
  if (typeof value !== "object" || value === null) {
    return {};
  }

  return {
    ...(typeof Reflect.get(value, "type") === "string"
      ? { type: Reflect.get(value, "type") }
      : {}),
    ...(typeof Reflect.get(value, "id") === "string"
      ? { id: Reflect.get(value, "id") }
      : {}),
    ...(typeof Reflect.get(value, "method") === "string"
      ? { method: Reflect.get(value, "method") }
      : {}),
    ...(typeof Reflect.get(value, "message") === "string"
      ? { message: Reflect.get(value, "message") }
      : {}),
    ...(typeof Reflect.get(value, "success") === "boolean"
      ? { success: Reflect.get(value, "success") }
      : {}),
    ...(Reflect.has(value, "data") ? { data: Reflect.get(value, "data") } : {}),
  };
}

function parseMessages(output: string): readonly RpcMessage[] {
  const lastCompleteLine = output.lastIndexOf("\n");
  if (lastCompleteLine < 0) {
    return [];
  }

  return output
    .slice(0, lastCompleteLine)
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line): RpcMessage => {
      const parsed: unknown = JSON.parse(line);
      return toRpcMessage(parsed);
    });
}

function packageVersion(root: string): string {
  const parsed: unknown = JSON.parse(
    readFileSync(join(root, "package.json"), "utf8"),
  );
  if (typeof parsed !== "object" || parsed === null) {
    throw new Error("package metadata is not an object");
  }
  const version: unknown = Reflect.get(parsed, "version");
  if (typeof version !== "string") {
    throw new Error("package metadata has no version");
  }
  return version;
}

function packedFilename(value: unknown): string | undefined {
  if (!Array.isArray(value) || value.length === 0) {
    return undefined;
  }

  const first: unknown = value[0];
  if (typeof first !== "object" || first === null) {
    return undefined;
  }

  const filename: unknown = Reflect.get(first, "filename");
  return typeof filename === "string" ? filename : undefined;
}

describe("the packed stock-Pi package", () => {
  it("installs, exercises signed task discovery, upgrades, and uninstalls outside the source repository", async () => {
    const root = resolve(import.meta.dirname, "../..");
    const temporaryDirectory = mkdtempSync(join(tmpdir(), "tiber-package-"));
    temporaryDirectories.push(temporaryDirectory);

    const packOutput = execFileSync(
      "npm",
      ["pack", "--json", "--pack-destination", temporaryDirectory],
      { cwd: root, encoding: "utf8" },
    );
    const packResult: unknown = JSON.parse(packOutput);
    const filename = packedFilename(packResult);
    expect(filename).toBeTypeOf("string");

    const tarball = join(temporaryDirectory, filename ?? "missing.tgz");
    const archiveEntries = execFileSync("tar", ["-tzf", tarball], {
      encoding: "utf8",
    });
    expect(archiveEntries).toContain("package/dist/extension/index.js");
    expect(archiveEntries).toContain("package/prompts/tiber-setup-agent.md");
    expect(archiveEntries).not.toContain("Cargo.toml");
    expect(archiveEntries).not.toContain("package/crates/");
    expect(archiveEntries).not.toContain("package/src/");
    expect(archiveEntries).not.toContain("package/tests/");
    expect(archiveEntries).not.toMatch(/\.d\.ts(?:\.map)?$/mu);
    expect(archiveEntries).not.toMatch(/\.js\.map$/mu);

    execFileSync("tar", ["-xzf", tarball, "-C", temporaryDirectory]);
    const packedManifest: unknown = JSON.parse(
      readFileSync(join(temporaryDirectory, "package", "package.json"), "utf8"),
    );
    expect(
      typeof packedManifest === "object" && packedManifest !== null
        ? Reflect.get(packedManifest, "engines")
        : undefined,
    ).toEqual({ node: ">=22.23.1 <23" });
    expect(
      typeof packedManifest === "object" && packedManifest !== null
        ? Reflect.get(packedManifest, "peerDependencies")
        : undefined,
    ).toMatchObject({
      "@earendil-works/pi-coding-agent": ">=0.84.2 <1",
      "@earendil-works/pi-ai": ">=0.84.2 <1",
      "@earendil-works/pi-tui": ">=0.84.2 <1",
      typebox: ">=1.3.7 <2",
    });

    const home = join(temporaryDirectory, "home");
    const agentDirectory = join(temporaryDirectory, "pi-agent");
    const workspace = join(temporaryDirectory, "workspace");
    const remote = join(temporaryDirectory, "remote.git");
    const signingKey = join(temporaryDirectory, "signing-key");
    const allowedSigners = join(temporaryDirectory, "allowed-signers");
    mkdirSync(home);
    mkdirSync(agentDirectory);
    mkdirSync(remote);
    execFileSync("git", ["init", "--bare", "--quiet", remote]);
    execFileSync("git", ["clone", "--quiet", remote, workspace]);
    execFileSync("ssh-keygen", [
      "-q",
      "-t",
      "ed25519",
      "-N",
      "",
      "-f",
      signingKey,
    ]);
    writeFileSync(
      allowedSigners,
      `release@example.test ${readFileSync(`${signingKey}.pub`, "utf8").trim()}\n`,
    );
    for (const [name, value] of [
      ["user.name", "Release Candidate"],
      ["user.email", "release@example.test"],
      ["user.signingkey", signingKey],
      ["gpg.format", "ssh"],
      ["gpg.ssh.allowedSignersFile", allowedSigners],
    ] as const)
      execFileSync("git", ["config", name, value], { cwd: workspace });

    const piBinary = resolve(root, "node_modules/.bin/pi");
    const environment = {
      ...process.env,
      HOME: home,
      PI_CODING_AGENT_DIR: agentDirectory,
    };

    execFileSync(piBinary, ["install", join(temporaryDirectory, "package")], {
      cwd: workspace,
      env: environment,
      stdio: "pipe",
    });

    const child = spawn(piBinary, ["--mode", "rpc", "--no-session"], {
      cwd: workspace,
      env: environment,
      stdio: ["pipe", "pipe", "pipe"],
    });

    let standardOutput = "";
    let standardError = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      standardOutput += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      standardError += chunk;
    });

    child.stdin.write('{"id":"commands","type":"get_commands"}\n');
    child.stdin.write(
      '{"id":"doctor","type":"prompt","message":"/tiber:doctor"}\n',
    );
    child.stdin.write(
      '{"id":"create","type":"prompt","message":"/tiber:task create Release candidate smoke"}\n',
    );
    child.stdin.write(
      '{"id":"tasks","type":"prompt","message":"/tiber:tasks"}\n',
    );

    await new Promise<void>((resolvePromise, rejectPromise) => {
      const timeout = setTimeout(() => {
        child.kill("SIGTERM");
        rejectPromise(new Error(`Pi RPC timed out. stderr: ${standardError}`));
      }, 10_000);

      const poll = setInterval(() => {
        const messages = parseMessages(standardOutput);
        const completed = ["doctor", "create", "tasks"].every((id) =>
          messages.some(
            (message) => message.id === id && message.success === true,
          ),
        );
        if (!completed) {
          return;
        }

        clearTimeout(timeout);
        clearInterval(poll);
        child.kill("SIGTERM");
        resolvePromise();
      }, 20);
    });

    const messages = parseMessages(standardOutput);
    const commandResponse = messages.find(
      (message) => message.id === "commands",
    );
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:doctor");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber-setup");
    expect(JSON.stringify(commandResponse?.data)).not.toContain(
      "tiber-setup-agent",
    );
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:green");
    expect(JSON.stringify(commandResponse?.data)).toContain(
      "tiber:final-review",
    );
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:done");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:deliver");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:ci");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:review");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:campaign");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:attention");
    expect(JSON.stringify(commandResponse?.data)).toContain("tiber:exception");

    const notification = messages.find(
      (message) =>
        message.type === "extension_ui_request" && message.method === "notify",
    );
    expect(notification?.message).toContain(
      `@jwilger/tiber ${packageVersion(root)}`,
    );
    expect(notification?.message).toContain("Mode: read-only-bootstrap");
    expect(notification?.message).toContain(`Repository: ${workspace}`);
    expect(
      messages.some(
        (message) =>
          message.type === "extension_ui_request" &&
          message.method === "notify" &&
          typeof message.message === "string" &&
          message.message.includes("Release candidate smoke"),
      ),
    ).toBe(true);

    if (child.exitCode === null)
      await new Promise<void>((resolvePromise) => {
        child.once("exit", () => {
          resolvePromise();
        });
      });
    execFileSync(piBinary, ["update", join(temporaryDirectory, "package")], {
      cwd: workspace,
      env: environment,
      stdio: "pipe",
    });
    execFileSync(piBinary, ["remove", join(temporaryDirectory, "package")], {
      cwd: workspace,
      env: environment,
      stdio: "pipe",
    });
  }, 30_000);
});
