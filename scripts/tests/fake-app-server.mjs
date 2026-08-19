#!/usr/bin/env node

import { createHash } from "node:crypto";
import fs from "node:fs";
import readline from "node:readline";

const fixtureMode =
  process.env.TIBER_FIXTURE_MODE ??
  process.argv.find((argument) => argument.startsWith("--mode="))?.slice(7) ??
  "success";
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
