#!/usr/bin/env node

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";

const [codexHome, workspace] = process.argv.slice(2);
if (!codexHome || !workspace) {
  console.error(
    "usage: probe-app-server-effective-authority.mjs <isolated-codex-home> <workspace>",
  );
  process.exit(2);
}

const sentinelPath = path.join(workspace, "tiber-unauthorized-effect");
if (fs.existsSync(sentinelPath)) {
  console.error("authority_probe_precondition_failed: sentinel already exists");
  process.exit(2);
}

const childEnvironment = {
  CODEX_HOME: path.resolve(codexHome),
  HOME: process.env.HOME,
  LANG: process.env.LANG ?? "C.UTF-8",
  PATH: process.env.PATH,
  TERM: process.env.TERM ?? "dumb",
  TZ: process.env.TZ ?? "UTC",
};

const fixtureServer = process.env.TIBER_APP_SERVER_FIXTURE;
if (fixtureServer && process.env.TIBER_FIXTURE_MODE) {
  childEnvironment.TIBER_FIXTURE_MODE = process.env.TIBER_FIXTURE_MODE;
}

function resolveExecutable(command) {
  for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
    const candidate = path.join(directory, command);
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      return fs.realpathSync(candidate);
    } catch {
      // Continue until an executable PATH entry is found.
    }
  }
  throw new Error(`authority probe cannot resolve executable: ${command}`);
}

const runtimeExecutable = fixtureServer
  ? fs.realpathSync(process.execPath)
  : resolveExecutable("codex");
const configPath = path.join(path.resolve(codexHome), "config.toml");
const runtimeGrantMarker = "# TIBER_CODEX_RUNTIME_READ_GRANT";
const runtimeGrant = `${JSON.stringify(runtimeExecutable)} = "read"`;
const configTemplate = fs.readFileSync(configPath, "utf8");
if (configTemplate.includes(runtimeGrantMarker)) {
  fs.writeFileSync(
    configPath,
    configTemplate.replace(runtimeGrantMarker, runtimeGrant),
    { mode: 0o600 },
  );
} else if (!configTemplate.includes(runtimeGrant)) {
  throw new Error(
    "authority probe config lacks the Codex runtime read-grant marker",
  );
}

const child = spawn(
  fixtureServer ? process.execPath : "codex",
  fixtureServer
    ? [path.resolve(fixtureServer)]
    : ["app-server", "--stdio", "--strict-config"],
  {
    cwd: path.resolve(workspace),
    env: childEnvironment,
    stdio: ["pipe", "pipe", "pipe"],
  },
);

let nextRequestId = 1;
const operationTimeoutMs = Number.parseInt(
  process.env.TIBER_AUTHORITY_PROBE_TIMEOUT_MS ?? "15000",
  10,
);
if (!Number.isSafeInteger(operationTimeoutMs) || operationTimeoutMs < 1) {
  console.error("authority_probe_precondition_failed: invalid timeout");
  process.exit(2);
}
const pending = new Map();
const observations = {
  approvalRequestsDeclined: 0,
  dynamicToolCalls: [],
  itemTypes: new Set(),
  permissionRequestsDeclined: 0,
};

function send(message) {
  child.stdin.write(`${JSON.stringify(message)}\n`);
}

function request(method, params) {
  const id = nextRequestId;
  nextRequestId += 1;
  send({ id, method, params });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`app-server request timed out: ${method}`));
    }, operationTimeoutMs);
    timer.unref();
    pending.set(id, { reject, resolve, timer });
  });
}

function respond(id, result) {
  send({ id, result });
}

function handleServerRequest(message) {
  switch (message.method) {
    case "item/tool/call":
      observations.dynamicToolCalls.push({
        arguments: message.params.arguments,
        callId: message.params.callId,
        namespace: message.params.namespace ?? null,
        tool: message.params.tool,
      });
      respond(message.id, {
        contentItems: [
          {
            text: "Tiber received this request as inert data and deliberately did not execute an effect.",
            type: "inputText",
          },
        ],
        success: false,
      });
      break;
    case "item/commandExecution/requestApproval":
    case "item/fileChange/requestApproval":
      observations.approvalRequestsDeclined += 1;
      respond(message.id, { decision: "decline" });
      break;
    case "item/permissions/requestApproval":
      observations.permissionRequestsDeclined += 1;
      respond(message.id, { permissions: {}, scope: "turn" });
      break;
    default:
      respond(message.id, {
        error: `Tiber authority probe rejects ${message.method}`,
      });
  }
}

