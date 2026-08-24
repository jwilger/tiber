import { describe, expect, it } from "vitest";
import {
  authorizeHindsightOperation,
  decideHindsightRetention,
  parseHindsightConfiguration,
  parseHindsightRecallRequest,
  parseHindsightRecallResponse,
  parseHindsightRetentionCandidate,
} from "../../src/core/memory/hindsight.js";

const permissions = {
  global: { recall: true, retain: false },
  private: { recall: true, retain: true },
  shared: { recall: false, retain: false },
};
const configured = (overrides: Readonly<Record<string, unknown>> = {}) =>
  parseHindsightConfiguration({
    endpoint: "https://memory.example/",
    repositoryIdentity: "repo",
    userIdentity: "user",
    permissions,
    ...overrides,
  });

describe("Hindsight memory policy", () => {
  it("derives stable separate user and repository banks", () => {
    const first = configured();
    const second = configured();
    expect(first).toEqual(second);
    if (!first.ok) throw new Error("invalid fixture");
    expect(first.value.banks.global).toMatch(/^tiber-global-[a-f0-9]{32}$/u);
    expect(first.value.banks.private).toMatch(/^tiber-private-[a-f0-9]{32}$/u);
    expect(first.value.banks.global).not.toBe(first.value.banks.private);
  });
  it.each([
    null,
    {},
    {
      endpoint: "not-url",
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions,
    },
    {
      endpoint: "ftp://memory.example/",
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions,
    },
    {
      endpoint: "https://user:pass@memory.example/",
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions,
    },
    {
      endpoint: "https://memory.example/path",
      repositoryIdentity: "repo",
      userIdentity: "user",
      permissions,
    },
  ])("rejects invalid configuration %#", (value) => {
    expect(parseHindsightConfiguration(value).ok).toBe(false);
  });
  it("requires exact independent permissions and explicit shared opt-in", () => {
    expect(configured({ permissions: {} }).ok).toBe(false);
    expect(
      configured({
        permissions: {
          ...permissions,
          shared: { recall: true, retain: false },
        },
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_HINDSIGHT_PERMISSION_DENIED" },
    });
    expect(
      configured({
        permissions: { ...permissions, shared: { recall: true, retain: true } },
        sharedBankId: "team-bank",
      }),
    ).toMatchObject({ ok: true, value: { banks: { shared: "team-bank" } } });
    expect(configured({ sharedBankId: "bad/bank" }).ok).toBe(false);
  });
  it("bounds initial and explicit recall independently", () => {
    expect(
      parseHindsightRecallRequest({
        scope: "private",
        query: " work ",
        phase: "initial",
      }),
    ).toEqual({
      ok: true,
      value: {
        scope: "private",
        query: "work",
        phase: "initial",
        maximumTokens: 256,
      },
    });
    expect(
      parseHindsightRecallRequest({
        scope: "global",
        query: "work",
        phase: "explicit",
      }),
    ).toMatchObject({ ok: true, value: { maximumTokens: 1024 } });
    for (const value of [
      null,
      { scope: "bad", query: "x", phase: "initial" },
      { scope: "private", query: "", phase: "initial" },
      { scope: "private", query: "x", phase: "bad" },
    ])
      expect(parseHindsightRecallRequest(value).ok).toBe(false);
  });
  it("excludes all raw material, secrets, and unreviewed shared writes", () => {
    const candidate = (overrides: Readonly<Record<string, unknown>> = {}) =>
      parseHindsightRetentionCandidate({
        scope: "private",
        kind: "checkpoint",
        content: "Parser completed",
        documentId: "task-1",
        reviewedCompletion: false,
        includesRawOutput: false,
        includesSource: false,
        includesDiff: false,
        ...overrides,
      });
    const valid = candidate();
    if (!valid.ok) throw new Error("invalid fixture");
    expect(decideHindsightRetention(valid.value)).toMatchObject({
      status: "authorized",
    });
    for (const key of [
      "includesRawOutput",
      "includesSource",
      "includesDiff",
    ] as const) {
      const parsed = candidate({ [key]: true });
      if (!parsed.ok) throw new Error("invalid fixture");
      expect(decideHindsightRetention(parsed.value)).toMatchObject({
        status: "denied",
        code: "TIBER_HINDSIGHT_RAW_MATERIAL_EXCLUDED",
      });
    }
    for (const content of [
      "diff --git a/file b/file",
      "@@ -1 +1 @@",
      "--- a/file\n+++ b/file",
      "```ts\nconst leaked = true",
      "--- stdout ---\nraw",
      "export function leaked() {}",
    ]) {
      const parsed = candidate({ content });
      if (!parsed.ok) throw new Error("invalid fixture");
      expect(decideHindsightRetention(parsed.value)).toMatchObject({
        status: "denied",
        code: "TIBER_HINDSIGHT_RAW_MATERIAL_EXCLUDED",
      });
    }
    for (const content of [
      "token=abcdefghijk",
      "ghp_abcdefghijklmnopqrstuvwxyz123456",
      "AKIAABCDEFGHIJKLMNOP",
      "-----BEGIN PRIVATE KEY-----",
    ]) {
      const parsed = candidate({ content });
      if (!parsed.ok) throw new Error("invalid fixture");
      expect(decideHindsightRetention(parsed.value)).toMatchObject({
        status: "denied",
        code: "TIBER_HINDSIGHT_SECRET_EXCLUDED",
      });
    }
    const shared = candidate({ scope: "shared" });
    if (!shared.ok) throw new Error("invalid fixture");
    expect(decideHindsightRetention(shared.value)).toMatchObject({
      status: "denied",
      code: "TIBER_HINDSIGHT_SHARED_COMPLETION_REQUIRED",
    });
    const completion = candidate({
      scope: "shared",
      kind: "completion",
      reviewedCompletion: true,
    });
    if (!completion.ok) throw new Error("invalid fixture");
    expect(decideHindsightRetention(completion.value)).toMatchObject({
      status: "authorized",
    });
  });
  it("parses only bounded structured recall facts", () => {
    expect(
      parseHindsightRecallResponse({
        results: [
          { id: "m1", text: "fact", type: "world", tags: ["scope:private"] },
        ],
      }),
    ).toEqual({
      ok: true,
      value: [
        { id: "m1", text: "fact", type: "world", tags: ["scope:private"] },
      ],
    });
    for (const value of [
      null,
      {},
      { results: new Array(21).fill({}) },
      { results: [{}] },
      { results: [{ id: "m", text: "x", type: "bad" }] },
      { results: [{ id: "m", text: "x", type: "world", tags: [1] }] },
    ])
      expect(parseHindsightRecallResponse(value).ok).toBe(false);
  });
  it("authorizes recall and retain separately per bank", () => {
    const config = configured();
    if (!config.ok) throw new Error("invalid fixture");
    expect(
      authorizeHindsightOperation(config.value, "global", "recall"),
    ).toMatchObject({ ok: true });
    expect(
      authorizeHindsightOperation(config.value, "global", "retain"),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_HINDSIGHT_PERMISSION_DENIED" },
    });
    expect(
      authorizeHindsightOperation(config.value, "shared", "recall"),
    ).toMatchObject({ ok: false });
  });

  it("returns complete stable failures for every policy boundary", () => {
    const expected = (code: string, message: string, evidence: string) => ({
      ok: false,
      failure: {
        code,
        message,
        safeContext: { domain: "hindsight" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: [evidence],
        redaction: "public",
      },
    });
    expect(parseHindsightConfiguration(null)).toEqual(
      expected(
        "TIBER_HINDSIGHT_UNAVAILABLE",
        "Hindsight configuration is unavailable",
        "corrected-memory-input",
      ),
    );
    expect(
      parseHindsightConfiguration({
        endpoint: "bad",
        repositoryIdentity: "r",
        userIdentity: "u",
        permissions,
      }),
    ).toEqual(
      expected(
        "TIBER_HINDSIGHT_VALUE_INVALID",
        "Hindsight endpoint is invalid",
        "corrected-memory-input",
      ),
    );
    expect(
      parseHindsightConfiguration({
        endpoint: "ftp://memory.example/",
        repositoryIdentity: "r",
        userIdentity: "u",
        permissions,
      }),
    ).toEqual(
      expected(
        "TIBER_HINDSIGHT_ENDPOINT_DENIED",
        "Hindsight endpoint is not authorized",
        "memory-permission",
      ),
    );
    expect(configured({ permissions: {} })).toEqual(
      expected(
        "TIBER_HINDSIGHT_VALUE_INVALID",
        "Hindsight permissions are invalid",
        "corrected-memory-input",
      ),
    );
    expect(configured({ sharedBankId: "bad/bank" })).toEqual(
      expected(
        "TIBER_HINDSIGHT_VALUE_INVALID",
        "Hindsight shared bank is invalid",
        "corrected-memory-input",
      ),
    );
    expect(
      configured({
        permissions: {
          ...permissions,
          shared: { recall: true, retain: false },
        },
      }),
    ).toEqual(
      expected(
        "TIBER_HINDSIGHT_PERMISSION_DENIED",
        "Shared memory requires an explicit bank opt-in",
        "memory-permission",
      ),
    );
    expect(parseHindsightRecallRequest({})).toEqual(
      expected(
        "TIBER_HINDSIGHT_VALUE_INVALID",
        "Hindsight recall request is invalid",
        "corrected-memory-input",
      ),
    );
    expect(parseHindsightRetentionCandidate({})).toEqual(
      expected(
        "TIBER_HINDSIGHT_VALUE_INVALID",
        "Hindsight retention candidate is invalid",
        "corrected-memory-input",
      ),
    );
    expect(parseHindsightRecallResponse({})).toEqual(
      expected(
        "TIBER_HINDSIGHT_RESPONSE_INVALID",
        "Hindsight recall response is invalid",
        "valid-memory-response",
      ),
    );
  });

  it("checks every configuration component and endpoint restriction", () => {
    for (const patch of [
      { endpoint: "" },
      { repositoryIdentity: "" },
      { userIdentity: "" },
      { permissions: null },
    ])
      expect(
        parseHindsightConfiguration({
          endpoint: "https://memory.example/",
          repositoryIdentity: "repo",
          userIdentity: "user",
          permissions,
          ...patch,
        }).ok,
      ).toBe(false);
    for (const endpoint of [
      "http://memory.example/",
      "http://127.0.0.1/",
      "http://localhost:1234/",
      "https://user@memory.example/",
      "https://:pass@memory.example/",
      "https://memory.example/?q=1",
      "https://memory.example/#x",
      "https://memory.example/path",
    ])
      expect(configured({ endpoint }).ok).toBe(false);
    expect(configured({ endpoint: "http://127.0.0.1:1234/" })).toMatchObject({
      ok: true,
    });
    const validPermission = { recall: true, retain: false };
    for (const shared of [
      null,
      {},
      { recall: "yes", retain: false },
      { recall: true, retain: "no" },
    ])
      expect(
        configured({
          permissions: {
            global: validPermission,
            private: validPermission,
            shared,
          },
        }).ok,
      ).toBe(false);
    expect(configured({ sharedBankId: "x".repeat(200) })).toMatchObject({
      ok: true,
    });
    expect(configured({ sharedBankId: "x".repeat(201) }).ok).toBe(false);
    expect(configured({ sharedBankId: " prefix" }).ok).toBe(false);
  });

  it("checks every recall and retention value boundary", () => {
    for (const scope of ["global", "private", "shared"] as const)
      expect(
        parseHindsightRecallRequest({ scope, query: "q", phase: "explicit" }),
      ).toMatchObject({ ok: true });
    expect(
      parseHindsightRecallRequest({
        scope: "private",
        query: "q".repeat(2000),
        phase: "explicit",
      }),
    ).toMatchObject({ ok: true });
    expect(
      parseHindsightRecallRequest({
        scope: "private",
        query: "q".repeat(2001),
        phase: "explicit",
      }).ok,
    ).toBe(false);
    expect(
      parseHindsightRecallRequest({
        scope: "private",
        query: "   ",
        phase: "explicit",
      }).ok,
    ).toBe(false);
    const base = {
      scope: "private",
      kind: "checkpoint",
      content: "checkpoint",
      documentId: "doc",
      reviewedCompletion: false,
      includesRawOutput: false,
      includesSource: false,
      includesDiff: false,
    };
    expect(
      parseHindsightRetentionCandidate({
        ...base,
        content: "x".repeat(16_384),
        documentId: "d".repeat(500),
      }),
    ).toMatchObject({ ok: true });
    for (const patch of [
      { scope: "bad" },
      { kind: "bad" },
      { content: "" },
      { content: "x".repeat(16_385) },
      { documentId: "" },
      { documentId: "d".repeat(501) },
      { reviewedCompletion: "no" },
      { includesRawOutput: "no" },
      { includesSource: "no" },
      { includesDiff: "no" },
    ])
      expect(parseHindsightRetentionCandidate({ ...base, ...patch }).ok).toBe(
        false,
      );
    expect(
      parseHindsightRetentionCandidate({
        ...base,
        scope: "global",
        kind: "completion",
      }),
    ).toMatchObject({ ok: true });
  });

  it("detects credential forms while avoiding ordinary words", () => {
    const decide = (content: string) => {
      const parsed = parseHindsightRetentionCandidate({
        scope: "private",
        kind: "checkpoint",
        content,
        documentId: "doc",
        reviewedCompletion: false,
        includesRawOutput: false,
        includesSource: false,
        includesDiff: false,
      });
      if (!parsed.ok) throw new Error("invalid fixture");
      return decideHindsightRetention(parsed.value);
    };
    for (const content of [
      "secret: abcdefgh",
      "password = abcdefgh",
      "api_key=abcdefgh",
      "TOKEN: abcdefgh",
      "gho_abcdefghijklmnopqrst",
      "ghu_abcdefghijklmnopqrst",
      "ghs_abcdefghijklmnopqrst",
      "ghr_abcdefghijklmnopqrst",
      "-----BEGIN RSA PRIVATE KEY-----",
    ])
      expect(decide(content)).toMatchObject({
        status: "denied",
        code: "TIBER_HINDSIGHT_SECRET_EXCLUDED",
      });
    expect(decide("token is a useful parser concept")).toMatchObject({
      status: "authorized",
    });
  });

  it("checks every recalled fact field and collection maximum", () => {
    const good = { id: "m", text: "fact", type: "world", tags: ["tag"] };
    expect(
      parseHindsightRecallResponse({ results: new Array(20).fill(good) }),
    ).toMatchObject({ ok: true });
    for (const item of [
      null,
      [],
      { ...good, id: "" },
      { ...good, id: "i".repeat(501) },
      { ...good, text: "" },
      { ...good, text: "x".repeat(16_385) },
      { ...good, type: "bad" },
      { ...good, tags: "bad" },
      { ...good, tags: new Array(21).fill("x") },
      { ...good, tags: [1] },
      { ...good, tags: ["x".repeat(201)] },
    ])
      expect(parseHindsightRecallResponse({ results: [item] })).toEqual({
        ok: false,
        failure: {
          code: "TIBER_HINDSIGHT_RESPONSE_INVALID",
          message: "Hindsight recall response is invalid",
          safeContext: { domain: "hindsight" },
          causes: [],
          retryability: "retry-after-input",
          requiredRecoveryEvidence: ["valid-memory-response"],
          redaction: "public",
        },
      });
    for (const type of ["world", "experience", "observation"] as const)
      expect(
        parseHindsightRecallResponse({
          results: [{ ...good, type, tags: null }],
        }),
      ).toEqual({
        ok: true,
        value: [{ id: "m", text: "fact", type, tags: [] }],
      });
    expect(
      parseHindsightRecallResponse({ results: [{ ...good, tags: undefined }] }),
    ).toMatchObject({ ok: true, value: [{ tags: [] }] });
    expect(
      parseHindsightRecallResponse({
        results: [
          {
            ...good,
            id: "i".repeat(500),
            text: "x".repeat(16_384),
            tags: ["t".repeat(200)],
          },
        ],
      }),
    ).toMatchObject({ ok: true });
  });

  it("distinguishes permission, scope, and collection edge cases", () => {
    expect(configured({ endpoint: "ftp://127.0.0.1:1234/" }).ok).toBe(false);
    const valid = { recall: true, retain: true };
    for (const permissionsPatch of [
      { global: {}, private: valid, shared: valid },
      { global: valid, private: {}, shared: valid },
      { global: valid, private: valid, shared: {} },
      {
        global: { recall: "yes", retain: true },
        private: valid,
        shared: valid,
      },
      {
        global: { recall: true, retain: "yes" },
        private: valid,
        shared: valid,
      },
    ])
      expect(
        configured({ permissions: permissionsPatch, sharedBankId: "team" }).ok,
      ).toBe(false);
    const candidate = (
      kind: "checkpoint" | "completion",
      reviewedCompletion: boolean,
    ) => {
      const parsed = parseHindsightRetentionCandidate({
        scope: "shared",
        kind,
        content: "safe",
        documentId: "doc",
        reviewedCompletion,
        includesRawOutput: false,
        includesSource: false,
        includesDiff: false,
      });
      if (!parsed.ok) throw new Error("invalid fixture");
      return decideHindsightRetention(parsed.value);
    };
    expect(candidate("checkpoint", true)).toMatchObject({ status: "denied" });
    expect(candidate("completion", false)).toMatchObject({ status: "denied" });
    const good = { id: "m", text: "fact", type: "world", tags: [] };
    expect(
      parseHindsightRecallResponse({ results: new Array(21).fill(good) }),
    ).toMatchObject({ ok: false });
    expect(
      parseHindsightRecallResponse({
        results: [{ ...good, tags: new Array(20).fill("tag") }],
      }),
    ).toMatchObject({ ok: true });
  });

  it("does not overmatch credential-like near misses", () => {
    const decide = (content: string) => {
      const parsed = parseHindsightRetentionCandidate({
        scope: "private",
        kind: "checkpoint",
        content,
        documentId: "doc",
        reviewedCompletion: false,
        includesRawOutput: false,
        includesSource: false,
        includesDiff: false,
      });
      if (!parsed.ok) throw new Error("invalid fixture");
      return decideHindsightRetention(parsed.value);
    };
    for (const content of [
      "ghp_abcdefghijklmnopqrs",
      "AKIAABCDEFGHIJKLMNO",
      "token=abcdefg",
      "not an AKIA key",
    ])
      expect(decide(content)).toMatchObject({ status: "authorized" });
    expect(decide("apikey=abcdefgh")).toMatchObject({ status: "denied" });
  });

  it("denies a missing target even if an internal permission were present", () => {
    const parsed = configured();
    if (!parsed.ok) throw new Error("invalid fixture");
    const forged = {
      ...parsed.value,
      permissions: {
        ...parsed.value.permissions,
        shared: { recall: true, retain: false },
      },
    };
    expect(authorizeHindsightOperation(forged, "shared", "recall")).toEqual({
      ok: false,
      failure: {
        code: "TIBER_HINDSIGHT_PERMISSION_DENIED",
        message: "Hindsight recall is not permitted for shared memory",
        safeContext: { domain: "hindsight" },
        causes: [],
        retryability: "retry-after-input",
        requiredRecoveryEvidence: ["memory-permission"],
        redaction: "public",
      },
    });
  });
});
