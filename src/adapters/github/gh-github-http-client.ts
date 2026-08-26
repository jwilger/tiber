import { spawn } from "node:child_process";

import { operationalFailure } from "../../core/failures/tiber-failure.js";
import type { ReviewServiceResult } from "../../core/reviews/review-service.js";
import type {
  GitHubHttpClient,
  GitHubHttpRequest,
} from "./github-review-service.js";

interface GhResult {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
}

type GhRunner = (argv: readonly string[], input: string) => Promise<GhResult>;

function failure(message: string): ReviewServiceResult<never> {
  return {
    ok: false,
    failure: operationalFailure(
      "TIBER_REVIEW_SERVICE_FAILED",
      "review-service",
      message,
      "transient",
    ),
  };
}

function runInstalledGh(
  argv: readonly string[],
  input: string,
): Promise<GhResult> {
  return new Promise((resolvePromise) => {
    const child = spawn("gh", argv, {
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
      shell: false,
    });
    let stdout = "";
    let stderr = "";
    let overflow = false;
    const timeout = setTimeout(() => child.kill("SIGTERM"), 30_000);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (Buffer.byteLength(stdout) > 1_048_576) {
        overflow = true;
        child.kill("SIGTERM");
      }
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (Buffer.byteLength(stderr) > 65_536) stderr = stderr.slice(-65_536);
    });
    child.on("error", () => {
      clearTimeout(timeout);
      resolvePromise({ code: 1, stdout: "", stderr: "gh unavailable" });
    });
    child.on("close", (code) => {
      clearTimeout(timeout);
      resolvePromise({
        code: overflow ? 1 : (code ?? 1),
        stdout,
        stderr,
      });
    });
    child.stdin.end(input);
  });
}

export class GhGitHubHttpClient implements GitHubHttpClient {
  public constructor(private readonly run: GhRunner = runInstalledGh) {}

  public async request(
    request: GitHubHttpRequest,
  ): Promise<ReviewServiceResult<unknown>> {
    let input: string;
    try {
      input = request.body === undefined ? "" : JSON.stringify(request.body);
    } catch {
      return failure("GitHub request body is invalid");
    }
    const argv = ["api", "--method", request.method, request.path];
    if (request.body !== undefined) argv.push("--input", "-");
    const result = await this.run(argv, input);
    if (result.code !== 0)
      return failure("GitHub CLI request could not be completed");
    try {
      const value: unknown = JSON.parse(result.stdout);
      return { ok: true, value };
    } catch {
      return failure("GitHub CLI response is invalid");
    }
  }
}
