import { describe, expect, it } from "vitest";
import {
  context7CacheKey,
  parseContext7Documentation,
  parseContext7Endpoint,
  parseContext7Libraries,
  parseContext7NetworkCapability,
  parseContext7QueryRequest,
  parseContext7ResolveRequest,
} from "../../src/core/context/context7.js";

describe("Context7 semantic boundaries", () => {
  it.each([
    undefined,
    null,
    "",
    "https://evil.example/api/v2",
    "https://context7.com/api/v1",
    "https://user@context7.com/api/v2",
    "https://context7.com/api/v2?x=1",
    "http://localhost:1234/api/v2",
    "http://127.0.0.1/api/v2",
  ])("denies endpoint %j", (value) => {
    expect(parseContext7Endpoint(value).ok).toBe(false);
  });
  it.each(["https://context7.com/api/v2", "http://127.0.0.1:1234/api/v2"])(
    "accepts constrained endpoint %s",
    (value) => {
      expect(parseContext7Endpoint(value)).toEqual({ ok: true, value });
    },
  );
  it("requires explicit bounded network authority", () => {
    expect(
      parseContext7NetworkCapability({
        enabled: false,
        endpoint: "https://context7.com/api/v2",
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_CONTEXT7_NETWORK_UNAVAILABLE" },
    });
    expect(
      parseContext7NetworkCapability({
        enabled: true,
        endpoint: "https://context7.com/api/v2",
        maximumResponseBytes: 1023,
      }),
    ).toMatchObject({ ok: false });
    expect(
      parseContext7NetworkCapability({
        enabled: true,
        endpoint: "https://context7.com/api/v2",
        timeoutMs: 60_001,
      }),
    ).toMatchObject({ ok: false });
  });
  it("parses bounded requests and exact library identifiers", () => {
    expect(
      parseContext7ResolveRequest({ libraryName: " react ", query: " hooks " }),
    ).toMatchObject({
      ok: true,
      value: { libraryName: "react", query: "hooks" },
    });
    expect(
      parseContext7ResolveRequest({ libraryName: "", query: "x" }).ok,
    ).toBe(false);
    expect(
      parseContext7QueryRequest({ libraryId: "/org/repo/v1", query: "api" }),
    ).toMatchObject({ ok: true });
    expect(
      parseContext7QueryRequest({
        libraryId: "https://evil.example",
        query: "api",
      }).ok,
    ).toBe(false);
  });
  it("parses bounded resolution and structured documentation payloads", () => {
    expect(
      parseContext7Libraries({
        results: [
          {
            id: "/org/repo",
            title: "Repo",
            description: "Docs",
            branch: "main",
            totalTokens: 1,
          },
        ],
      }),
    ).toMatchObject({
      ok: true,
      value: [{ libraryId: "/org/repo", version: "main" }],
    });
    expect(
      parseContext7Libraries({
        results: [
          {
            id: "bad",
            title: "Repo",
            description: "Docs",
            branch: "main",
            totalTokens: 1,
          },
        ],
      }).ok,
    ).toBe(false);
    expect(parseContext7Libraries({ results: new Array(21).fill({}) }).ok).toBe(
      false,
    );
    const id = parseContext7QueryRequest({
      libraryId: "/org/repo/v2",
      query: "x",
    });
    if (!id.ok) throw new Error("invalid fixture");
    expect(
      parseContext7Documentation(
        { codeSnippets: [{ title: "A" }], infoSnippets: [] },
        id.value.libraryId,
      ),
    ).toMatchObject({ ok: true, value: { version: "v2" } });
    expect(
      parseContext7Documentation({ codeSnippets: ["bad"], infoSnippets: [] })
        .ok,
    ).toBe(false);
    expect(parseContext7Documentation({ nope: true }).ok).toBe(false);
  });
  it("derives stable cache keys from endpoint, request kind, and complete request", () => {
    const endpoint = parseContext7Endpoint("https://context7.com/api/v2");
    const request = parseContext7ResolveRequest({
      libraryName: "react",
      query: "hooks",
    });
    if (!endpoint.ok || !request.ok) throw new Error("invalid fixture");
    expect(context7CacheKey("resolve", endpoint.value, request.value)).toMatch(
      /^[a-f0-9]{64}$/u,
    );
    expect(context7CacheKey("resolve", endpoint.value, request.value)).not.toBe(
      context7CacheKey("docs", endpoint.value, request.value),
    );
  });

  it("returns complete stable failures", () => {
    expect(parseContext7Endpoint("")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_VALUE_INVALID",
        message: "Context7 endpoint is invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-context7-input"],
        redaction: "public",
      },
    });
    expect(parseContext7Endpoint("https://evil.example/api/v2")).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_ENDPOINT_DENIED",
        message: "Context7 endpoint is not authorized",
        requiredRecoveryEvidence: ["network-authority"],
      },
    });
    expect(parseContext7NetworkCapability(null)).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_NETWORK_UNAVAILABLE",
        message: "Context7 network authority is unavailable",
      },
    });
  });

  it("enforces every endpoint component and boundary", () => {
    expect(parseContext7Endpoint("not a url").ok).toBe(false);
    expect(parseContext7Endpoint("x".repeat(2049)).ok).toBe(false);
    expect(parseContext7Endpoint(`https://context7.com/api/v2#x`).ok).toBe(
      false,
    );
    expect(parseContext7Endpoint("https://context7.com:444/api/v2").ok).toBe(
      false,
    );
    expect(parseContext7Endpoint("http://127.0.0.1:1234/api/v2")).toMatchObject(
      { ok: true },
    );
  });

  it("enforces both inclusive network boundaries and integer inputs", () => {
    for (const input of [
      { maximumResponseBytes: 1024, timeoutMs: 100 },
      { maximumResponseBytes: 4_194_304, timeoutMs: 60_000 },
    ])
      expect(
        parseContext7NetworkCapability({
          enabled: true,
          endpoint: "https://context7.com/api/v2",
          ...input,
        }),
      ).toMatchObject({ ok: true, value: input });
    for (const input of [
      { maximumResponseBytes: 1023 },
      { maximumResponseBytes: 4_194_305 },
      { maximumResponseBytes: 1.5 },
      { timeoutMs: 99 },
      { timeoutMs: 60_001 },
      { timeoutMs: 1.5 },
    ])
      expect(
        parseContext7NetworkCapability({
          enabled: true,
          endpoint: "https://context7.com/api/v2",
          ...input,
        }),
      ).toMatchObject({
        ok: false,
        failure: { message: "Context7 network bounds are invalid" },
      });
    expect(
      parseContext7NetworkCapability({
        enabled: true,
        endpoint: "https://context7.com/api/v2",
      }),
    ).toMatchObject({
      ok: true,
      value: { maximumResponseBytes: 1_048_576, timeoutMs: 10_000 },
    });
  });

  it("enforces every request field and inclusive byte bound", () => {
    expect(parseContext7ResolveRequest(null).ok).toBe(false);
    expect(
      parseContext7ResolveRequest({ libraryName: "x", query: "" }).ok,
    ).toBe(false);
    expect(
      parseContext7ResolveRequest({ libraryName: "x".repeat(201), query: "q" })
        .ok,
    ).toBe(false);
    expect(
      parseContext7ResolveRequest({
        libraryName: "x".repeat(200),
        query: "q".repeat(2000),
      }).ok,
    ).toBe(true);
    expect(
      parseContext7ResolveRequest({ libraryName: "x", query: "q".repeat(2001) })
        .ok,
    ).toBe(false);
    expect(parseContext7QueryRequest(null).ok).toBe(false);
    expect(parseContext7QueryRequest({ libraryId: "/a/b", query: "" }).ok).toBe(
      false,
    );
    expect(
      parseContext7QueryRequest({ libraryId: "/a/b/", query: "q" }).ok,
    ).toBe(false);
    expect(
      parseContext7QueryRequest({ libraryId: "/a/b", query: "q".repeat(4000) })
        .ok,
    ).toBe(true);
    expect(
      parseContext7QueryRequest({ libraryId: "/a/b", query: "q".repeat(4001) })
        .ok,
    ).toBe(false);
  });

  it("rejects each malformed resolution response field", () => {
    const good = {
      id: "/org/repo",
      title: "Repo",
      description: "Docs",
      branch: "main",
      totalTokens: 1,
    };
    expect(parseContext7Libraries(null).ok).toBe(false);
    expect(parseContext7Libraries({ results: "bad" }).ok).toBe(false);
    for (const item of [
      null,
      [],
      { ...good, id: "" },
      { ...good, title: "" },
      { ...good, description: 1 },
      { ...good, description: "x".repeat(4001) },
      { ...good, branch: "" },
      { ...good, totalTokens: 1.5 },
      { ...good, totalTokens: -1 },
    ])
      expect(parseContext7Libraries({ results: [item] })).toMatchObject({
        ok: false,
        failure: {
          code: "TIBER_CONTEXT7_RESPONSE_INVALID",
          requiredRecoveryEvidence: ["valid-context7-response"],
        },
      });
    expect(
      parseContext7Libraries({ results: [{ ...good, id: "invalid" }] }),
    ).toMatchObject({
      ok: false,
      failure: { message: "Context7 library identifier is invalid" },
    });
    expect(
      parseContext7Libraries({
        results: [
          {
            ...good,
            id: "/org/repo",
            title: "x".repeat(500),
            description: "x".repeat(4000),
            branch: "x".repeat(200),
            totalTokens: 0,
          },
        ],
      }),
    ).toMatchObject({ ok: true });
  });

  it("selects only valid documentation representations and provenance", () => {
    expect(parseContext7Documentation(null)).toMatchObject({
      ok: false,
      failure: { message: "Context7 documentation response is invalid" },
    });
    expect(
      parseContext7Documentation({
        text: "text",
        content: "content",
        version: "v1",
      }),
    ).toEqual({ ok: true, value: { text: "text", version: "v1" } });
    expect(parseContext7Documentation({ content: "content" })).toEqual({
      ok: true,
      value: { text: "content", version: "unspecified" },
    });
    expect(
      parseContext7Documentation({ codeSnippets: [], infoSnippets: [] }),
    ).toMatchObject({ ok: true });
    for (const value of [
      { codeSnippets: "bad", infoSnippets: [] },
      { codeSnippets: [], infoSnippets: "bad" },
      { codeSnippets: new Array(101).fill({}), infoSnippets: [] },
      { codeSnippets: [], infoSnippets: new Array(101).fill({}) },
      { codeSnippets: [null], infoSnippets: [] },
      { codeSnippets: [], infoSnippets: [null] },
      { text: "" },
      { text: "x".repeat(4_194_305) },
      { text: "x", version: "" },
      { text: "x", version: "v".repeat(201) },
    ])
      expect(parseContext7Documentation(value).ok).toBe(false);
    expect(
      parseContext7Documentation({
        text: "x".repeat(4_194_304),
        version: "v".repeat(200),
      }),
    ).toMatchObject({ ok: true });
  });

  it("distinguishes all remaining trust-boundary cases and failure metadata", () => {
    const full = (
      result: ReturnType<typeof parseContext7Endpoint>,
      code: string,
      message: string,
      evidence: string,
    ) => {
      expect(result).toEqual({
        ok: false,
        failure: {
          code,
          message,
          safeContext: { domain: "context7" },
          causes: [],
          retryability: "retry-after-input",
          requiredRecoveryEvidence: [evidence],
          redaction: "public",
        },
      });
    };
    full(
      parseContext7Endpoint("not a url"),
      "TIBER_CONTEXT7_VALUE_INVALID",
      "Context7 endpoint is invalid",
      "corrected-context7-input",
    );
    for (const endpoint of [
      "ftp://context7.com/api/v2",
      "https://127.0.0.1:1234/api/v2",
      "http://user:password@127.0.0.1:1234/api/v2",
    ])
      expect(parseContext7Endpoint(endpoint).ok).toBe(false);
    expect(parseContext7Endpoint("   ").ok).toBe(false);
    expect(parseContext7NetworkCapability(null)).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_NETWORK_UNAVAILABLE",
        message: "Context7 network authority is unavailable",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["network-authority"],
        redaction: "public",
      },
    });
    expect(
      parseContext7NetworkCapability({
        enabled: true,
        endpoint: "https://context7.com/api/v2",
        maximumResponseBytes: 1,
      }),
    ).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_VALUE_INVALID",
        message: "Context7 network bounds are invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["corrected-context7-input"],
        redaction: "public",
      },
    });
    for (const [result, message, evidence] of [
      [
        parseContext7ResolveRequest({}),
        "Context7 resolution request is invalid",
        "corrected-context7-input",
      ],
      [
        parseContext7QueryRequest({}),
        "Context7 documentation request is invalid",
        "corrected-context7-input",
      ],
      [
        parseContext7Libraries({}),
        "Context7 resolution response is invalid",
        "valid-context7-response",
      ],
      [
        parseContext7Documentation({}),
        "Context7 documentation response is invalid",
        "valid-context7-response",
      ],
    ] as const)
      expect(result).toMatchObject({
        ok: false,
        failure: { message, requiredRecoveryEvidence: [evidence] },
      });
  });

  it("accepts exact collection maxima and rejects anchored identifier near-matches", () => {
    const good = {
      id: "/org/repo",
      title: "Repo",
      description: "Docs",
      branch: "main",
      totalTokens: 1,
    };
    expect(
      parseContext7Libraries({ results: new Array(20).fill(good) }),
    ).toMatchObject({ ok: true });
    expect(
      parseContext7QueryRequest({ libraryId: "prefix/org/repo", query: "q" })
        .ok,
    ).toBe(false);
    expect(
      parseContext7Libraries({ results: [{ ...good, id: "invalid" }] }),
    ).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_RESPONSE_INVALID",
        message: "Context7 library identifier is invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["valid-context7-response"],
        redaction: "public",
      },
    });
    const structured = parseContext7Documentation({
      codeSnippets: new Array(100).fill({ a: 1 }),
      infoSnippets: new Array(100).fill({ b: 2 }),
    });
    expect(structured).toMatchObject({ ok: true });
    if (structured.ok) {
      expect(structured.value.text).toContain("codeSnippets");
      expect(structured.value.text).toContain("infoSnippets");
      expect(structured.value.text).toContain('"a":1');
    }
    expect(
      parseContext7Documentation({ codeSnippets: [], infoSnippets: [] }),
    ).toEqual({
      ok: true,
      value: {
        text: '{"codeSnippets":[],"infoSnippets":[]}',
        version: "unspecified",
      },
    });
  });

  it("preserves distinct denial codes at every parser branch", () => {
    expect(
      parseContext7Endpoint(`https://context7.com/api/v2${"x".repeat(2048)}`),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_CONTEXT7_VALUE_INVALID" },
    });
    expect(
      parseContext7Endpoint("http://:password@127.0.0.1:1234/api/v2").ok,
    ).toBe(false);
    expect(
      parseContext7ResolveRequest({ libraryName: "   ", query: "q" }),
    ).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_VALUE_INVALID",
        message: "Context7 resolution request is invalid",
      },
    });
    expect(
      parseContext7QueryRequest({ libraryId: "bad", query: "q" }),
    ).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_VALUE_INVALID",
        message: "Context7 documentation request is invalid",
      },
    });
    const good = {
      id: "/org/repo",
      title: "Repo",
      description: "Docs",
      branch: "main",
      totalTokens: 1,
    };
    expect(
      parseContext7Libraries({ results: new Array(21).fill(good) }),
    ).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_RESPONSE_INVALID",
        message: "Context7 resolution response is invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["valid-context7-response"],
        redaction: "public",
      },
    });
    expect(parseContext7Libraries({ results: [{}] })).toMatchObject({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_RESPONSE_INVALID",
        message: "Context7 resolution response is invalid",
      },
    });
    expect(parseContext7Documentation(null)).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_RESPONSE_INVALID",
        message: "Context7 documentation response is invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["valid-context7-response"],
        redaction: "public",
      },
    });
    expect(parseContext7Documentation({ text: "" })).toEqual({
      ok: false,
      failure: {
        code: "TIBER_CONTEXT7_RESPONSE_INVALID",
        message: "Context7 documentation response is invalid",
        safeContext: { domain: "context7" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["valid-context7-response"],
        redaction: "public",
      },
    });
  });
});
