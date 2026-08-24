import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { deliverGit } from "../../src/adapters/git/git-delivery.js";
import { observeSourceSnapshot } from "../../src/adapters/git/git-source-diff.js";
import {
  parseDeliveryCommitBody,
  parseDeliveryCommitSubject,
  parseDeliveryDestinationRef,
} from "../../src/core/delivery/git-delivery-values.js";
import { some } from "../../src/core/types/option.js";
import { claimBaselineRevision } from "../fixtures/task-values.js";
import { ownedWorktreePath } from "../fixtures/worktree-values.js";

function git(cwd: string, args: readonly string[]): string {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}
function required<Value>(
  result: { ok: true; value: Value } | { ok: false },
): Value {
  if (!result.ok) throw new Error("invalid delivery fixture");
  return result.value;
}

describe("generic signed Git delivery", () => {
  it("pushes and observes the exact signed commit and rejects non-fast-forward delivery", () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-delivery-"));
    const remote = join(root, "remote.git");
    const repository = join(root, "repository");
    const key = join(root, "signing-key");
    const allowed = join(root, "allowed-signers");
    mkdirSync(remote);
    git(remote, ["init", "--bare", "--quiet"]);
    execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", key]);
    writeFileSync(
      allowed,
      `delivery@example.test ${readFileSync(`${key}.pub`, "utf8").trim()}\n`,
    );
    git(root, ["clone", "--quiet", remote, repository]);
    const configuration: readonly (readonly [string, string])[] = [
      ["user.name", "Delivery Test"],
      ["user.email", "delivery@example.test"],
      ["user.signingkey", key],
      ["gpg.format", "ssh"],
      ["gpg.ssh.allowedSignersFile", allowed],
    ];
    for (const [name, value] of configuration)
      git(repository, ["config", name, value]);
    writeFileSync(join(repository, "source.ts"), "export const value = 1;\n");
    git(repository, ["add", "--all"]);
    git(repository, [
      "commit",
      "-S",
      "-m",
      "test: baseline",
      "-m",
      "Create exact baseline.",
    ]);
    const baseline = claimBaselineRevision(
      git(repository, ["rev-parse", "HEAD"]),
    );
    git(repository, ["push", "origin", "HEAD:refs/heads/main"]);
    writeFileSync(join(repository, "source.ts"), "export const value = 2;\n");
    const worktree = ownedWorktreePath(repository);
    const snapshot = observeSourceSnapshot(worktree, baseline);
    if (!snapshot.ok) throw new Error("snapshot failed");
    const destination = required(
      parseDeliveryDestinationRef("refs/heads/feature/exact"),
    );
    const exactInput = {
      worktree,
      baselineRevision: baseline,
      mode: "branch-push" as const,
      destination: some(destination),
      subject: required(
        parseDeliveryCommitSubject("feat: deliver exact source"),
      ),
      body: required(
        parseDeliveryCommitBody("Preserve the reviewed source exactly."),
      ),
      sourceSnapshotDigest: snapshot.value,
    };
    writeFileSync(join(repository, "source.ts"), "export const value = 3;\n");
    expect(deliverGit(exactInput)).toMatchObject({
      ok: false,
      failure: { code: "TIBER_DELIVERY_OBSERVATION_INVALID" },
    });
    writeFileSync(join(repository, "source.ts"), "export const value = 2;\n");
    const delivered = deliverGit({
      ...exactInput,
    });
    expect(delivered).toMatchObject({
      ok: true,
      value: { mode: "branch-push", observedRemoteCommit: { kind: "some" } },
    });
    if (!delivered.ok) return;
    expect(git(repository, ["verify-commit", delivered.value.commit])).toBe("");
    expect(
      git(repository, [
        "show",
        "--no-show-signature",
        "-s",
        "--format=%s%n%b",
        delivered.value.commit,
      ]),
    ).toBe("feat: deliver exact source\nPreserve the reviewed source exactly.");
    expect(git(repository, ["ls-remote", "origin", destination])).toContain(
      delivered.value.commit,
    );

    git(repository, ["reset", "--hard", baseline]);
    writeFileSync(join(repository, "other.ts"), "export const other = true;\n");
    const changed = observeSourceSnapshot(worktree, baseline);
    if (!changed.ok) throw new Error("changed snapshot failed");
    expect(
      deliverGit({
        worktree,
        baselineRevision: baseline,
        mode: "branch-push",
        destination: some(destination),
        subject: required(parseDeliveryCommitSubject("fix: diverge delivery")),
        body: required(
          parseDeliveryCommitBody("Create a deliberately divergent commit."),
        ),
        sourceSnapshotDigest: changed.value,
      }),
    ).toMatchObject({
      ok: false,
      failure: { code: "TIBER_DELIVERY_NON_FAST_FORWARD" },
    });
  });
});
