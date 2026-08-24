import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

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
    expect(authority.grant(catalog.value.digest)).toBe(true);
    const decision = decideCommandExecution(catalog.value, "large-output", {
      activeClaim: true,
      grantedCatalogDigest: authority.readGrant(),
    });
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;

    const run = await new StructuredCommandRunner(
      new FileProcessGroupRegistry(agent),
    ).run(decision.command, repository, {
      taskId: "2424c876-6180-4c64-976e-9ea4bd540744",
      claimId: "00000000-0000-4000-8000-000000000001",
    });
    expect(run.ok).toBe(true);
    if (!run.ok) return;
    const result = virtualizeCommandOutput(
      run.output,
      decision.command.maxOutputBytes,
    );
    expect(result.kind).toBe("artifact");
    if (result.kind !== "artifact") return;
    expect(Buffer.byteLength(JSON.stringify(result.preview))).toBeLessThan(512);
    const artifacts = new FileArtifactStore(agent);
    expect(artifacts.put(result)).toEqual({ ok: true, value: result.digest });
    expect(artifacts.search(result.digest, "result-499", 5)).toEqual({
      ok: true,
      value: [{ line: 503, text: "result-499" }],
    });
    const range = artifacts.range(result.digest, 0, 64);
    expect(range).toMatchObject({
      ok: true,
      value: { offset: 0, nextOffset: 64 },
    });
    expect(artifacts.range(result.digest, -1, 1)).toMatchObject({ ok: false });
    expect(artifacts.search(result.digest, "", 1)).toMatchObject({ ok: false });
    expect(new FileProcessGroupRegistry(agent).read()).toEqual({
      ok: true,
      value: [],
    });

    const hash = result.digest.slice("sha256:".length);
    const path = join(agent, "tiber", "artifacts", "sha256", `${hash}.txt`);
    utimesSync(path, new Date(0), new Date(0));
    expect(artifacts.reap(Date.now())).toEqual({ ok: true, value: 1 });
    expect(artifacts.read(result.digest)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_ARTIFACT_NOT_FOUND" },
    });
  });
});