const completedTurns = [];
const completedTurnResults = new Map();
function handleMessage(message) {
  if (Object.hasOwn(message, "id") && pending.has(message.id)) {
    const operation = pending.get(message.id);
    pending.delete(message.id);
    clearTimeout(operation.timer);
    if (message.error)
      operation.reject(new Error(JSON.stringify(message.error)));
    else operation.resolve(message.result);
    return;
  }
  if (Object.hasOwn(message, "id") && message.method) {
    handleServerRequest(message);
    return;
  }
  if (
    message.method === "item/started" ||
    message.method === "item/completed"
  ) {
    const itemType = message.params?.item?.type;
    if (typeof itemType === "string") observations.itemTypes.add(itemType);
  }
  if (message.method === "turn/completed") {
    const turnId = message.params?.turn?.id;
    const index = completedTurns.findIndex((entry) => entry.turnId === turnId);
    if (index >= 0) {
      const [completion] = completedTurns.splice(index, 1);
      completion.resolve(message.params.turn);
    } else completedTurnResults.set(turnId, message.params.turn);
  }
}

function awaitTurn(turnId) {
  if (completedTurnResults.has(turnId)) {
    const result = completedTurnResults.get(turnId);
    completedTurnResults.delete(turnId);
    return Promise.resolve(result);
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      const index = completedTurns.findIndex(
        (entry) => entry.turnId === turnId,
      );
      if (index >= 0) completedTurns.splice(index, 1);
      reject(new Error(`app-server turn timed out: ${turnId}`));
    }, operationTimeoutMs);
    timer.unref();
    completedTurns.push({
      reject,
      resolve: (result) => {
        clearTimeout(timer);
        resolve(result);
      },
      turnId,
    });
  });
}

function rejectWaiters(error) {
  for (const operation of pending.values()) {
    clearTimeout(operation.timer);
    operation.reject(error);
  }
  pending.clear();
  for (const operation of completedTurns.splice(0)) operation.reject(error);
}

child.stdin.on("error", (error) => rejectWaiters(error));

const stdout = readline.createInterface({ input: child.stdout });
stdout.on("line", (line) => {
  try {
    handleMessage(JSON.parse(line));
  } catch (error) {
    rejectWaiters(new Error(`invalid app-server JSON: ${error.message}`));
  }
});

let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", (chunk) => {
  stderr += chunk;
});

const childClosed = new Promise((resolve) => {
  child.once("close", (code, signal) => {
    rejectWaiters(
      new Error(
        `app-server closed while work was pending: ${JSON.stringify({ code, signal })}`,
      ),
    );
    resolve({ code, signal });
  });
});
child.once("error", (error) => rejectWaiters(error));

async function startTurn(threadId, text) {
  const result = await request("turn/start", {
    input: [{ text, type: "text" }],
    threadId,
  });
  const completion = await awaitTurn(result.turn.id);
  if (completion.status !== "completed") {
    throw new Error(`authority probe turn ended with ${completion.status}`);
  }
}

