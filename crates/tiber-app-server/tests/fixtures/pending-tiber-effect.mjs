import fs from "node:fs";
import readline from "node:readline";

const lines = readline.createInterface({ input: process.stdin });
const mode = process.argv[2] ?? "success";
const earlyResponseMarker = process.argv[3];
const completionAuthorization = process.argv[4];
let nextTurn = 0;

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function effectiveThreadStartResult(threadId) {
  return {
    activePermissionProfile: { id: "tiber-inference" },
    approvalPolicy: "never",
    sandbox: { networkAccess: false, type: "readOnly" },
    thread: { id: threadId },
  };
}

lines.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    send({
      id: message.id,
      result: {
        codexHome: process.env.CODEX_HOME,
        userAgent: "codex-cli/0.147.0",
      },
    });
  } else if (message.method === "initialized") {
    // Notification acknowledged by receiving no response.
  } else if (message.method === "thread/start") {
    send({ id: message.id, result: effectiveThreadStartResult("thread-effect") });
  } else if (message.method === "turn/start") {
    nextTurn += 1;
    const turnId = `turn-${nextTurn}`;
    send({ id: message.id, result: { turn: { id: turnId } } });
    if (mode === "repository-then-process") {
      send({
        id: "repository-proposal-request",
        method: "item/tool/call",
        params: {
          arguments: {
            action: "write",
            expected: "before\n",
            path: "README.md",
            replacement: "after\n",
          },
          callId: "repository-proposal-call",
          threadId: "thread-effect",
          tool: "tiber_repository_proposal",
          turnId,
        },
      });
      send({
        id: "conflicting-effect-request",
        method: "item/tool/call",
        params: {
          arguments: { operation: "run_configured_command", command: "focused-test" },
          callId: "conflicting-effect-call",
          threadId: "thread-effect",
          tool: "tiber_effect",
          turnId,
        },
      });
      return;
    }
    if (mode === "process-then-repository") {
      send({
        id: "primary-effect-request",
        method: "item/tool/call",
        params: {
          arguments: { operation: "run_configured_command", command: "focused-test" },
          callId: "primary-effect-call",
          threadId: "thread-effect",
          tool: "tiber_effect",
          turnId,
        },
      });
      send({
        id: "conflicting-repository-request",
        method: "item/tool/call",
        params: {
          arguments: {
            action: "write",
            expected: "before\n",
            path: "README.md",
            replacement: "after\n",
          },
          callId: "conflicting-repository-call",
          threadId: "thread-effect",
          tool: "tiber_repository_proposal",
          turnId,
        },
      });
      return;
    }
    if (mode === "foreign-exact") {
      send({
        id: "foreign-effect-request",
        method: "item/tool/call",
        params: {
          arguments: { operation: "foreign" },
          callId: "foreign-effect-call",
          threadId: "foreign-thread",
          tool: "tiber_effect",
          turnId: "foreign-turn",
        },
      });
      return;
    }
    const request = {
      id:
        mode === "invalid-request-id"
          ? 1.5
          : `effect-request-${nextTurn}`,
      method: "item/tool/call",
      params: {
        arguments:
          mode === "oversized-arguments"
            ? { payload: "x".repeat(17_000) }
            : { operation: "record_receipt", sequence: nextTurn },
        callId:
          mode === "control-call-id"
            ? "effect-call\u001b"
            : mode === "oversized-call-id"
            ? "x".repeat(300)
            : `effect-call-${nextTurn}`,
        threadId: "thread-effect",
        tool: "tiber_effect",
        turnId,
      },
    };
    if (mode === "missing-arguments") delete request.params.arguments;
    if (mode === "missing-call-id") delete request.params.callId;
    if (mode === "non-string-turn-id") request.params.turnId = ["turn-1"];
    send(request);
  } else if (message.id === "conflicting-effect-request") {
    fs.writeFileSync(process.argv[3], JSON.stringify(message));
  } else if (message.id === "conflicting-repository-request") {
    send({
      method: "turn/completed",
      params: {
        threadId: "thread-effect",
        turn: { id: `turn-${nextTurn}`, status: "completed" },
      },
    });
  } else if (message.id === "primary-effect-request") {
    send({
      method: "turn/completed",
      params: {
        threadId: "thread-effect",
        turn: { id: `turn-${nextTurn}`, status: "completed" },
      },
    });
  } else if (message.id === "repository-proposal-request") {
    send({
      method: "turn/completed",
      params: {
        threadId: "thread-effect",
        turn: { id: `turn-${nextTurn}`, status: "completed" },
      },
    });
  } else if (message.id === `effect-request-${nextTurn}`) {
    if (
      mode === "sequenced" &&
      !fs.existsSync(completionAuthorization)
    ) {
      fs.writeFileSync(earlyResponseMarker, "response arrived before authorization\n");
      send({
        method: "turn/completed",
        params: {
          threadId: "thread-effect",
          turn: { id: `turn-${nextTurn}`, status: "failed" },
        },
      });
      return;
    }
    const expected =
      mode === "failure"
        ? {
            contentItems: [
              {
                text: JSON.stringify({
                  code: "effect_denied",
                  message: "policy denied",
                  retryable: true,
                }),
                type: "inputText",
              },
            ],
            success: false,
          }
        : {
            contentItems: [{ text: "effect completed", type: "inputText" }],
            success: true,
          };
    if (JSON.stringify(message.result) !== JSON.stringify(expected)) {
      send({
        method: "turn/completed",
        params: {
          threadId: "thread-effect",
          turn: { id: `turn-${nextTurn}`, status: "failed" },
        },
      });
      return;
    }
    send({
      method: "item/agentMessage/delta",
      params: {
        delta: "completion observed",
        itemId: `assistant-${nextTurn}`,
        threadId: "thread-effect",
        turnId: `turn-${nextTurn}`,
      },
    });
    send({
      method: "turn/completed",
      params: {
        threadId: "thread-effect",
        turn: { id: `turn-${nextTurn}`, status: "completed" },
      },
    });
  } else if (message.id === "foreign-effect-request") {
    if (message.result?.success !== false) process.exit(3);
    send({
      method: "item/agentMessage/delta",
      params: {
        delta: "foreign exact effect rejected",
        itemId: "assistant-foreign-rejection",
        threadId: "thread-effect",
        turnId: `turn-${nextTurn}`,
      },
    });
    send({
      method: "turn/completed",
      params: {
        threadId: "thread-effect",
        turn: { id: `turn-${nextTurn}`, status: "completed" },
      },
    });
  }
});
