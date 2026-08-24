import { createServer } from "node:http";

import { afterEach, describe, expect, it } from "vitest";

import {
  FetchGitHubHttpClient,
  GitHubCiAdapter,
  GitHubMergeAdapter,
  GitHubPullRequestAdapter,
  GitHubReviewAdapter,
  parseGitHubCiCredential,
  parseGitHubMergeCredential,
  parseGitHubPullRequestCredential,
  parseGitHubReviewCredential,
} from "../../src/adapters/github/github-review-service.js";
import {
  parseReviewBaseRef,
  parseReviewBody,
  parseReviewHeadRef,
  parseReviewRepositoryName,
  parseReviewRepositoryOwner,
  parseReviewRevision,
  parseReviewTitle,
} from "../../src/core/reviews/review-service-values.js";

const servers: ReturnType<typeof createServer>[] = [];
afterEach(async () => {
  await Promise.all(
    servers.splice(0).map(
      (server) =>
        new Promise<void>((resolve, reject) => {
          server.close((error) => {
            if (error) reject(error);
            else resolve();
          });
        }),
    ),
  );
});

function parsed<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  if (!result.ok) throw new Error("fixture invalid");
  return result.value;
}

describe("first-party GitHub HTTP review service", () => {
  it("uses separate credentials for PR, review, CI, and merge effects", async () => {
    const revision = "a".repeat(40);
    const observed: { authorization: string; path: string }[] = [];
    const server = createServer((incoming, response) => {
      const authorization = incoming.headers.authorization ?? "";
      const path = incoming.url ?? "";
      observed.push({ authorization, path });
      response.setHeader("Content-Type", "application/json");
      if (path === "/repos/owner/repo/pulls" && incoming.method === "POST")
        response.end(
          JSON.stringify({
            number: 42,
            node_id: "PR_node",
            html_url: "https://github.com/owner/repo/pull/42",
            user: { login: "author" },
            head: { sha: revision },
          }),
        );
      else if (path.endsWith("/reviews?per_page=100"))
        response.end(
          JSON.stringify([{ state: "APPROVED", user: { login: "reviewer" } }]),
        );
      else if (path === "/graphql" && authorization === "Bearer review-token")
        response.end(
          JSON.stringify({
            data: {
              node: {
                reviewThreads: {
                  pageInfo: { hasNextPage: false },
                  nodes: [{ isResolved: true }],
                },
              },
            },
          }),
        );
      else if (path.includes("/check-runs?per_page=100"))
        response.end(
          JSON.stringify({
            total_count: 1,
            check_runs: [
              {
                head_sha: revision,
                status: "completed",
                conclusion: "success",
              },
            ],
          }),
        );
      else if (path.includes("/collaborators/author/permission"))
        response.end(JSON.stringify({ permission: "write" }));
      else if (path === "/repos/owner/repo/pulls/42")
        response.end(
          JSON.stringify({ merged: false, head: { sha: revision } }),
        );
      else if (path === "/graphql" && authorization === "Bearer merge-token")
        response.end(
          JSON.stringify({
            data: {
              enablePullRequestAutoMerge: { pullRequest: { id: "PR_node" } },
            },
          }),
        );
      else
        response.writeHead(404).end(JSON.stringify({ message: "not found" }));
    });
    servers.push(server);
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (address === null || typeof address === "string")
      throw new Error("server did not bind");
    const client = new FetchGitHubHttpClient(
      `http://127.0.0.1:${String(address.port)}`,
    );
    const request = {
      repositoryOwner: parsed(parseReviewRepositoryOwner("owner")),
      repositoryName: parsed(parseReviewRepositoryName("repo")),
      headRef: parsed(parseReviewHeadRef("refs/heads/feat/review")),
      headRevision: parsed(parseReviewRevision(revision)),
      baseRef: parsed(parseReviewBaseRef("main")),
      title: parsed(parseReviewTitle("feat(review): add HTTP adapter")),
      body: parsed(parseReviewBody("Exact delivery.")),
    };
    const pr = new GitHubPullRequestAdapter(
      client,
      parsed(parseGitHubPullRequestCredential("pr-token")),
    );
    const opened = await pr.create(request);
    if (!opened.ok) throw new Error(opened.failure.code);
    const review = await new GitHubReviewAdapter(
      client,
      parsed(parseGitHubReviewCredential("review-token")),
    ).observe(request, opened.value);
    const ci = await new GitHubCiAdapter(
      client,
      parsed(parseGitHubCiCredential("ci-token")),
    ).observe(request);
    const merge = new GitHubMergeAdapter(
      client,
      parsed(parseGitHubMergeCredential("merge-token")),
    );
    const permission = await merge.observeAuthorPermission(
      request,
      opened.value,
    );
    const mergeStatus = await merge.observeMerge(request, opened.value);
    const enabled = await merge.enableAutoMerge(request, opened.value);

    expect([
      review.ok,
      ci.ok,
      permission.ok,
      mergeStatus.ok,
      enabled.ok,
    ]).toEqual([true, true, true, true, true]);
    expect(
      observed.filter(
        ({ authorization }) => authorization === "Bearer pr-token",
      ),
    ).toHaveLength(1);
    expect(
      observed.filter(
        ({ authorization }) => authorization === "Bearer review-token",
      ),
    ).toHaveLength(2);
    expect(
      observed.filter(
        ({ authorization }) => authorization === "Bearer ci-token",
      ),
    ).toHaveLength(1);
    expect(
      observed.filter(
        ({ authorization }) => authorization === "Bearer merge-token",
      ),
    ).toHaveLength(3);
  });
});
