#!/usr/bin/env node

import { createHash } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import readline from "node:readline";

function websocketFrame(payload, masked) {
  const bytes = Buffer.from(payload);
  const extended = bytes.length >= 126;
  const header = Buffer.alloc(extended ? 4 : 2);
  header[0] = 0x81;
  header[1] = (masked ? 0x80 : 0) | (extended ? 126 : bytes.length);
  if (extended) header.writeUInt16BE(bytes.length, 2);
  if (!masked) return Buffer.concat([header, bytes]);
  const mask = Buffer.from([0x11, 0x22, 0x33, 0x44]);
  const encoded = Buffer.from(bytes);
  for (let index = 0; index < encoded.length; index += 1) {
    encoded[index] ^= mask[index % mask.length];
  }
  return Buffer.concat([header, mask, encoded]);
}

function websocketReader(socket, initial = Buffer.alloc(0)) {
  let buffered = initial;
  const waiting = [];
  const frames = [];
  function drain() {
    while (buffered.length >= 2) {
      const masked = (buffered[1] & 0x80) !== 0;
      let length = buffered[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buffered.length < 4) return;
        length = buffered.readUInt16BE(2);
        offset = 4;
      }
      const maskLength = masked ? 4 : 0;
      if (buffered.length < offset + maskLength + length) return;
      const mask = masked ? buffered.subarray(offset, offset + 4) : null;
      offset += maskLength;
      const payload = Buffer.from(buffered.subarray(offset, offset + length));
      if (mask) {
        for (let index = 0; index < payload.length; index += 1) {
          payload[index] ^= mask[index % mask.length];
        }
      }
      buffered = buffered.subarray(offset + length);
      const waiter = waiting.shift();
      if (waiter) waiter.resolve(payload.toString("utf8"));
      else frames.push(payload.toString("utf8"));
    }
  }
  socket.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    drain();
  });
  socket.on("error", (error) => {
    while (waiting.length > 0) waiting.shift().reject(error);
  });
  socket.on("end", () => {
    while (waiting.length > 0) waiting.shift().reject(new Error("websocket closed"));
  });
  drain();
  return () => {
    const frame = frames.shift();
    if (frame !== undefined) return Promise.resolve(frame);
    return new Promise((resolve, reject) => waiting.push({ resolve, reject }));
  };
}

function readUpgrade(socket) {
  return new Promise((resolve, reject) => {
    let buffered = Buffer.alloc(0);
    function onData(chunk) {
      buffered = Buffer.concat([buffered, chunk]);
      const boundary = buffered.indexOf("\r\n\r\n");
      if (boundary < 0) return;
      socket.off("data", onData);
      resolve({
        head: buffered.subarray(0, boundary + 4).toString("utf8"),
        remainder: buffered.subarray(boundary + 4),
      });
    }
    socket.on("data", onData);
    socket.once("error", reject);
  });
}

