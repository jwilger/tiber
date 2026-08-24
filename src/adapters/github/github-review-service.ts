import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import type {
  PullRequestReceipt,
  PullRequestService,
  ReviewCiService,
  ReviewMergePermission,
  ReviewMergeService,
  ReviewMergeStatus,
  ReviewObservationService,
  ReviewRequest,
  ReviewServiceFailure,
  ReviewServiceResult,
} from "../../core/reviews/review-service.js";
import {
  parseReviewAuthorLogin,
  parseReviewNodeId,
  parseReviewNumber,
  parseReviewRevision,
  parseReviewUrl,
} from "../../core/reviews/review-service-values.js";

export type GitHubPullRequestCredential = string & {
  readonly __brand: "GitHubPullRequestCredential";
};
export type GitHubReviewCredential = string & {
  readonly __brand: "GitHubReviewCredential";
};
export type GitHubCiCredential = string & {
  readonly __brand: "GitHubCiCredential";
};
export type GitHubMergeCredential = string & {
  readonly __brand: "GitHubMergeCredential";
};
export type GitHubCredential =
  | GitHubPullRequestCredential
  | GitHubReviewCredential
  | GitHubCiCredential
  | GitHubMergeCredential;

type CredentialFailure = TiberFailure<
  "TIBER_GITHUB_CREDENTIAL_INVALID",
  { readonly domain: "github-credential" },
  "corrected-input" | "state-change" | "retry-operation"
>;
type CredentialResult<T> = TiberResult<T, CredentialFailure>;

function parseCredential<T extends GitHubCredential>(
  input: unknown,
): CredentialResult<T> {
  return typeof input === "string" &&
    input.length >= 1 &&
    input.length <= 4_096 &&
    !input.includes("\0")
    ? { ok: true, value: input as T }
    : {
        ok: false,
        failure: operationalFailure(
          "TIBER_GITHUB_CREDENTIAL_INVALID",
          "github-credential",
          "GitHub credential is invalid",
          "retry-after-input",
        ),
      };
}

export const parseGitHubPullRequestCredential = (
  input: unknown,
): CredentialResult<GitHubPullRequestCredential> => parseCredential(input);
export const parseGitHubReviewCredential = (
  input: unknown,
): CredentialResult<GitHubReviewCredential> => parseCredential(input);
export const parseGitHubCiCredential = (
  input: unknown,
): CredentialResult<GitHubCiCredential> => parseCredential(input);
export const parseGitHubMergeCredential = (
  input: unknown,
): CredentialResult<GitHubMergeCredential> => parseCredential(input);

export interface GitHubHttpRequest {
  readonly method: "GET" | "POST";
  readonly path: string;
  readonly credential: GitHubCredential;
  readonly body?: unknown;
}

export interface GitHubHttpClient {
  request(input: GitHubHttpRequest): Promise<ReviewServiceResult<unknown>>;
}

function failure(
  code: ReviewServiceFailure["code"],
  message: string,
): ReviewServiceResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "review-service",
      message,
      code === "TIBER_REVIEW_SERVICE_FAILED"
        ? "transient"
        : code === "TIBER_REVIEW_SERVICE_PERMISSION_MISSING"
          ? "retry-after-state-change"
          : "retry-after-input",
    ),
  };
}

function record(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function repositoryPath(request: ReviewRequest): string {
  return `/repos/${encodeURIComponent(request.repositoryOwner)}/${encodeURIComponent(request.repositoryName)}`;
}

export class GitHubPullRequestAdapter implements PullRequestService {
  public constructor(
    private readonly client: GitHubHttpClient,
    private readonly credential: GitHubPullRequestCredential,
  ) {}

  public async create(
    request: ReviewRequest,
  ): Promise<ReviewServiceResult<PullRequestReceipt>> {
    const response = await this.client.request({
      method: "POST",
      path: `${repositoryPath(request)}/pulls`,
      credential: this.credential,
      body: {
        head: request.headRef.slice("refs/heads/".length),
        base: request.baseRef,
        title: request.title,
        body: request.body,
      },
    });
    if (!response.ok) return response;
    const value = response.value;
    if (!record(value) || !record(value.user) || !record(value.head))
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub pull request response is invalid",
      );
    const number = parseReviewNumber(value.number);
    const nodeId = parseReviewNodeId(value.node_id);
    const url = parseReviewUrl(value.html_url);
    const author = parseReviewAuthorLogin(value.user.login);
    const headRevision = parseReviewRevision(value.head.sha);
    if (
      !number.ok ||
      !nodeId.ok ||
      !url.ok ||
      !author.ok ||
      !headRevision.ok ||
      headRevision.value !== request.headRevision
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub pull request identity is invalid",
      );
    return {
      ok: true,
      value: {
        number: number.value,
        nodeId: nodeId.value,
        url: url.value,
        author: author.value,
        headRevision: headRevision.value,
      },
    };
  }
}

