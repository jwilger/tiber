import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { GitTaskRemote } from "../../src/adapters/tasks/git-task-remote.js";
import type { TaskCreatedEvent } from "../../src/core/tasks/task-board.js";

const temporaryDirectories: string[] = [];
afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

function git(cwd: string, args: string[]): void {
  execFileSync("git", args, { cwd, stdio: "ignore" });
}

function event(
  eventId: string,
  taskId: string,
  title: string,
): TaskCreatedEvent {
  return {
    schemaVersion: 1,
    eventId,
    kind: "task-created",
    occurredAt: "2026-08-23T00:00:00.000Z",
    task: { id: taskId, title, description: "" },
  };
}

describe("signed shared task ref", () => {
  it("reconciles concurrent clone publication without force or task loss", async () => {
    const root = mkdtempSync(join(tmpdir(), "tiber-task-ref-"));
    temporaryDirectories.push(root);
    const remote = join(root, "remote.git");
    const key = join(root, "signing-key");
    const allowed = join(root, "allowed-signers");
    mkdirSync(remote);
    git(remote, ["init", "--bare", "--quiet"]);
    execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", key]);
    const publicKey = readFileSync(`${key}.pub`, "utf8").trim();
    writeFileSync(allowed, `task@example.test ${publicKey}\n`);

    const clones = [join(root, "a"), join(root, "b")];
    for (const clone of clones) {
      git(root, ["clone", "--quiet", remote, clone]);
      for (const [name, value] of [
        ["user.name", "Task Test"],
        ["user.email", "task@example.test"],
        ["user.signingkey", key],
        ["gpg.format", "ssh"],
        ["gpg.ssh.allowedSignersFile", allowed],
      ] as const)
        git(clone, ["config", name, value]);
    }

    const [left, right] = await Promise.all([
      Promise.resolve().then(() =>
        new GitTaskRemote(clones[0] ?? "").publish(
          event(
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "Left",
          ),
        ),
      ),
      Promise.resolve().then(() =>
        new GitTaskRemote(clones[1] ?? "").publish(
          event(
            "33333333-3333-4333-8333-333333333333",
            "44444444-4444-4444-8444-444444444444",
            "Right",
          ),
        ),
      ),
    ]);
    expect([left.mode, right.mode]).toContain("writable");
    const board = new GitTaskRemote(clones[0] ?? "").read();
    expect(board.mode).toBe("writable");
    expect(board.tasks.map((task) => task.title).sort()).toEqual([
      "Left",
      "Right",
    ]);

    const attacker = join(root, "attacker");
    git(root, ["clone", "--quiet", remote, attacker]);
    git(attacker, ["checkout", "--quiet", "tiber/tasks/v1"]);
    writeFileSync(join(attacker, "unsigned.txt"), "rewritten authority\n");
    git(attacker, ["add", "unsigned.txt"]);
    git(attacker, [
      "-c",
      "user.name=Untrusted",
      "-c",
      "user.email=untrusted@example.test",
      "commit",
      "--quiet",
      "--no-gpg-sign",
      "-m",
      "unsigned task history",
    ]);
    git(attacker, [
      "push",
      "--quiet",
      "origin",
      "HEAD:refs/heads/tiber/tasks/v1",
    ]);
    expect(new GitTaskRemote(clones[0] ?? "").read()).toMatchObject({
      mode: "degraded-read-only",
      failure: "task history signature or event verification failed",
    });
  }, 30_000);
});