async function connectWebsocket(path) {
  const socket = net.createConnection(path);
  await new Promise((resolve, reject) => {
    socket.once("connect", resolve);
    socket.once("error", reject);
  });
  const key = Buffer.from("tiber-native-fixture").toString("base64");
  socket.write(
    `GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
  );
  const upgrade = await readUpgrade(socket);
  if (!upgrade.head.startsWith("HTTP/1.1 101")) throw new Error("websocket upgrade failed");
  return { next: websocketReader(socket, upgrade.remainder), socket };
}

async function acceptWebsocket(socket) {
  const upgrade = await readUpgrade(socket);
  const key = upgrade.head.match(/Sec-WebSocket-Key:\s*([^\r\n]+)/i)?.[1];
  if (!key) throw new Error("websocket key missing");
  const accept = createHash("sha1")
    .update(`${key.trim()}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  return websocketReader(socket, upgrade.remainder);
}

const fixtureMode =
  process.env.TIBER_FIXTURE_MODE ??
  process.argv.find((argument) => argument.startsWith("--mode="))?.slice(7) ??
  "success";
if (process.argv.includes("--version")) {
  console.log(
    fixtureMode === "wrong-version"
      ? "codex-cli 0.148.0"
      : "codex-cli 0.147.0",
  );
  process.exit(0);
}
const remoteIndex = process.argv.indexOf("--remote");
if (remoteIndex >= 0 && process.env.TIBER_FIXTURE_CODEX_TUI_INVOCATION) {
  fs.writeFileSync(
    process.env.TIBER_FIXTURE_CODEX_TUI_INVOCATION,
    JSON.stringify(process.argv.slice(2)),
  );
  if (!process.env.TIBER_FIXTURE_NATIVE_TURN_RESULT) process.exit(0);
  const endpoint = process.argv.at(remoteIndex + 1);
  if (!endpoint?.startsWith("unix://")) process.exit(2);
  const peer = await connectWebsocket(endpoint.slice("unix://".length));
  peer.socket.write(
    websocketFrame(
      JSON.stringify({
        id: 17,
        method: "turn/start",
        params: {
          input: [{ text: "native fixture prompt", type: "text" }],
          threadId: "thread-1",
        },
      }),
      true,
    ),
  );
  if (fixtureMode === "native-process-cancel") {
    const started = process.env.TIBER_FIXTURE_NATIVE_PROCESS_STARTED;
    if (!started) process.exit(2);
    while (!fs.existsSync(started)) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    peer.socket.write(Buffer.from([0x88, 0x80, 0x11, 0x22, 0x33, 0x44]));
    await new Promise((resolve) => setTimeout(resolve, 20));
    peer.socket.end();
    process.exit(0);
  }
  const response = JSON.parse(await peer.next());
  const completed = JSON.parse(await peer.next());
  let ownerDecision;
  if (fixtureMode === "native-repository") {
    if (process.env.TIBER_FIXTURE_NATIVE_OWNER_RELEASE) {
      while (!fs.existsSync(process.env.TIBER_FIXTURE_NATIVE_OWNER_RELEASE)) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
    peer.socket.write(
      websocketFrame(
        JSON.stringify({
          id: 18,
          method: "turn/start",
          params: {
            input: [{ text: "approve", type: "text" }],
            threadId: "thread-1",
          },
        }),
        true,
      ),
    );
    ownerDecision = {
      response: JSON.parse(await peer.next()),
      completed: JSON.parse(await peer.next()),
    };
  }
  fs.writeFileSync(
    process.env.TIBER_FIXTURE_NATIVE_TURN_RESULT,
    JSON.stringify({ completed, ownerDecision, response }),
  );
  peer.socket.write(Buffer.from([0x88, 0x80, 0x11, 0x22, 0x33, 0x44]));
  await new Promise((resolve) => setTimeout(resolve, 20));
  peer.socket.end();
  process.exit(0);
}
const listenIndex = process.argv.indexOf("--listen");
if (process.argv.includes("app-server") && listenIndex >= 0) {
  if (fixtureMode === "native-backend-exit") process.exit(3);
  const endpoint = process.argv.at(listenIndex + 1);
  if (!endpoint?.startsWith("unix://")) process.exit(2);
  const server = net.createServer(async (socket) => {
    if (!process.env.TIBER_FIXTURE_NATIVE_TURN_RESULT) return;
    const next = await acceptWebsocket(socket);
    if (fixtureMode === "native-real-tui") {
      const nativeThreadId = "019c0132-1111-7111-8111-111111111111";
      const nativeTurnId = "019c0132-2222-7222-8222-222222222222";
      while (true) {
        const message = JSON.parse(await next());
        if (process.env.TIBER_FIXTURE_NATIVE_BACKEND_MESSAGES) {
          fs.appendFileSync(
            process.env.TIBER_FIXTURE_NATIVE_BACKEND_MESSAGES,
            `${JSON.stringify(message)}\n`,
          );
        }
        const reply = (value) => {
          socket.write(websocketFrame(JSON.stringify(value), false));
        };
        if (message.method === "initialize") {
          reply({
            id: message.id,
            result: {
              codexHome: process.env.CODEX_HOME,
              platformFamily: "unix",
              platformOs: "linux",
              userAgent: "fixture/0.147.0",
            },
          });
        } else if (message.method === "account/read") {
          reply({
            id: message.id,
            result: {
              account: {
                email: "fixture@example.invalid",
                planType: "plus",
                type: "chatgpt",
              },
              requiresOpenaiAuth: true,
            },
          });
        } else if (message.method === "model/list") {
          reply({
            id: message.id,
            result: {
              data: [
                {
                  additionalSpeedTiers: [],
                  availabilityNux: null,
                  defaultReasoningEffort: "medium",
                  defaultServiceTier: null,
                  description: "Fixture model for the reviewed native TUI boundary.",
                  displayName: "Fixture Model",
                  hidden: false,
                  id: "fixture-model",
                  inputModalities: ["text"],
                  isDefault: true,
                  model: "fixture-model",
                  modelSpecialty: null,
                  multiAgentVersion: null,
                  serviceTiers: [],
                  supportedReasoningEfforts: [
                    { description: "Fixture reasoning", reasoningEffort: "medium" },
                  ],
                  supportsPersonality: false,
                  upgrade: null,
                  upgradeInfo: null,
                },
              ],
              nextCursor: null,
            },
          });
        } else if (message.method === "configRequirements/read") {
          reply({ id: message.id, result: { requirements: null } });
        } else if (message.method === "hooks/list") {
          reply({
            id: message.id,
            result: {
              data: (message.params?.cwds ?? []).map((cwd) => ({
                cwd,
                errors: [],
                hooks: [],
                warnings: [],
              })),
            },
          });
        } else if (message.method === "skills/list") {
          reply({
            id: message.id,
            result: {
              data: (message.params?.cwds ?? []).map((cwd) => ({
                cwd,
                errors: [],
                skills: [],
              })),
            },
          });
        } else if (message.method === "plugin/list") {
          reply({
            id: message.id,
            result: {
              featuredPluginIds: [],
              marketplaceLoadErrors: [],
              marketplaces: [],
            },
          });
        } else if (message.method === "account/rateLimits/read") {
          reply({
            id: message.id,
            result: {
              rateLimitResetCredits: null,
              rateLimits: null,
              rateLimitsByLimitId: {},
            },
          });
        } else if (message.method === "thread/start") {
          const now = Math.floor(Date.now() / 1000);
          reply({
            id: message.id,
            result: {
              activePermissionProfile: null,
              approvalPolicy: "never",
              approvalsReviewer: "user",
              cwd: process.cwd(),
              instructionSources: [],
              model: "fixture-model",
              modelProvider: "fixture",
              multiAgentMode: "explicitRequestOnly",
              reasoningEffort: "medium",
              runtimeWorkspaceRoots: [process.cwd()],
              sandbox: { networkAccess: false, type: "readOnly" },
              serviceTier: "default",
              thread: {
                agentNickname: null,
                agentRole: null,
                canAcceptDirectInput: true,
                cliVersion: "0.147.0",
                createdAt: now,
                cwd: process.cwd(),
                ephemeral: false,
                extra: null,
                forkedFromId: null,
                gitInfo: null,
                historyMode: "paginated",
                id: nativeThreadId,
                modelProvider: "fixture",
                name: null,
                parentThreadId: null,
                path: null,
                preview: "",
                recencyAt: now,
                section: null,
                sectionEnteredAt: null,
                sessionId: nativeThreadId,
                source: "vscode",
                status: { type: "idle" },
                threadSource: "user",
                turns: [],
                updatedAt: now,
              },
            },
          });
        } else if (message.method === "turn/start") {
          const prompt = message.params?.input?.[0]?.text ?? "missing";
          if (process.env.TIBER_FIXTURE_NATIVE_BACKEND_TURNS) {
            fs.appendFileSync(
              process.env.TIBER_FIXTURE_NATIVE_BACKEND_TURNS,
              `${prompt}\n`,
            );
          }
          reply({
            id: message.id,
            result: { turn: { id: nativeTurnId, items: [], status: "inProgress" } },
          });
          reply({
            method: "item/agentMessage/delta",
            params: {
              delta: "native real TUI answer",
              itemId: "message-1",
              threadId: nativeThreadId,
              turnId: nativeTurnId,
            },
          });
          reply({
            method: "turn/completed",
            params: {
              threadId: nativeThreadId,
              turn: {
                id: nativeTurnId,
                items: [
                  { id: "message-1", text: "native real TUI answer", type: "agentMessage" },
                ],
                status: "completed",
              },
            },
          });
        } else if (message.id !== undefined) {
          reply({
            error: { code: -32601, message: `unsupported fixture method: ${message.method}` },
            id: message.id,
          });
        }
      }
    }
    const request = JSON.parse(await next());
    if (process.env.TIBER_FIXTURE_NATIVE_BACKEND_TURNS) {
      fs.appendFileSync(
        process.env.TIBER_FIXTURE_NATIVE_BACKEND_TURNS,
        `${request.params?.input?.[0]?.text ?? "missing"}\n`,
      );
    }
    socket.write(
      websocketFrame(JSON.stringify({ id: 17, result: { turn: { id: "turn-1" } } }), false),
    );
    if (fixtureMode === "native-process" || fixtureMode === "native-process-cancel") {
      socket.write(
        websocketFrame(
          JSON.stringify({
            id: "native-process-request",
            method: "item/tool/call",
            params: {
              arguments: {
                command: "focused-test",
                operation: "run_configured_command",
              },
              callId: "native-process-call",
              threadId: "thread-1",
              tool: "tiber_effect",
              turnId: "turn-1",
            },
          }),
          false,
        ),
      );
      const effectResult = JSON.parse(await next());
      if (process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT) {
        fs.writeFileSync(
          process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT,
          JSON.stringify(effectResult),
        );
      }
    }
    if (fixtureMode === "native-task") {
      socket.write(
        websocketFrame(
          JSON.stringify({
            id: "native-task-request",
            method: "item/tool/call",
            params: {
              arguments: { arguments: ["list"] },
              callId: "native-task-call",
              threadId: "thread-1",
              tool: "tiber_tasks",
              turnId: "turn-1",
            },
          }),
          false,
        ),
      );
      const effectResult = JSON.parse(await next());
      if (process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT) {
        fs.writeFileSync(
          process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT,
          JSON.stringify(effectResult),
        );
      }
    }
    if (
      fixtureMode === "native-repository" ||
      fixtureMode === "native-repository-crash"
    ) {
      socket.write(
        websocketFrame(
          JSON.stringify({
            id: "native-repository-read-request",
            method: "item/tool/call",
            params: {
              arguments: { operation: "read_file", path: "README.md" },
              callId: "native-repository-read-call",
              threadId: "thread-1",
              tool: "tiber_repository_read",
              turnId: "turn-1",
            },
          }),
          false,
        ),
      );
      const readResult = JSON.parse(await next());
      if (process.env.TIBER_FIXTURE_NATIVE_READ_RESULT) {
        fs.writeFileSync(
          process.env.TIBER_FIXTURE_NATIVE_READ_RESULT,
          JSON.stringify(readResult),
        );
      }
      const observed = JSON.parse(readResult.result.contentItems[0].text).content;
      socket.write(
        websocketFrame(
          JSON.stringify({
            id: "native-repository-request",
            method: "item/tool/call",
            params: {
              arguments: {
                action: "write",
                expected: observed,
                path: "README.md",
                replacement: "after\n",
              },
              callId: "native-repository-call",
              threadId: "thread-1",
              tool: "tiber_repository_proposal",
              turnId: "turn-1",
            },
          }),
          false,
        ),
      );
      const effectResult = JSON.parse(await next());
      if (process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT) {
        fs.writeFileSync(
          process.env.TIBER_FIXTURE_NATIVE_EFFECT_RESULT,
          JSON.stringify(effectResult),
        );
      }
    }
    socket.write(
      websocketFrame(
        JSON.stringify({
          method: "turn/completed",
          params: {
            threadId: "thread-1",
            turn: {
              id: "turn-1",
              items:
                process.env.TIBER_FIXTURE_NATIVE_TURN_STATUS === "failed"
                  ? []
                  : [{ id: "message-1", text: "native fixture answer", type: "agentMessage" }],
              status: process.env.TIBER_FIXTURE_NATIVE_TURN_STATUS ?? "completed",
            },
          },
        }),
        false,
      ),
    );
    if (fixtureMode === "native-repository") {
      const ownerRequest = JSON.parse(await next());
      socket.write(
        websocketFrame(
          JSON.stringify({ id: ownerRequest.id, result: { turn: { id: "turn-2" } } }),
          false,
        ),
      );
      socket.write(
        websocketFrame(
          JSON.stringify({
            id: "native-repository-verification-request",
            method: "item/tool/call",
            params: {
              arguments: {
                command: "focused-test",
                operation: "run_configured_command",
              },
              callId: "native-repository-verification-call",
              threadId: "thread-1",
              tool: "tiber_effect",
              turnId: "turn-2",
            },
          }),
          false,
        ),
      );
      const verificationResult = JSON.parse(await next());
      if (process.env.TIBER_FIXTURE_NATIVE_VERIFICATION_RESULT) {
        fs.writeFileSync(
          process.env.TIBER_FIXTURE_NATIVE_VERIFICATION_RESULT,
          JSON.stringify(verificationResult),
        );
      }
      socket.write(
        websocketFrame(
          JSON.stringify({
            method: "turn/completed",
            params: {
              threadId: "thread-1",
              turn: {
                id: "turn-2",
                items: [
                  { id: "message-2", text: "approved native repository change", type: "agentMessage" },
                ],
                status: "completed",
              },
            },
          }),
          false,
        ),
      );
    }
  });
  server.listen(endpoint.slice("unix://".length));
  setInterval(() => {}, 1000);
}
const fixtureAuthState = process.env.TIBER_FIXTURE_AUTH_STATE;
const fixtureStdinCanary = "fixture-stdin-token";
const processPolicyResults = [];
const apiKeyLogin =
  process.argv.includes("login") && process.argv.includes("--with-api-key");
const effectiveProfileMismatch = [
  "wrong-profile",
  "wrong-approval-policy",
  "wrong-sandbox-type",
  "network-enabled",
].includes(fixtureMode);

function fixtureAuthValue(name) {
  if (!fixtureAuthState || !fs.existsSync(fixtureAuthState)) return undefined;
  const prefix = `${name}=`;
  return fs
    .readFileSync(fixtureAuthState, "utf8")
    .split("\n")
    .find((line) => line.startsWith(prefix))
    ?.slice(prefix.length);
}

function recordAccountRead() {
  if (fixtureAuthState && fs.existsSync(fixtureAuthState)) {
    fs.appendFileSync(
      fixtureAuthState,
      [
        "account_read=true",
        `app_server_anthropic_api_key_present=${String(
          Object.hasOwn(process.env, "ANTHROPIC_API_KEY"),
        )}`,
        `app_server_openai_api_key_present=${String(
          Object.hasOwn(process.env, "OPENAI_API_KEY"),
        )}`,
        "",
      ].join("\n"),
    );
  }
}

function effectiveThreadStartResult(threadId) {
  return {
    activePermissionProfile: {
      extends: null,
      id:
        fixtureMode === "wrong-profile" ? "fixture-profile" : "tiber-inference",
    },
    approvalPolicy:
      fixtureMode === "wrong-approval-policy" ? "on-request" : "never",
    approvalsReviewer: "user",
    sandbox: {
      networkAccess: fixtureMode === "network-enabled",
      type:
        fixtureMode === "wrong-sandbox-type" ? "workspaceWrite" : "readOnly",
    },
    thread: { id: threadId },
  };
}

function runApiKeyLogin() {
  let stdin = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    stdin += chunk;
  });
  process.stdin.on("end", () => {
    const stdinSha256 = createHash("sha256").update(stdin).digest("hex");
    if (
      process.env.TIBER_FIXTURE_EXPECTED_STDIN_SHA256 &&
      stdinSha256 !== process.env.TIBER_FIXTURE_EXPECTED_STDIN_SHA256
    ) {
      process.stderr.write("fixture-api-key-login-stdin-mismatch\n");
      process.exitCode = 17;
      return;
    }
    if (fixtureMode === "api-key-login-failure") {
      process.stderr.write(`fixture-api-key-login-failure: ${stdin}`);
      process.exitCode = 17;
      return;
    }
    if (!fixtureAuthState) {
      process.stderr.write("fixture-api-key-login-state-unavailable\n");
      process.exitCode = 17;
      return;
    }
    const accountType =
      fixtureMode === "api-key-login-without-account" ? "signedOut" : "apiKey";
    fs.writeFileSync(
      fixtureAuthState,
      [
        `account_type=${accountType}`,
        `anthropic_api_key_present=${String(
          Object.hasOwn(process.env, "ANTHROPIC_API_KEY"),
        )}`,
        `argv_contains_fixture_input=${String(
          process.argv.some((argument) =>
            argument.includes(fixtureStdinCanary),
          ),
        )}`,
        `codex_home=${process.env.CODEX_HOME ?? "missing"}`,
        `environment_contains_fixture_input=${String(
          Object.values(process.env).some((value) =>
            value.includes(fixtureStdinCanary),
          ),
        )}`,
        `openai_api_key_present=${String(
          Object.hasOwn(process.env, "OPENAI_API_KEY"),
        )}`,
        `stdin_sha256=${stdinSha256}`,
        "",
      ].join("\n"),
    );
  });
}