export class GitHubReviewAdapter implements ReviewObservationService {
  public constructor(
    private readonly client: GitHubHttpClient,
    private readonly credential: GitHubReviewCredential,
  ) {}

  public async observe(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<
    ReviewServiceResult<{
      readonly headRevision: typeof request.headRevision;
      readonly reviewStatus: "approved" | "pending" | "changes-requested";
      readonly conversationStatus: "resolved" | "unresolved";
    }>
  > {
    const reviews = await this.client.request({
      method: "GET",
      path: `${repositoryPath(request)}/pulls/${String(pullRequest.number)}/reviews?per_page=100`,
      credential: this.credential,
    });
    if (!reviews.ok) return reviews;
    const threads = await this.client.request({
      method: "POST",
      path: "/graphql",
      credential: this.credential,
      body: {
        query:
          "query($id:ID!){node(id:$id){... on PullRequest{reviewThreads(first:100){pageInfo{hasNextPage} nodes{isResolved}}}}}",
        variables: { id: pullRequest.nodeId },
      },
    });
    if (!threads.ok) return threads;
    if (!Array.isArray(reviews.value) || reviews.value.length >= 100)
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub review response is incomplete",
      );
    const latestStateByReviewer = new Map<string, string>();
    for (const review of reviews.value) {
      if (
        !record(review) ||
        typeof review.state !== "string" ||
        !record(review.user) ||
        typeof review.user.login !== "string"
      )
        return failure(
          "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
          "GitHub review response is invalid",
        );
      latestStateByReviewer.set(review.user.login, review.state);
    }
    const states = [...latestStateByReviewer.values()];
    const threadRoot = threads.value;
    const reviewThreads =
      record(threadRoot) &&
      !("errors" in threadRoot) &&
      record(threadRoot.data) &&
      record(threadRoot.data.node) &&
      record(threadRoot.data.node.reviewThreads)
        ? threadRoot.data.node.reviewThreads
        : undefined;
    if (
      reviewThreads === undefined ||
      !record(reviewThreads.pageInfo) ||
      typeof reviewThreads.pageInfo.hasNextPage !== "boolean" ||
      reviewThreads.pageInfo.hasNextPage ||
      !Array.isArray(reviewThreads.nodes) ||
      reviewThreads.nodes.some(
        (thread) => !record(thread) || typeof thread.isResolved !== "boolean",
      )
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub conversation response is invalid or incomplete",
      );
    const reviewStatus = states.includes("CHANGES_REQUESTED")
      ? "changes-requested"
      : states.includes("APPROVED")
        ? "approved"
        : "pending";
    return {
      ok: true,
      value: {
        headRevision: pullRequest.headRevision,
        reviewStatus,
        conversationStatus: reviewThreads.nodes.every(
          (thread) => record(thread) && thread.isResolved === true,
        )
          ? "resolved"
          : "unresolved",
      },
    };
  }
}

export class GitHubCiAdapter implements ReviewCiService {
  public constructor(
    private readonly client: GitHubHttpClient,
    private readonly credential: GitHubCiCredential,
  ) {}

  public async observe(request: ReviewRequest): Promise<
    ReviewServiceResult<{
      readonly headRevision: typeof request.headRevision;
      readonly ciStatus: "success" | "pending" | "failure";
    }>
  > {
    const response = await this.client.request({
      method: "GET",
      path: `${repositoryPath(request)}/commits/${request.headRevision}/check-runs?per_page=100`,
      credential: this.credential,
    });
    if (!response.ok) return response;
    const value = response.value;
    if (
      !record(value) ||
      !Number.isSafeInteger(value.total_count) ||
      !Array.isArray(value.check_runs) ||
      value.total_count !== value.check_runs.length ||
      value.check_runs.length === 0 ||
      value.check_runs.length > 100
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub CI response is invalid or incomplete",
      );
    const runs = value.check_runs;
    if (
      runs.some(
        (run) =>
          !record(run) ||
          run.head_sha !== request.headRevision ||
          typeof run.status !== "string" ||
          (run.conclusion !== null && typeof run.conclusion !== "string"),
      )
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub CI check identity is invalid",
      );
    const ciStatus = runs.some(
      (run) =>
        record(run) &&
        run.status === "completed" &&
        run.conclusion !== "success",
    )
      ? "failure"
      : runs.every(
            (run) =>
              record(run) &&
              run.status === "completed" &&
              run.conclusion === "success",
          )
        ? "success"
        : "pending";
    return {
      ok: true,
      value: { headRevision: request.headRevision, ciStatus },
    };
  }
}

