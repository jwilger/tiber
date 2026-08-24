import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { FileCiAuthorityStore } from "../../src/adapters/ci/file-ci-authority-store.js";
import { observeCiAuthority } from "../../src/adapters/ci/user-local-ci-authority.js";
import { decideCiEvaluation } from "../../src/core/ci/ci-authority.js";
import {
  parseCiDiagnosis,
  parseCiExecutableDigest,
  parseCiObservationDigest,
  parseCiRevision,
} from "../../src/core/ci/ci-values.js";

const temporaryDirectories: string[] = [];
afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

describe("digest-pinned CI authorities", () => {
  it("requires all providers at the exact delivered revision and shares recovery hold across worktrees", () => {
    const temporary = mkdtempSync(join(tmpdir(), "tiber-ci-"));
    temporaryDirectories.push(temporary);
    const repository = join(temporary, "repository");
    const worktree = join(temporary, "worktree");
    const agent = join(temporary, "agent");
    mkdirSync(repository);
    mkdirSync(join(agent, "tiber"), { recursive: true });
    execFileSync("git", ["init", "--quiet"], { cwd: repository });
    execFileSync("git", ["config", "user.email", "ci@example.test"], {
      cwd: repository,
    });
    execFileSync("git", ["config", "user.name", "CI Test"], {
      cwd: repository,
    });
    writeFileSync(join(repository, "README.md"), "fixture\n");
    execFileSync("git", ["add", "README.md"], { cwd: repository });
    execFileSync("git", ["commit", "--quiet", "-m", "test: fixture"], {
      cwd: repository,
    });
    const revisionText = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repository,
      encoding: "utf8",
    }).trim();
    const revision = parseCiRevision(revisionText);
    if (!revision.ok) throw new Error("fixture revision invalid");
    execFileSync(
      "git",
      ["worktree", "add", "--quiet", "-b", "ci-worktree", worktree],
      { cwd: repository },
    );

    const executable = join(agent, "ci-observer.mjs");
    const script =
      "#!/usr/bin/env node\nconst [authority, status, revision] = process.argv.slice(2);\nprocess.stdout.write(JSON.stringify({schemaVersion:1,authority,revision,status}));\n";
    writeFileSync(executable, script);
    chmodSync(executable, 0o700);
    const executableSha256 = createHash("sha256").update(script).digest("hex");
    writeFileSync(
      join(agent, "tiber", "ci-authorities.v1.json"),
      JSON.stringify({
        schemaVersion: 1,
        authorities: ["quality", "acceptance"].map((name) => ({
          name,
          executable,
          executableSha256,
          argv: [name, "failure", "{revision}"],
        })),
      }),
    );

    const store = new FileCiAuthorityStore(repository, agent);
    const catalog = store.loadCatalog();
    if (!catalog.ok) throw new Error(catalog.failure.code);
    const observations = catalog.value.authorities.map((definition) =>
      observeCiAuthority(definition, revision.value),
    );
    expect(observations.every((result) => result.ok)).toBe(true);

    const quality = catalog.value.authorities[0];
    const acceptance = catalog.value.authorities[1];
    if (quality === undefined || acceptance === undefined)
      throw new Error("fixture catalog invalid");
    const observationDigest = parseCiObservationDigest(executableSha256);
    const adapterDigest = parseCiExecutableDigest(executableSha256);
    if (!observationDigest.ok || !adapterDigest.ok)
      throw new Error("fixture digest invalid");
    const failedDecision = decideCiEvaluation(
      revision.value,
      [quality.name, acceptance.name],
      [
        {
          authority: quality.name,
          revision: revision.value,
          status: "failure",
          adapterDigest: adapterDigest.value,
          observationDigest: observationDigest.value,
        },
        {
          authority: acceptance.name,
          revision: revision.value,
          status: "success",
          adapterDigest: adapterDigest.value,
          observationDigest: observationDigest.value,
        },
      ],
    );
    if (failedDecision.status !== "failed")
      throw new Error("expected failure hold");
    expect(store.recordHold(failedDecision.hold).ok).toBe(true);
    expect(store.recordHold(failedDecision.hold).ok).toBe(true);
    const conflictingRevision = parseCiRevision("f".repeat(40));
    if (!conflictingRevision.ok) throw new Error("fixture revision invalid");
    expect(
      store.recordHold({
        ...failedDecision.hold,
        failedRevision: conflictingRevision.value,
      }).ok,
    ).toBe(false);
    const linkedStore = new FileCiAuthorityStore(worktree, agent);
    expect(linkedStore.readHold()).toMatchObject({
      ok: true,
      value: { kind: "some" },
    });

    const success = decideCiEvaluation(
      revision.value,
      [quality.name, acceptance.name],
      [
        {
          authority: quality.name,
          revision: revision.value,
          status: "success",
          adapterDigest: adapterDigest.value,
          observationDigest: observationDigest.value,
        },
        {
          authority: acceptance.name,
          revision: revision.value,
          status: "success",
          adapterDigest: adapterDigest.value,
          observationDigest: observationDigest.value,
        },
      ],
    );
    const diagnosis = parseCiDiagnosis(
      "The failed provider was repaired and the exact revision was rerun.",
    );
    if (success.status !== "succeeded" || !diagnosis.ok)
      throw new Error("recovery fixture invalid");
    expect(linkedStore.recoverHold(diagnosis.value, success.receipt).ok).toBe(
      true,
    );
    expect(store.readHold()).toEqual({ ok: true, value: { kind: "none" } });
  });
});
