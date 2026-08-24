import { createHash } from "node:crypto";
import {
  context7CacheKey,
  parseContext7Documentation,
  parseContext7Libraries,
  type Context7Documentation,
  type Context7Failure,
  type Context7Library,
  type Context7NetworkCapability,
  type Context7QueryRequest,
  type Context7ResolveRequest,
  type Context7Result,
} from "../../core/context/context7.js";
import { fail, succeed } from "../../core/failures/tiber-failure.js";

interface Provenance {
  readonly endpoint: Context7NetworkCapability["endpoint"];
  readonly responseDigest: string;
}
export interface Context7Resolution {
  readonly libraries: readonly Context7Library[];
  readonly source: Provenance;
  readonly cache: "hit" | "miss";
}
export interface Context7Docs {
  readonly documentation: Context7Documentation;
  readonly libraryId: Context7QueryRequest["libraryId"];
  readonly source: Provenance;
  readonly cache: "hit" | "miss";
}
type Cached =
  | {
      readonly kind: "resolve";
      readonly value: Omit<Context7Resolution, "cache">;
    }
  | { readonly kind: "docs"; readonly value: Omit<Context7Docs, "cache"> };

function adapterFailure(
  code: Context7Failure["code"],
  message: string,
): Context7Result<never> {
  const recovery =
    code === "TIBER_CONTEXT7_RESPONSE_OVERSIZED" ||
    code === "TIBER_CONTEXT7_RESPONSE_INVALID"
      ? "valid-context7-response"
      : "retry-context7";
  return fail({
    code,
    message,
    safeContext: { domain: "context7" },
    causes: [],
    retryability:
      code === "TIBER_CONTEXT7_HTTP_FAILED" ? "transient" : "retry-after-input",
    requiredRecoveryEvidence: [recovery],
    redaction: "public",
  });
}

export class Context7HttpService {
  private readonly cache = new Map<string, Cached>();
  public constructor(
    private readonly capability: Context7NetworkCapability,
    private readonly apiKey?: string,
  ) {}

  private cacheValue(key: string, value: Cached): void {
    if (this.cache.size >= 32) {
      const oldest = this.cache.keys().next().value;
      if (typeof oldest === "string") this.cache.delete(oldest);
    }
    this.cache.set(key, value);
  }

  private async get(
    path: string,
    signal?: AbortSignal,
  ): Promise<
    Context7Result<{ readonly document: unknown; readonly digest: string }>
  > {
    const boundedSignal = AbortSignal.any([
      AbortSignal.timeout(this.capability.timeoutMs),
      ...(signal === undefined ? [] : [signal]),
    ]);
    try {
      const response = await fetch(`${this.capability.endpoint}${path}`, {
        headers: {
          accept: "application/json",
          ...(this.apiKey === undefined
            ? {}
            : { authorization: `Bearer ${this.apiKey}` }),
        },
        redirect: "error",
        signal: boundedSignal,
      });
      if (!response.ok)
        return adapterFailure(
          "TIBER_CONTEXT7_HTTP_FAILED",
          "Context7 request did not return success",
        );
      const reader = response.body?.getReader();
      if (reader === undefined)
        return adapterFailure(
          "TIBER_CONTEXT7_HTTP_FAILED",
          "Context7 response body was unavailable",
        );
      const chunks: Uint8Array[] = [];
      let length = 0;
      let reading = true;
      while (reading) {
        const untrustedPart: unknown = await reader.read();
        if (
          typeof untrustedPart !== "object" ||
          untrustedPart === null ||
          !("done" in untrustedPart)
        )
          return adapterFailure(
            "TIBER_CONTEXT7_RESPONSE_INVALID",
            "Context7 response stream was invalid",
          );
        if (untrustedPart.done === true) {
          reading = false;
          continue;
        }
        if (
          !("value" in untrustedPart) ||
          !(untrustedPart.value instanceof Uint8Array)
        )
          return adapterFailure(
            "TIBER_CONTEXT7_RESPONSE_INVALID",
            "Context7 response stream was invalid",
          );
        length += untrustedPart.value.byteLength;
        if (length > this.capability.maximumResponseBytes) {
          await reader.cancel();
          return adapterFailure(
            "TIBER_CONTEXT7_RESPONSE_OVERSIZED",
            "Context7 response exceeded its hard byte bound",
          );
        }
        chunks.push(untrustedPart.value);
      }
      const bytes = Buffer.concat(chunks);
      let document: unknown;
      try {
        document = JSON.parse(bytes.toString("utf8"));
      } catch {
        return adapterFailure(
          "TIBER_CONTEXT7_RESPONSE_INVALID",
          "Context7 response was not valid JSON",
        );
      }
      return succeed({
        document,
        digest: `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
      });
    } catch {
      return adapterFailure(
        "TIBER_CONTEXT7_HTTP_FAILED",
        "Context7 request failed or timed out",
      );
    }
  }

  public async resolveLibrary(
    request: Context7ResolveRequest,
    signal?: AbortSignal,
  ): Promise<Context7Result<Context7Resolution>> {
    const key = context7CacheKey("resolve", this.capability.endpoint, request);
    const cached = this.cache.get(key);
    if (cached?.kind === "resolve")
      return succeed({ ...cached.value, cache: "hit" });
    const response = await this.get(
      `/libs/search?libraryName=${encodeURIComponent(request.libraryName)}&query=${encodeURIComponent(request.query)}`,
      signal,
    );
    if (!response.ok) return response;
    const libraries = parseContext7Libraries(response.value.document);
    if (!libraries.ok) return libraries;
    const value = {
      libraries: libraries.value,
      source: {
        endpoint: this.capability.endpoint,
        responseDigest: response.value.digest,
      },
    };
    this.cacheValue(key, { kind: "resolve", value });
    return succeed({ ...value, cache: "miss" });
  }

  public async queryDocs(
    request: Context7QueryRequest,
    signal?: AbortSignal,
  ): Promise<Context7Result<Context7Docs>> {
    const key = context7CacheKey("docs", this.capability.endpoint, request);
    const cached = this.cache.get(key);
    if (cached?.kind === "docs")
      return succeed({ ...cached.value, cache: "hit" });
    const response = await this.get(
      `/context?libraryId=${encodeURIComponent(request.libraryId)}&query=${encodeURIComponent(request.query)}&type=json`,
      signal,
    );
    if (!response.ok) return response;
    const documentation = parseContext7Documentation(
      response.value.document,
      request.libraryId,
    );
    if (!documentation.ok) return documentation;
    const value = {
      documentation: documentation.value,
      libraryId: request.libraryId,
      source: {
        endpoint: this.capability.endpoint,
        responseDigest: response.value.digest,
      },
    };
    this.cacheValue(key, { kind: "docs", value });
    return succeed({ ...value, cache: "miss" });
  }
}
