#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const EXPECTED_ORIGIN = "github.com/jwilger/tiber";

export function isExpectedOrigin(remote) {
  return [
    /^git@github\.com:jwilger\/tiber(?:\.git)?$/,
    /^https:\/\/github\.com\/jwilger\/tiber(?:\.git)?$/,
    /^ssh:\/\/git@github\.com\/jwilger\/tiber(?:\.git)?$/,
  ].some((pattern) => pattern.test(remote));
}

export function managedClonePath(environment = process.env, home = homedir()) {
  const agentDirectory =
    environment.PI_CODING_AGENT_DIR || join(home, ".pi", "agent");
  return join(agentDirectory, "git", "github.com", "jwilger", "tiber");
}

function execute(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: options.cwd,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

function capture(command, args, cwd) {
  return execute(command, args, { cwd, capture: true }).trim();
}

export function installDevSnapshot({
  repositoryRoot,
  environment = process.env,
  home = homedir(),
  run = execute,
  output = console.log,
  warning = console.warn,
}) {
  const read = (command, args, cwd = repositoryRoot) =>
    run(command, args, { cwd, capture: true }).trim();

  const branch = read("git", ["symbolic-ref", "--quiet", "--short", "HEAD"]);
  if (!branch)
    throw new Error("dev:install requires HEAD to be on a named branch");

  const sha = read("git", ["rev-parse", "HEAD"]);
  if (!/^[0-9a-f]{40}$/.test(sha))
    throw new Error(`git returned an invalid HEAD SHA: ${sha}`);

  const dirty = read("git", ["status", "--porcelain"]);
  if (dirty) {
    warning(
      `Working tree is dirty; dev:install will exclude uncommitted changes and install committed HEAD ${sha}.`,
    );
  }

  const origin = read("git", ["remote", "get-url", "origin"]);
  if (!isExpectedOrigin(origin)) {
    throw new Error(
      `refusing to publish from unexpected origin ${JSON.stringify(origin)}; expected ${EXPECTED_ORIGIN}`,
    );
  }

  output(`Pushing ${branch} at ${sha} to origin...`);
  run("git", ["push", "origin", `HEAD:refs/heads/${branch}`], {
    cwd: repositoryRoot,
  });

  const remoteLine = read("git", [
    "ls-remote",
    "--heads",
    "origin",
    `refs/heads/${branch}`,
  ]);
  const remoteSha = remoteLine.split(/\s+/, 1)[0];
  if (remoteSha !== sha) {
    throw new Error(
      `origin branch ${branch} resolved to ${remoteSha || "nothing"}, expected ${sha}`,
    );
  }

  const source = `git:github.com/jwilger/tiber@${sha}`;
  output(`Installing immutable Pi package ${source}...`);
  run("pi", ["install", source], { cwd: repositoryRoot });

  const clone = managedClonePath(environment, home);
  if (!existsSync(clone))
    throw new Error(
      `Pi reported success but its managed clone is missing at ${clone}`,
    );

  const installedSha = read("git", ["rev-parse", "HEAD"], clone);
  if (installedSha !== sha) {
    throw new Error(
      `Pi managed clone contains ${installedSha}, expected ${sha}`,
    );
  }

  output("Installing and verifying the package-owned Rust runtime...");
  run("npm", ["run", "runtime:install"], { cwd: clone });

  const executable = join(
    clone,
    ".runtime",
    "current",
    "bin",
    process.platform === "win32" ? "tiber.exe" : "tiber",
  );
  if (!existsSync(executable))
    throw new Error(`verified Tiber executable is missing at ${executable}`);
  const doctor = read(executable, ["doctor"], clone);
  if (doctor !== "tiber 0.1.0 protocol 1") {
    throw new Error(
      `installed Tiber runtime is incompatible: ${doctor || "no doctor output"}`,
    );
  }

  output(`Installed Tiber dev snapshot ${sha} from ${branch}.`);
  output("Restart Pi to load the installed snapshot.");
  return { branch, sha, source, clone, executable };
}

function main() {
  const repositoryRoot = fileURLToPath(new URL("..", import.meta.url));
  try {
    installDevSnapshot({ repositoryRoot });
  } catch (error) {
    console.error(
      `tiber dev:install failed: ${error instanceof Error ? error.message : String(error)}`,
    );
    process.exitCode = 1;
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1])
  main();
