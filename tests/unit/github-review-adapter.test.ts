import { describe, expect, it } from "vitest";

import {
  GitHubCiAdapter,
  GitHubMergeAdapter,
  GitHubPullRequestAdapter,
  GitHubReviewAdapter,
  parseGitHubCiCredential,
  parseGitHubMergeCredential,
  parseGitHubPullRequestCredential,
  parseGitHubReviewCredential,
  type GitHubHttpClient,
  type GitHubHttpRequest,
} from "../../src/adapters/github/github-review-service.js";
import {
  parseReviewBaseRef,
  parseReviewBody,
  parseReviewHeadRef,
  parseReviewRevision,
  parseReviewTitle,
  parseReviewRepositoryName,
  parseReviewRepositoryOwner,
} from "../../src/core/reviews/review-service-values.js";

function parsed<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  if (!result.ok) throw new Error("fixture invalid");
  return result.value;
}

const request = {
  repositoryOwner: parsed(parseReviewRepositoryOwner("owner")),
  repositoryName: parsed(parseReviewRepositoryName("repo")),
  headRef: parsed(parseReviewHeadRef("refs/heads/feat/review")),
  headRevision: parsed(parseReviewRevision("a".repeat(40))),
  baseRef: parsed(parseReviewBaseRef("main")),
  title: parsed(parseReviewTitle("feat(review): add adapter")),
  body: parsed(parseReviewBody("Exact reviewed delivery.")),
};

class StubClient implements GitHubHttpClient {
  public readonly requests: GitHubHttpRequest[] = [];
  public constructor(private readonly responses: unknown[]) {}
  public request(
    input: GitHubHttpRequest,
  ): Promise<{ readonly ok: true; readonly value: unknown }> {
    this.requests.push(input);
    const value = this.responses.shift();
    return Promise.resolve({ ok: true, value });
  }
}

function credentials() {
  const pullRequest = parseGitHubPullRequestCredential("pr-token");
  const review = parseGitHubReviewCredential("review-token");
  const ci = parseGitHubCiCredential("ci-token");
  const merge = parseGitHubMergeCredential("merge-token");
  if (!pullRequest.ok || !review.ok || !ci.ok || !merge.ok)
    throw new Error("credential fixture invalid");
  return {
    pullRequest: pullRequest.value,
    review: review.value,
    ci: ci.value,
    merge: merge.value,
  };
}

const pullResponse = {
  number: 42,
  node_id: "PR_node",
  html_url: "https://github.com/owner/repo/pull/42",
  user: { login: "author" },
  head: { sha: "a".repeat(40) },
};

