import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { some } from "../../src/core/types/option.js";
import {
  artifactRangeLimit,
  artifactReapAtMilliseconds,
  artifactRangeOffset,
  artifactSearchMaximumMatches,
  artifactSearchQuery,
  inlineOutputMaximumBytes,
} from "../fixtures/artifact-values.js";
import { commandName } from "../fixtures/command-values.js";
import { taskClaimId, taskId } from "../fixtures/task-values.js";

import { FileArtifactStore } from "../../src/adapters/artifacts/file-artifact-store.js";
import { FileCommandAuthority } from "../../src/adapters/commands/file-command-authority.js";
import { StructuredCommandRunner } from "../../src/adapters/commands/structured-command-runner.js";
import { FileProcessGroupRegistry } from "../../src/adapters/processes/file-process-group-registry.js";
import { virtualizeCommandOutput } from "../../src/core/artifacts/output-virtualization.js";
import { decideCommandExecution } from "../../src/core/commands/structured-command.js";

function git(cwd: string, args: readonly string[]): void {
  execFileSync("git", [...args], { cwd, stdio: "ignore" });
}

describe("structured command artifact boundary", () => {
  it("runs a locally granted argv command and virtualizes oversized output", async () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-command-"));
    const repository = join(root, "repository");
    const agent = join(root, "agent");
    git(root, ["init", repository]);
    mkdirSync(join(repository, ".tiber"));
    const definition = {
      schemaVersion: 1,
      commands: [
        {
          name: "large-output",
          executable: process.execPath,
          purpose: "test",
          argv: [
            "-e",
            "for(let i=0;i<500;i++) process.stdout.write(`result-${i}\\n`)",
          ],
          cwd: "worktree",
          environment: {},
          timeoutMs: 10_000,
          maxOutputBytes: 128,
        },
      ],
    };
    writeFileSync(
      join(repository, ".tiber", "commands.json"),
      JSON.stringify(definition),
    );
    const authority = new FileCommandAuthority(repository);
    const catalog = authority.loadCatalog();
    expect(catalog.ok).toBe(true);
    if (!catalog.ok) return;
    expect(authority.grant(catalog.value.digest)).toEqual({
      ok: true,
      value: undefined,
    });
    const grant = authority.readGrant();
    expect(grant.ok).toBe(true);
    if (!grant.ok) return;
    const decision = decideCommandExecution(
      catalog.value,
      commandName("large-output"),
      {
        claimStatus: "published",
        grantedCatalogDigest: grant.value,
      },
    );
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;

    const run = await new StructuredCommandRunner(
      new FileProcessGroupRegistry(agent),
    ).run(decision.command, repository, {
      taskId: taskId("2424c876-6180-4c64-976e-9ea4bd540744"),
      claimId: taskClaimId("00000000-0000-4000-8000-000000000001"),
    });
    expect(run.ok).toBe(true);
    if (!run.ok) return;
    const result = virtualizeCommandOutput(
      run.output,
      inlineOutputMaximumBytes(decision.command.maxOutputBytes),
    );
    expect(result.kind).toBe("artifact");
    if (result.kind !== "artifact") return;
    expect(Buffer.byteLength(JSON.stringify(result.preview))).toBeLessThan(512);
    const artifacts = new FileArtifactStore(agent);
    expect(artifacts.put(result)).toEqual({
      ok: true,
      value: some(result.digest),
    });
    expect(
      artifacts.search(
        result.digest,
        artifactSearchQuery("result-499"),
        artifactSearchMaximumMatches(5),
      ),
    ).toEqual({
      ok: true,
      value: [{ line: 503, text: "result-499" }],
    });
    const range = artifacts.range(
      result.digest,
      artifactRangeOffset(0),
      artifactRangeLimit(64),
    );
    expect(range).toMatchObject({
      ok: true,
      value: { offset: 0, nextOffset: some(64) },
    });
    expect(new FileProcessGroupRegistry(agent).read()).toEqual({
      ok: true,
      value: [],
    });

    const hash = result.digest.slice("sha256:".length);
    const path = join(agent, "tiber", "artifacts", "sha256", `${hash}.txt`);
    utimesSync(path, new Date(0), new Date(0));
    expect(artifacts.reap(artifactReapAtMilliseconds(Date.now()))).toEqual({
      ok: true,
      value: 1,
    });
    expect(artifacts.read(result.digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_ARTIFACT_NOT_FOUND" },
    });
  });
});