export class GitHubMergeAdapter implements ReviewMergeService {
  public constructor(
    private readonly client: GitHubHttpClient,
    private readonly credential: GitHubMergeCredential,
  ) {}

  public async observeMerge(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<ReviewMergeStatus>> {
    const response = await this.client.request({
      method: "GET",
      path: `${repositoryPath(request)}/pulls/${String(pullRequest.number)}`,
      credential: this.credential,
    });
    if (!response.ok) return response;
    if (
      !record(response.value) ||
      typeof response.value.merged !== "boolean" ||
      !record(response.value.head) ||
      response.value.head.sha !== request.headRevision
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub merge observation is invalid",
      );
    return {
      ok: true,
      value: response.value.merged ? "merged" : "open",
    };
  }

  public async observeAuthorPermission(
    request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<ReviewMergePermission>> {
    const response = await this.client.request({
      method: "GET",
      path: `${repositoryPath(request)}/collaborators/${encodeURIComponent(pullRequest.author)}/permission`,
      credential: this.credential,
    });
    if (!response.ok) return response;
    if (
      !record(response.value) ||
      typeof response.value.permission !== "string"
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub permission response is invalid",
      );
    return {
      ok: true,
      value: ["admin", "maintain", "write"].includes(response.value.permission)
        ? "granted"
        : "missing",
    };
  }

  public async enableAutoMerge(
    _request: ReviewRequest,
    pullRequest: PullRequestReceipt,
  ): Promise<ReviewServiceResult<void>> {
    const response = await this.client.request({
      method: "POST",
      path: "/graphql",
      credential: this.credential,
      body: {
        query:
          "mutation($id:ID!){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:SQUASH}){pullRequest{id}}}",
        variables: { id: pullRequest.nodeId },
      },
    });
    if (!response.ok) return response;
    const value = response.value;
    if (
      !record(value) ||
      "errors" in value ||
      !record(value.data) ||
      !record(value.data.enablePullRequestAutoMerge) ||
      !record(value.data.enablePullRequestAutoMerge.pullRequest) ||
      value.data.enablePullRequestAutoMerge.pullRequest.id !==
        pullRequest.nodeId
    )
      return failure(
        "TIBER_REVIEW_SERVICE_RESPONSE_INVALID",
        "GitHub auto-merge response is invalid",
      );
    return { ok: true, value: undefined };
  }
}

export class FetchGitHubHttpClient implements GitHubHttpClient {
  public constructor(private readonly endpoint = "https://api.github.com") {}

  public async request(
    input: GitHubHttpRequest,
  ): Promise<ReviewServiceResult<unknown>> {
    const controller = new AbortController();
    const timeout = setTimeout(() => {
      controller.abort();
    }, 30_000);
    try {
      const response = await fetch(`${this.endpoint}${input.path}`, {
        method: input.method,
        redirect: "error",
        signal: controller.signal,
        headers: {
          Accept: "application/vnd.github+json",
          Authorization: `Bearer ${input.credential}`,
          "Content-Type": "application/json",
          "X-GitHub-Api-Version": "2022-11-28",
        },
        ...(input.body === undefined
          ? {}
          : { body: JSON.stringify(input.body) }),
      });
      if (!response.ok)
        return response.status === 401 || response.status === 403
          ? failure(
              "TIBER_REVIEW_SERVICE_PERMISSION_MISSING",
              "GitHub permission is missing",
            )
          : failure("TIBER_REVIEW_SERVICE_FAILED", "GitHub request failed");
      const value: unknown = await response.json();
      return { ok: true, value };
    } catch {
      return failure("TIBER_REVIEW_SERVICE_FAILED", "GitHub request failed");
    } finally {
      clearTimeout(timeout);
    }
  }
}
