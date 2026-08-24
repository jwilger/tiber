import { createServer } from "node:http";
import { afterEach, describe, expect, it } from "vitest";
import { HindsightHttpService } from "../../src/adapters/memory/hindsight-http-service.js";
import {
  decideHindsightRetention,
  parseHindsightConfiguration,
  parseHindsightRecallRequest,
  parseHindsightRetentionCandidate,
} from "../../src/core/memory/hindsight.js";

const servers: ReturnType<typeof createServer>[] = [];
afterEach(async () => {
  await Promise.all(
    servers.splice(0).map(
      (server) =>
        new Promise<void>((resolve) =>
          server.close(() => {
            resolve();
          }),
        ),
    ),
  );
});

describe("optional isolated Hindsight memory", () => {
  it("keeps global, private repository, and opt-in shared banks separate", async () => {
    const observed: { path: string; body: unknown }[] = [];
    const server = createServer((request, response) => {
      const chunks: Uint8Array[] = [];
      request.on("data", (chunk: unknown) => {
        if (typeof chunk === "string" || chunk instanceof Uint8Array)
          chunks.push(Buffer.from(chunk));
      });
      request.on("end", () => {
        const body: unknown = JSON.parse(
          Buffer.concat(chunks).toString("utf8"),
        );
        observed.push({ path: request.url ?? "", body });
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify(
            request.url?.endsWith("/recall")
              ? {
                  results: [
                    {
                      id: "m1",
                      text: "Prefer exact tests",
                      type: "observation",
                      tags: [],
                    },
                  ],
                }
              : {
                  success: true,
                  bank_id: "bank",
                  items_count: 1,
                  async: false,
                },
          ),
        );
      });
    });
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("missing address");
    const config = parseHindsightConfiguration({
      endpoint: `http://127.0.0.1:${String(address.port)}`,
      repositoryIdentity: "git@example/repo",
      userIdentity: "/users/test/.pi",
      permissions: {
        global: { recall: true, retain: true },
        private: { recall: true, retain: true },
        shared: { recall: true, retain: true },
      },
      sharedBankId: "team-project",
    });
    if (!config.ok) throw new Error("invalid fixture");
    const service = new HindsightHttpService(config.value);
    for (const scope of ["global", "private", "shared"] as const) {
      const request = parseHindsightRecallRequest({
        scope,
        query: "testing preferences",
        phase: "explicit",
      });
      if (!request.ok) throw new Error("invalid request");
      await expect(service.recall(request.value)).resolves.toMatchObject({
        ok: true,
        value: { memories: [{ text: "Prefer exact tests" }] },
      });
    }
    expect(
      new Set(
        observed.map(({ path }) => path.split("/banks/")[1]?.split("/")[0]),
      ).size,
    ).toBe(3);
    expect(
      observed.every(({ body }) => typeof body === "object" && body !== null),
    ).toBe(true);
  });

  it("filters retention and only permits reviewed completion in shared memory", () => {
    const secret = parseHindsightRetentionCandidate({
      scope: "private",
      kind: "checkpoint",
      content: "token ghp_abcdefghijklmnopqrstuvwxyz123456",
      documentId: "task-1",
      reviewedCompletion: false,
      includesRawOutput: false,
      includesSource: false,
      includesDiff: false,
    });
    expect(secret.ok && decideHindsightRetention(secret.value)).toMatchObject({
      status: "denied",
      code: "TIBER_HINDSIGHT_SECRET_EXCLUDED",
    });
    const shared = parseHindsightRetentionCandidate({
      scope: "shared",
      kind: "checkpoint",
      content: "implemented parser",
      documentId: "task-1",
      reviewedCompletion: false,
      includesRawOutput: false,
      includesSource: false,
      includesDiff: false,
    });
    expect(shared.ok && decideHindsightRetention(shared.value)).toMatchObject({
      status: "denied",
      code: "TIBER_HINDSIGHT_SHARED_COMPLETION_REQUIRED",
    });
  });

  it.each([
    { body: "not-json", code: "TIBER_HINDSIGHT_RESPONSE_INVALID" },
    {
      body: JSON.stringify({
        results: [{ id: "m", text: "x".repeat(270_000), type: "world" }],
      }),
      code: "TIBER_HINDSIGHT_RESPONSE_OVERSIZED",
    },
  ])("fails closed for $code", async ({ body, code }) => {
    const server = createServer((_request, response) => response.end(body));
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("missing address");
    const config = parseHindsightConfiguration({
      endpoint: `http://127.0.0.1:${String(address.port)}`,
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions: {
        global: { recall: false, retain: false },
        private: { recall: true, retain: false },
        shared: { recall: false, retain: false },
      },
    });
    const request = parseHindsightRecallRequest({
      scope: "private",
      query: "work",
      phase: "explicit",
    });
    if (!config.ok || !request.ok) throw new Error("invalid fixture");
    await expect(
      new HindsightHttpService(config.value).recall(request.value),
    ).resolves.toMatchObject({ ok: false, failure: { code } });
  });

  it("sends only filtered private checkpoints and reviewed shared completions", async () => {
    const paths: string[] = [];
    const server = createServer((request, response) => {
      request.resume();
      request.on("end", () => {
        paths.push(request.url ?? "");
        response.end(
          JSON.stringify({
            success: true,
            bank_id: "bank",
            items_count: 1,
            async: false,
          }),
        );
      });
    });
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("missing address");
    const config = parseHindsightConfiguration({
      endpoint: `http://127.0.0.1:${String(address.port)}`,
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions: {
        global: { recall: false, retain: false },
        private: { recall: false, retain: true },
        shared: { recall: false, retain: true },
      },
      sharedBankId: "team",
    });
    if (!config.ok) throw new Error("invalid fixture");
    const client = new HindsightHttpService(config.value);
    for (const input of [
      {
        scope: "private",
        kind: "checkpoint",
        reviewedCompletion: false,
        content: "RED accepted and parser implemented",
        documentId: "checkpoint-1",
      },
      {
        scope: "shared",
        kind: "completion",
        reviewedCompletion: true,
        content: "Task task-1 completed with reviewed digest abc",
        documentId: "completion-1",
      },
    ] as const) {
      const candidate = parseHindsightRetentionCandidate({
        ...input,
        includesRawOutput: false,
        includesSource: false,
        includesDiff: false,
      });
      if (!candidate.ok) throw new Error("invalid fixture");
      await expect(client.retain(candidate.value)).resolves.toMatchObject({
        ok: true,
      });
    }
    expect(paths).toHaveLength(2);
    expect(paths[0]).toContain(config.value.banks.private);
    expect(paths[1]).toContain("/banks/team/");
  });
});
