import { describe, expect, it } from "vitest";

import { GhGitHubHttpClient } from "../../src/adapters/github/gh-github-http-client.js";
import { parseGitHubReviewCredential } from "../../src/adapters/github/github-review-service.js";

function credential() {
  const parsed = parseGitHubReviewCredential("host-gh");
  if (!parsed.ok) throw new Error("invalid credential fixture");
  return parsed.value;
}

describe("GitHub CLI HTTP client", () => {
  it("uses the installed gh authentication without receiving a token", async () => {
    const calls: {
      readonly argv: readonly string[];
      readonly input: string;
    }[] = [];
    const client = new GhGitHubHttpClient((argv, input) => {
      calls.push({ argv, input });
      return Promise.resolve({
        code: 0,
        stdout: '{"ok":true}',
        stderr: "",
      });
    });

    expect(
      await client.request({
        method: "POST",
        path: "/graphql",
        credential: credential(),
        body: { query: "query { viewer { login } }" },
      }),
    ).toEqual({ ok: true, value: { ok: true } });
    expect(calls).toEqual([
      {
        argv: ["api", "--method", "POST", "/graphql", "--input", "-"],
        input: '{"query":"query { viewer { login } }"}',
      },
    ]);
  });

  it("returns a typed failure when gh cannot complete the request", async () => {
    const client = new GhGitHubHttpClient(() =>
      Promise.resolve({ code: 1, stdout: "", stderr: "not logged in" }),
    );

    expect(
      await client.request({
        method: "GET",
        path: "/user",
        credential: credential(),
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_REVIEW_SERVICE_FAILED" },
    });
  });
});
