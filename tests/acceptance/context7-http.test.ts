import { createServer } from "node:http";
import { afterEach, describe, expect, it } from "vitest";
import { Context7HttpService } from "../../src/adapters/context/context7-http-service.js";
import {
  parseContext7Endpoint,
  parseContext7NetworkCapability,
  parseContext7QueryRequest,
  parseContext7ResolveRequest,
} from "../../src/core/context/context7.js";

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

describe("bounded direct Context7 HTTP", () => {
  it("resolves a library with exact source and cache provenance", async () => {
    let requests = 0;
    const server = createServer((_request, response) => {
      requests += 1;
      response.setHeader("content-type", "application/json");
      response.end(
        JSON.stringify({
          results: [
            {
              id: "/vitest-dev/vitest",
              title: "Vitest",
              description: "Testing",
              branch: "main",
              totalTokens: 1200,
            },
          ],
        }),
      );
    });
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("missing test address");
    const endpoint = parseContext7Endpoint(
      `http://127.0.0.1:${String(address.port)}/api/v2`,
    );
    const network = parseContext7NetworkCapability({
      endpoint: endpoint.ok ? endpoint.value : "",
      enabled: true,
    });
    const request = parseContext7ResolveRequest({
      libraryName: "vitest",
      query: "mock timers",
    });
    if (!network.ok || !request.ok) throw new Error("invalid fixture");
    const service = new Context7HttpService(network.value);

    const first = await service.resolveLibrary(request.value);
    const second = await service.resolveLibrary(request.value);

    expect(first).toMatchObject({
      ok: true,
      value: {
        cache: "miss",
        source: { endpoint: endpoint.ok ? endpoint.value : "" },
        libraries: [{ libraryId: "/vitest-dev/vitest", version: "main" }],
      },
    });
    expect(second).toMatchObject({ ok: true, value: { cache: "hit" } });
    expect(requests).toBe(1);
  });

  it.each([
    {
      body: "not-json",
      maximumResponseBytes: 1024,
      code: "TIBER_CONTEXT7_RESPONSE_INVALID",
    },
    {
      body: JSON.stringify({ content: "x".repeat(2048) }),
      maximumResponseBytes: 1024,
      code: "TIBER_CONTEXT7_RESPONSE_OVERSIZED",
    },
  ])("fails safely for $code", async ({ body, maximumResponseBytes, code }) => {
    const server = createServer((_request, response) => response.end(body));
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("missing test address");
    const network = parseContext7NetworkCapability({
      endpoint: `http://127.0.0.1:${String(address.port)}/api/v2`,
      enabled: true,
      maximumResponseBytes,
    });
    const request = parseContext7QueryRequest({
      libraryId: "/org/repo",
      query: "api",
    });
    if (!network.ok || !request.ok) throw new Error("invalid fixture");
    await expect(
      new Context7HttpService(network.value).queryDocs(request.value),
    ).resolves.toMatchObject({ ok: false, failure: { code } });
  });

  it("denies absent authority and unauthorized endpoints without sending HTTP", () => {
    expect(
      parseContext7NetworkCapability({
        endpoint: "https://evil.example/api/v2",
        enabled: true,
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_CONTEXT7_ENDPOINT_DENIED" },
    });
    expect(
      parseContext7NetworkCapability({
        endpoint: "https://context7.com/api/v2",
        enabled: false,
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_CONTEXT7_NETWORK_UNAVAILABLE" },
    });
  });
});