function runAppServer() {
  let threadId = "thread-fixture";
  let nextTurn = 0;
  const completedToolRequests = new Set();
  let account =
    fixtureAuthValue("account_type") === "apiKey" ? { type: "apiKey" } : null;
  const input = readline.createInterface({ input: process.stdin });

  if (fixtureMode === "ignored-term") {
    process.on("SIGTERM", () => {});
  }

  function send(message) {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  }

  function completeTurn(turnId) {
    send({
      method: "turn/completed",
      params: {
        threadId,
        turn: { id: turnId, items: [], status: "completed" },
      },
    });
    if (process.env.TIBER_FIXTURE_TURN_COMPLETED_SENTINEL) {
      fs.writeFileSync(
        process.env.TIBER_FIXTURE_TURN_COMPLETED_SENTINEL,
        `${turnId}\n`,
      );
    }
  }

  input.on("line", async (line) => {
    if (fixtureMode === "silent" || fixtureMode === "ignored-term") return;
    if (fixtureMode === "early-close") process.exit(3);
    if (fixtureMode === "malformed") {
      process.stdout.write("not-json\n");
      return;
    }
    const message = JSON.parse(line);
    if (message.method === "initialize") {
      if (fixtureMode === "chatty") {
        const timer = setInterval(
          () => send({ method: "fixture/progress" }),
          25,
        );
        setTimeout(() => clearInterval(timer), 5_000);
        return;
      }
      send({
        id: message.id,
        result: {
          codexHome:
            fixtureMode === "wrong-home"
              ? "/unexpected"
              : process.env.CODEX_HOME,
          platformFamily: "unix",
          platformOs: "linux",
          userAgent:
            fixtureMode === "wrong-version"
              ? "fixture/0.148.0 compatibility/0.147.0"
              : "fixture/0.147.0",
        },
      });
      if (process.env.TIBER_FIXTURE_INITIALIZED_SENTINEL) {
        fs.writeFileSync(process.env.TIBER_FIXTURE_INITIALIZED_SENTINEL, "initialized\n");
      }
    } else if (message.method === "permissionProfile/list") {
      send({
        id: message.id,
        result: {
          data: [
            {
              allowed: true,
              description: "Read-only, offline inference for the Tiber harness",
              id: "tiber-inference",
            },
          ],
        },
      });
    } else if (message.method === "account/read") {
      recordAccountRead();
      send({ id: message.id, result: { account, requiresOpenaiAuth: true } });
    } else if (message.method === "account/login/start") {
      if (message.params.type !== "chatgpt") {
        send({
          error: { code: -32602, message: "unsupported fixture login type" },
          id: message.id,
        });
        return;
      }
      account = { type: "chatgpt" };
      send({
        id: message.id,
        result: {
          authUrl: "https://example.invalid/login",
          loginId: "login-fixture",
          type: "chatgpt",
        },
      });
      send({
        method: "account/login/completed",
        params:
          fixtureMode === "idless-login-failure"
            ? { error: "fixture login denied", success: false }
            : { loginId: "login-fixture", success: true },
      });
    } else if (message.method === "account/logout") {
      account = null;
      send({ id: message.id, result: {} });
    } else if (message.method === "thread/start") {
      if (
        fixtureMode === "repository-tool-contract" ||
        fixtureMode === "configured-command-tool-contract"
      ) {
        const tools = message.params.dynamicTools ?? [];
        const names = tools.map((tool) => tool.name);
        const effect = tools.find((tool) => tool.name === "tiber_effect");
        const repositoryProposal = tools.find(
          (tool) => tool.name === "tiber_repository_proposal",
        );
        const serializedTools = JSON.stringify(tools);
        const baseCommandSchema = {
          maxLength: 128,
          minLength: 1,
          type: "string",
        };
        const commandSchema =
          fixtureMode === "configured-command-tool-contract"
            ? { enum: ["focused-test", "format"], ...baseCommandSchema }
            : baseCommandSchema;
        const expectedEffectSchema = {
          additionalProperties: false,
          properties: {
            command: commandSchema,
            operation: { const: "run_configured_command", type: "string" },
          },
          required: ["operation", "command"],
          type: "object",
        };
        const expectedRepositoryProposalSchema = {
          additionalProperties: false,
          properties: {
            action: { const: "write", type: "string" },
            expected: { type: "string" },
            path: { type: "string" },
            replacement: { type: "string" },
          },
          required: ["action", "expected", "path", "replacement"],
          type: "object",
        };
        if (
          !names.includes("tiber_effect") ||
          !names.includes("tiber_repository_proposal") ||
          JSON.stringify(effect?.inputSchema) !==
            JSON.stringify(expectedEffectSchema) ||
          JSON.stringify(repositoryProposal?.inputSchema) !==
            JSON.stringify(expectedRepositoryProposalSchema) ||
          serializedTools.includes("must-not-cross-tool-schema")
        ) {
          send({
            error: {
              code: -32602,
              message: `invalid closed Tiber tool declaration: ${JSON.stringify(tools)}`,
            },
            id: message.id,
          });
          return;
        }
      }
      if (fixtureMode === "oversized-line") {
        process.stdout.write("x".repeat(40 * 1024));
        return;
      }
      if (fixtureMode === "id-collision") {
        send({
          id: message.id,
          method: "item/commandExecution/requestApproval",
          params: { threadId, turnId: "turn-1" },
        });
        return;
      }
      if (fixtureMode === "delayed-start") {
        await new Promise((resolve) => setTimeout(resolve, 500));
      }
      if (fixtureMode === "hold-thread-start") return;
      send({
        id: message.id,
        result: effectiveThreadStartResult(threadId),
      });
    } else if (message.method === "command/exec") {
      const isControl = message.params.command.includes("process.exit(0)");
      if (fixtureMode === "control-failure" && isControl) {
        send({ id: message.id, result: { exitCode: 1 } });
        return;
      }
      if (fixtureMode === "command-timeout" && !isControl) return;
      if (fixtureMode === "command-malformed" && !isControl) {
        send({ id: message.id, result: {} });
        return;
      }
      if (fixtureMode === "command-error" && !isControl) {
        send({
          error: { code: -32601, message: "command/exec unavailable" },
          id: message.id,
        });
        return;
      }
      send({
        id: message.id,
        result: {
          exitCode: isControl ? 0 : 1,
          stderr: isControl ? "" : "write denied by fixture sandbox",
          stdout: "",
        },
      });
    } else if (message.method === "turn/start") {
      if (process.env.TIBER_FIXTURE_INVOCATIONS) {
        fs.appendFileSync(process.env.TIBER_FIXTURE_INVOCATIONS, "turn/start\n");
      }
      if (effectiveProfileMismatch) {
        send({
          error: {
            code: -32600,
            message:
              "fixture effective-profile mismatch must fail before turn/start",
          },
          id: message.id,
        });
        return;
      }
      nextTurn += 1;
      const turnId = `turn-${nextTurn}`;
      send({ id: message.id, result: { turn: { id: turnId } } });
      send({
        method: "item/agentMessage/delta",
        params: {
          delta: "foreign text",
          itemId: "foreign-assistant",
          threadId: "foreign-thread",
          turnId: "foreign-turn",
        },
      });
      if (fixtureMode !== "success") {
        send({
          id: "foreign-dynamic",
          method: "item/tool/call",
          params: {
            arguments: { foreign: true },
            callId: "foreign-call",
            threadId: "foreign-thread",
            tool: "tiber_effect",
            turnId: "foreign-turn",
          },
        });
      }
      const assistantDeltas =
        fixtureMode === "oversized-assistant"
          ? Array.from({ length: 257 }, () => "x".repeat(1024))
          : fixtureMode === "control-assistant"
          ? ["PROVIDER_BEFORE\u001b[31mPROVIDER_AFTER"]
          : fixtureMode === "split-stream"
          ? ["hello ", "from Tiber"]
          : fixtureMode.startsWith("repository-edit")
          ? ["I inspected README.md and propose changing before to after."]
          : fixtureMode === "process-fix"
          ? ["I will run the configured focused test before changing the repository."]
          : ["hello from Tiber"];
      if (fixtureMode === "delayed-stream") {
        await new Promise((resolve) => setTimeout(resolve, 75));
      }
      for (const delta of assistantDeltas) {
        send({
          method: "item/agentMessage/delta",
          params: {
            delta,
            itemId: `assistant-${nextTurn}`,
            threadId,
            turnId,
          },
        });
      }
      if (fixtureMode === "control-assistant") {
        await new Promise((resolve) => setTimeout(resolve, 200));
      }
      if (fixtureMode === "oversized-assistant" && process.env.TIBER_FIXTURE_OVERSIZED_SENTINEL) {
        fs.writeFileSync(process.env.TIBER_FIXTURE_OVERSIZED_SENTINEL, "oversized\n");
      }
      send({
        method: "item/started",
        params: {
          item: { id: `user-${nextTurn}`, type: "userMessage" },
          threadId,
          turnId,
        },
      });
      if (nextTurn === 1) {
        const processPolicyArguments = {
          "process-policy-shell": {
            command: "known",
            operation: "run_configured_command",
            shell: "touch /host/ambient-shell-secret",
          },
          "process-policy-cwd": {
            command: "known",
            cwd: "/host/ambient-cwd-secret",
            operation: "run_configured_command",
          },
          "process-policy-env": {
            command: "known",
            environment: { AMBIENT_PROCESS_SECRET: "ambient-env-secret" },
            operation: "run_configured_command",
          },
          "process-policy-network": {
            command: "known",
            network: true,
            operation: "run_configured_command",
          },
          "process-policy-executable": {
            command: "known",
            executable: "/host/ambient-executable-secret",
            operation: "run_configured_command",
          },
          "process-policy-malformed": {
            command: ["known"],
            operation: "run_configured_command",
          },
          "process-policy-unknown": {
            command: "ambient-unknown-command-secret",
            operation: "run_configured_command",
          },
        };
        send({
          id: "dynamic-fixture",
          method: "item/tool/call",
          params: {
            arguments:
              fixtureMode.startsWith("repository-edit")
                ? {
                    action:
                      fixtureMode === "repository-edit-non-write" ? "read" : "write",
                    expected: "before\n",
                    path: "README.md",
                    replacement: "after\n",
                  }
                : processPolicyArguments[fixtureMode] ??
                  (fixtureMode.startsWith("process-")
                ? {
                    command:
                      fixtureMode === "process-timeout"
                        ? "timeout"
                        : fixtureMode === "process-cancel"
                        ? "cancel"
                        : fixtureMode.startsWith("process-recovery")
                        ? "recovery"
                        : fixtureMode === "process-output-limit"
                        ? "output-limit"
                        : fixtureMode === "process-adapter-config"
                        ? "config-failure"
                        : "focused-test",
                    operation: "run_configured_command",
                  }
                : { action: "sentinel" }),
            callId: "call-fixture",
            namespace: null,
            threadId,
            tool:
              fixtureMode.startsWith("repository-edit")
                ? "tiber_repository_proposal"
                : fixtureMode.startsWith("process-")
                ? "tiber_effect"
                : "tiber_authority_probe",
            turnId,
          },
        });
        if (fixtureMode === "repository-edit-duplicate") {
          send({
            id: "dynamic-fixture-duplicate",
            method: "item/tool/call",
            params: {
              arguments: {
                action: "write",
                expected: "before\n",
                path: "README.md",
                replacement: "second\n",
              },
              callId: "call-fixture-duplicate",
              namespace: null,
              threadId,
              tool: "tiber_repository_proposal",
              turnId,
            },
          });
        }
        if (
          fixtureMode.startsWith("repository-edit") &&
          fixtureMode !== "repository-edit-delayed-completion"
        ) {
          completeTurn(turnId);
        }
        if (fixtureMode === "close-after-request") {
          setImmediate(() => process.exit(4));
        }
      } else {
        completeTurn(turnId);
      }
    } else if (
      message.id === "dynamic-fixture" ||
      message.id === "dynamic-fixture-duplicate" ||
      message.id === "dynamic-fixture-second" ||
      message.id === "dynamic-fixture-retry"
    ) {
      if (
        (fixtureMode.startsWith("process-policy-") ||
          fixtureMode === "process-timeout" ||
          fixtureMode === "process-cancel" ||
          fixtureMode === "process-success" ||
          fixtureMode === "process-output-limit" ||
          fixtureMode === "process-adapter-config" ||
          fixtureMode.startsWith("process-recovery")) &&
        message.id === "dynamic-fixture"
      ) {
        processPolicyResults.push(message.result);
        if (fixtureMode === "process-policy-unknown" && processPolicyResults.length === 1) {
          send({
            id: "dynamic-fixture",
            method: "item/tool/call",
            params: {
              arguments: {
                command: "ambient-unknown-command-secret",
                operation: "run_configured_command",
              },
              callId: "call-fixture-replay",
              namespace: null,
              threadId,
              tool: "tiber_effect",
              turnId: "turn-1",
            },
          });
        } else if (process.env.TIBER_FIXTURE_PROCESS_RESULT) {
          fs.writeFileSync(
            process.env.TIBER_FIXTURE_PROCESS_RESULT,
            JSON.stringify(processPolicyResults),
          );
          if (fixtureMode !== "process-recovery-hold") completeTurn("turn-1");
        }
      } else
      if (fixtureMode === "process-fix" && message.id === "dynamic-fixture") {
        const output = message.result?.contentItems?.[0]?.text;
        let parsedOutput;
        try {
          parsedOutput = JSON.parse(output);
        } catch {
          parsedOutput = null;
        }
        if (
          message.result?.success !== true ||
          typeof output !== "string" ||
          parsedOutput?.status?.exit_code !== 1 ||
          !parsedOutput?.stderr?.includes("focused failure")
        ) {
          process.exitCode = 1;
          return;
        }
        send({
          id: "dynamic-fixture-second",
          method: "item/tool/call",
          params: {
            arguments: {
              action: "write",
              expected: "#!/bin/sh\nprintf 'invoked\\n' >> /workspace/focused-test-invocations\ni=0\nwhile [ \"$i\" -lt 20000 ]; do printf '\"'; i=$((i + 1)); done\nprintf 'focused failure\\n' >&2\nexit 1\n",
              path: "focused-test",
              replacement: "#!/bin/sh\nprintf 'invoked\\n' >> /workspace/focused-test-invocations\nprintf 'focused success\\n'\n",
            },
            callId: "call-fixture-second",
            namespace: null,
            threadId,
            tool: "tiber_repository_proposal",
            turnId: "turn-1",
          },
        });
      } else if (
        fixtureMode === "process-fix" &&
        message.id === "dynamic-fixture-second"
      ) {
        if (message.result?.success !== true) {
          process.exitCode = 1;
          return;
        }
        send({
          method: "item/agentMessage/delta",
          params: {
            delta: "I observed the focused failure and propose repairing focused-test.",
            itemId: "assistant-process-proposal",
            threadId,
            turnId: "turn-1",
          },
        });
        send({
          id: "dynamic-fixture-retry",
          method: "item/tool/call",
          params: {
            arguments: {
              command: "focused-test",
              operation: "run_configured_command",
            },
            callId: "call-fixture-retry",
            namespace: null,
            threadId,
            tool: "tiber_effect",
            turnId: "turn-1",
          },
        });
      } else if (
        fixtureMode === "process-fix" &&
        message.id === "dynamic-fixture-retry"
      ) {
        const output = message.result?.contentItems?.[0]?.text;
        let parsedOutput;
        try {
          parsedOutput = JSON.parse(output);
        } catch {
          parsedOutput = null;
        }
        if (
          message.result?.success !== true ||
          parsedOutput?.status?.exit_code !== 0 ||
          !parsedOutput?.stdout?.includes("focused success")
        ) {
          process.exitCode = 1;
          return;
        }
        send({
          method: "item/agentMessage/delta",
          params: {
            delta: "The approved repair passed the exact configured focused-test command.",
            itemId: "assistant-process-final",
            threadId,
            turnId: "turn-1",
          },
        });
      } else if (
        fixtureMode.startsWith("repository-edit") &&
        typeof message.result?.success === "boolean"
      ) {
        // Repository proposal completion reflects the durable owner decision.
      } else if (message.result?.success !== false) {
        process.exitCode = 1;
      }
      if (completedToolRequests.has(message.id)) process.exitCode = 1;
      completedToolRequests.add(message.id);
      const expectedResponses = fixtureMode === "process-fix"
        ? 3
        : fixtureMode === "repository-edit-duplicate"
        ? 2
        : 1;
      if (
        completedToolRequests.size === expectedResponses &&
        fixtureMode !== "process-recovery-hold" &&
        (!fixtureMode.startsWith("repository-edit") ||
          fixtureMode === "repository-edit-delayed-completion")
      ) {
        if (fixtureMode === "repository-edit-delayed-completion") {
          while (!fs.existsSync(process.env.TIBER_FIXTURE_COMPLETION_RELEASE)) {
            await new Promise((resolve) => setTimeout(resolve, 5));
          }
        }
        completeTurn("turn-1");
      }
    } else if (
      fixtureMode === "id-collision" &&
      message.result?.decision === "decline"
    ) {
      send({
        id: message.id,
        result: effectiveThreadStartResult(threadId),
      });
    }
  });
}

if (apiKeyLogin) {
  runApiKeyLogin();
} else {
  runAppServer();
}