describe("thin GitHub review adapters", () => {
  it("creates an exact PR using only the PR permission", async () => {
    const client = new StubClient([pullResponse]);
    const receipt = await new GitHubPullRequestAdapter(
      client,
      credentials().pullRequest,
    ).create(request);
    expect(receipt.ok).toBe(true);
    expect(client.requests).toEqual([
      {
        method: "POST",
        path: "/repos/owner/repo/pulls",
        credential: "pr-token",
        body: {
          head: "feat/review",
          base: "main",
          title: "feat(review): add adapter",
          body: "Exact reviewed delivery.",
        },
      },
    ]);
  });

  it("observes approvals and every resolved conversation using only review permission", async () => {
    const client = new StubClient([
      [{ state: "APPROVED", user: { login: "reviewer" } }],
      {
        data: {
          node: {
            reviewThreads: {
              pageInfo: { hasNextPage: false },
              nodes: [{ isResolved: true }, { isResolved: true }],
            },
          },
        },
      },
    ]);
    const pullClient = new StubClient([pullResponse]);
    const pull = await new GitHubPullRequestAdapter(
      pullClient,
      credentials().pullRequest,
    ).create(request);
    if (!pull.ok) throw new Error("fixture invalid");
    const result = await new GitHubReviewAdapter(
      client,
      credentials().review,
    ).observe(request, pull.value);
    expect(result).toEqual({
      ok: true,
      value: {
        headRevision: request.headRevision,
        reviewStatus: "approved",
        conversationStatus: "resolved",
      },
    });
    expect(
      client.requests.every(({ credential }) => credential === "review-token"),
    ).toBe(true);
  });

  it("uses each reviewer's latest state and fails closed on GraphQL errors", async () => {
    const pullClient = new StubClient([pullResponse]);
    const pull = await new GitHubPullRequestAdapter(
      pullClient,
      credentials().pullRequest,
    ).create(request);
    if (!pull.ok) throw new Error("fixture invalid");
    const latest = new StubClient([
      [
        { state: "APPROVED", user: { login: "reviewer" } },
        { state: "DISMISSED", user: { login: "reviewer" } },
      ],
      {
        data: {
          node: {
            reviewThreads: {
              pageInfo: { hasNextPage: false },
              nodes: [],
            },
          },
        },
      },
    ]);
    expect(
      await new GitHubReviewAdapter(latest, credentials().review).observe(
        request,
        pull.value,
      ),
    ).toMatchObject({ ok: true, value: { reviewStatus: "pending" } });

    const errored = new StubClient([
      [{ state: "APPROVED", user: { login: "reviewer" } }],
      { errors: [{ message: "denied" }], data: null },
    ]);
    expect(
      (
        await new GitHubReviewAdapter(errored, credentials().review).observe(
          request,
          pull.value,
        )
      ).ok,
    ).toBe(false);
  });

  it("observes exact-SHA check runs using only CI permission", async () => {
    const client = new StubClient([
      {
        total_count: 2,
        check_runs: [
          {
            head_sha: "a".repeat(40),
            status: "completed",
            conclusion: "success",
          },
          {
            head_sha: "a".repeat(40),
            status: "completed",
            conclusion: "success",
          },
        ],
      },
    ]);
    const result = await new GitHubCiAdapter(client, credentials().ci).observe(
      request,
    );
    expect(result).toEqual({
      ok: true,
      value: { headRevision: request.headRevision, ciStatus: "success" },
    });
    expect(client.requests[0]?.credential).toBe("ci-token");
  });

  it("keeps permission observation and auto-merge on the merge credential", async () => {
    const client = new StubClient([
      { permission: "write" },
      { merged: false, head: { sha: "a".repeat(40) } },
      {
        data: {
          enablePullRequestAutoMerge: { pullRequest: { id: "PR_node" } },
        },
      },
    ]);
    const pullClient = new StubClient([pullResponse]);
    const pull = await new GitHubPullRequestAdapter(
      pullClient,
      credentials().pullRequest,
    ).create(request);
    if (!pull.ok) throw new Error("fixture invalid");
    const adapter = new GitHubMergeAdapter(client, credentials().merge);
    expect(await adapter.observeAuthorPermission(request, pull.value)).toEqual({
      ok: true,
      value: "granted",
    });
    expect(await adapter.observeMerge(request, pull.value)).toEqual({
      ok: true,
      value: "open",
    });
    expect(await adapter.enableAutoMerge(request, pull.value)).toEqual({
      ok: true,
      value: undefined,
    });
    expect(
      client.requests.every(({ credential }) => credential === "merge-token"),
    ).toBe(true);
  });

  it("rejects missing or malformed independent credentials", () => {
    for (const parser of [
      parseGitHubPullRequestCredential,
      parseGitHubReviewCredential,
      parseGitHubCiCredential,
      parseGitHubMergeCredential,
    ]) {
      expect(parser(undefined).ok).toBe(false);
      expect(parser("").ok).toBe(false);
      expect(parser("x").ok).toBe(true);
    }
  });

  it("fails closed on wrong revision, pagination, or malformed GitHub data", async () => {
    const wrongPull = new StubClient([
      { ...pullResponse, head: { sha: "b".repeat(40) } },
    ]);
    expect(
      (
        await new GitHubPullRequestAdapter(
          wrongPull,
          credentials().pullRequest,
        ).create(request)
      ).ok,
    ).toBe(false);

    const paginated = new StubClient([
      [{ state: "APPROVED", user: { login: "reviewer" } }],
      {
        data: {
          node: {
            reviewThreads: { pageInfo: { hasNextPage: true }, nodes: [] },
          },
        },
      },
    ]);
    const pullClient = new StubClient([pullResponse]);
    const pull = await new GitHubPullRequestAdapter(
      pullClient,
      credentials().pullRequest,
    ).create(request);
    if (!pull.ok) throw new Error("fixture invalid");
    expect(
      (
        await new GitHubReviewAdapter(paginated, credentials().review).observe(
          request,
          pull.value,
        )
      ).ok,
    ).toBe(false);

    const wrongCi = new StubClient([
      {
        total_count: 1,
        check_runs: [
          {
            head_sha: "b".repeat(40),
            status: "completed",
            conclusion: "success",
          },
        ],
      },
    ]);
    expect(
      (await new GitHubCiAdapter(wrongCi, credentials().ci).observe(request))
        .ok,
    ).toBe(false);
  });
});