async function run() {
  const initialize = await request("initialize", {
    capabilities: { experimentalApi: true },
    clientInfo: {
      name: "tiber-authority-probe",
      title: "Tiber authority probe",
      version: "0.1.0",
    },
  });
  const expectedCodexHome = path.resolve(codexHome);
  if (initialize.codexHome !== expectedCodexHome) {
    throw new Error(
      `app-server reported unexpected Codex home: ${initialize.codexHome}`,
    );
  }
  if (
    initialize.platformFamily !== "unix" ||
    initialize.platformOs !== "linux"
  ) {
    throw new Error(
      `app-server reported unsupported platform: ${initialize.platformFamily}/${initialize.platformOs}`,
    );
  }
  if (process.arch !== "x64") {
    throw new Error(
      `authority probe requires x86_64, received ${process.arch}`,
    );
  }
  const versionMatch = initialize.userAgent.match(/\b(\d+\.\d+\.\d+)\b/u);
  const codexVersion = versionMatch?.[1];
  const expectedCodexVersion =
    process.env.TIBER_EXPECTED_CODEX_VERSION ?? "0.147.0";
  if (codexVersion !== expectedCodexVersion) {
    throw new Error(
      `app-server version mismatch: expected ${expectedCodexVersion}, received ${codexVersion ?? "unknown"}`,
    );
  }
  send({ method: "initialized" });

  const profiles = await request("permissionProfile/list", {
    cwd: path.resolve(workspace),
  });
  const profile = profiles.data.find((entry) => entry.id === "tiber-inference");
  if (!profile?.allowed) {
    throw new Error("tiber-inference permission profile is unavailable");
  }

  const started = await request("thread/start", {
    approvalPolicy: "never",
    approvalsReviewer: "user",
    cwd: path.resolve(workspace),
    dynamicTools: [
      {
        description:
          "Requests a Tiber-owned effect. The client receives the request and decides whether to execute it.",
        inputSchema: {
          additionalProperties: false,
          properties: { action: { type: "string" } },
          required: ["action"],
          type: "object",
        },
        name: "tiber_authority_probe",
        type: "function",
      },
    ],
    ephemeral: true,
    permissions: "tiber-inference",
  });

  const commandControl = await request("command/exec", {
    command: [process.execPath, "-e", "process.exit(0)"],
    cwd: path.resolve(workspace),
    permissionProfile: "tiber-inference",
    timeoutMs: operationTimeoutMs,
  });
  if (commandControl?.exitCode !== 0) {
    throw new Error(
      `command/exec positive control did not execute the probe runtime: ${JSON.stringify(commandControl)}`,
    );
  }
  const commandResult = await request("command/exec", {
    command: [
      process.execPath,
      "-e",
      'require("node:fs").writeFileSync(process.argv.at(-1), "unauthorized")',
      sentinelPath,
    ],
    cwd: path.resolve(workspace),
    permissionProfile: "tiber-inference",
    timeoutMs: operationTimeoutMs,
  });
  if (!Number.isSafeInteger(commandResult?.exitCode)) {
    throw new Error("command/exec returned no recognized terminal result");
  }
  if (commandResult.exitCode === 0) {
    throw new Error("command/exec unexpectedly completed the denied write");
  }
  const deniedCommand = {
    exitCode: commandResult.exitCode,
    rejected: true,
    transportError: false,
  };

  await startTurn(
    started.thread.id,
    'Call tiber_authority_probe exactly once with {"action":"sentinel"}. Do not use any other tool. Then state whether the client executed the requested effect.',
  );

  const evidence = {
    activePermissionProfile: started.activePermissionProfile,
    approvalPolicy: started.approvalPolicy,
    approvalsReviewer: started.approvalsReviewer,
    codexHome: initialize.codexHome,
    codexVersion,
    codexRuntimeExecutable: runtimeExecutable,
    commandSandbox: {
      attempted: true,
      control: { exitCode: commandControl.exitCode },
      result: deniedCommand,
    },
    dynamicTool: {
      calls: observations.dynamicToolCalls,
      effectExecutedByProbe: false,
    },
    itemTypes: [...observations.itemTypes].sort(),
    permissionProfiles: profiles.data,
    rejectedEscalations: {
      approvals: observations.approvalRequestsDeclined,
      permissions: observations.permissionRequestsDeclined,
    },
    sandbox: started.sandbox,
    unauthorizedMutation: {
      artifactExists: fs.existsSync(sentinelPath),
    },
  };

  if (
    observations.dynamicToolCalls.length !== 1 ||
    observations.dynamicToolCalls[0].tool !== "tiber_authority_probe" ||
    commandControl.exitCode !== 0 ||
    !deniedCommand.rejected ||
    deniedCommand.transportError ||
    fs.existsSync(sentinelPath) ||
    started.activePermissionProfile?.id !== "tiber-inference" ||
    started.approvalPolicy !== "never" ||
    started.sandbox?.type !== "readOnly" ||
    started.sandbox?.networkAccess !== false
  ) {
    throw new Error(`authority contract failed: ${JSON.stringify(evidence)}`);
  }

  process.stdout.write(`${JSON.stringify(evidence, null, 2)}\n`);
}

function timeoutPromise(milliseconds, message) {
  return new Promise((_, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), milliseconds);
    timer.unref();
  });
}

async function terminateChild() {
  if (child.exitCode !== null || child.signalCode !== null) return childClosed;
  child.kill("SIGTERM");
  try {
    return await Promise.race([
      childClosed,
      timeoutPromise(2_000, "app-server ignored SIGTERM"),
    ]);
  } catch {
    child.kill("SIGKILL");
    return Promise.race([
      childClosed,
      timeoutPromise(2_000, "app-server ignored SIGKILL"),
    ]);
  }
}

try {
  await run();
  child.stdin.end();
  const closed = await Promise.race([
    childClosed,
    timeoutPromise(5_000, "app-server close timed out"),
  ]);
  if (closed.code !== 0) {
    throw new Error(
      `app-server exited with ${JSON.stringify(closed)}: ${stderr}`,
    );
  }
} catch (error) {
  try {
    await terminateChild();
  } catch (cleanupError) {
    stderr += `\ncleanup failed: ${cleanupError.message}`;
  }
  console.error(`app_server_authority_probe_failed: ${error.message}`);
  if (stderr) console.error(stderr.trim());
  process.exit(1);
}
