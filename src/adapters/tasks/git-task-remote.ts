import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  foldTaskEvents,
  parseTaskCreatedEvent,
  type TaskBoard,
  type TaskCreatedEvent,
} from "../../core/tasks/task-board.js";

const TASK_REF = "refs/heads/tiber/tasks/v1";

function git(cwd: string, args: readonly string[]): string {
  return execFileSync("git", [...args], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function degraded(failure: string): TaskBoard {
  return { mode: "degraded-read-only", tasks: [], failure };
}

function copySigningConfiguration(source: string, target: string): void {
  for (const key of [
    "user.name",
    "user.email",
    "user.signingkey",
    "gpg.format",
    "gpg.ssh.allowedSignersFile",
  ] as const) {
    try {
      const value = git(source, ["config", "--get", key]);
      if (value.length > 0) git(target, ["config", key, value]);
    } catch {
      continue;
    }
  }
}

function verifiedEvents(repository: string): TaskCreatedEvent[] | undefined {
  const commits = git(repository, ["rev-list", "--reverse", "HEAD"])
    .split("\n")
    .filter(Boolean);
  for (const commit of commits) {
    try {
      git(repository, ["verify-commit", commit]);
    } catch {
      return undefined;
    }
  }
  const directory = join(repository, "events");
  const events: TaskCreatedEvent[] = [];
  for (const name of readdirSync(directory).sort()) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(readFileSync(join(directory, name), "utf8"));
    } catch {
      return undefined;
    }
    const event = parseTaskCreatedEvent(parsed);
    if (event === undefined) return undefined;
    events.push(event);
  }
  return events;
}

export class GitTaskRemote {
  public constructor(private readonly cwd: string) {}

  private remoteUrl(): string {
    return git(this.cwd, ["remote", "get-url", "origin"]);
  }

  private clone(): string {
    const directory = mkdtempSync(join(tmpdir(), "tiber-tasks-"));
    git(directory, [
      "clone",
      "--quiet",
      "--no-checkout",
      this.remoteUrl(),
      "repository",
    ]);
    const repository = join(directory, "repository");
    copySigningConfiguration(this.cwd, repository);
    return repository;
  }

  public read(): TaskBoard {
    let repository: string | undefined;
    try {
      repository = this.clone();
      git(repository, [
        "fetch",
        "--quiet",
        "origin",
        `${TASK_REF}:refs/remotes/origin/tiber-tasks`,
      ]);
      git(repository, [
        "checkout",
        "--quiet",
        "--detach",
        "refs/remotes/origin/tiber-tasks",
      ]);
      const events = verifiedEvents(repository);
      return events === undefined
        ? degraded("task history signature or event verification failed")
        : foldTaskEvents(events);
    } catch {
      return degraded("signed task ref is unavailable");
    } finally {
      if (repository !== undefined)
        rmSync(join(repository, ".."), { recursive: true, force: true });
    }
  }

  public publish(event: TaskCreatedEvent): TaskBoard {
    for (let attempt = 0; attempt < 4; attempt += 1) {
      let repository: string | undefined;
      try {
        repository = this.clone();
        try {
          git(repository, [
            "fetch",
            "--quiet",
            "origin",
            `${TASK_REF}:refs/remotes/origin/tiber-tasks`,
          ]);
          git(repository, [
            "checkout",
            "--quiet",
            "-B",
            "task-events",
            "refs/remotes/origin/tiber-tasks",
          ]);
          if (verifiedEvents(repository) === undefined)
            return degraded(
              "task history signature or event verification failed",
            );
        } catch {
          git(repository, ["checkout", "--quiet", "--orphan", "task-events"]);
          git(repository, ["rm", "-rf", "--ignore-unmatch", "."]);
        }
        mkdirSync(join(repository, "events"), { recursive: true });
        writeFileSync(
          join(repository, "events", `${event.eventId}.json`),
          `${JSON.stringify(event, null, 2)}\n`,
          { encoding: "utf8", flag: "wx" },
        );
        git(repository, ["add", "events"]);
        git(repository, [
          "commit",
          "--quiet",
          "-S",
          "-m",
          `task: create ${event.task.id}`,
          "-m",
          "Publish an append-only Tiber task event.",
        ]);
        git(repository, ["push", "--quiet", "origin", `HEAD:${TASK_REF}`]);
        const board = this.read();
        if (board.mode === "writable") return board;
        return board;
      } catch {
        if (attempt === 3)
          return degraded(
            "concurrent signed task publication could not be reconciled",
          );
      } finally {
        if (repository !== undefined)
          rmSync(join(repository, ".."), { recursive: true, force: true });
      }
    }
    return degraded("task publication failed");
  }
}
