import { readFileSync } from "node:fs";

const path = process.argv[2];
if (path === undefined) {
  console.error("commit message path is required");
  process.exit(2);
}

const message = readFileSync(path, "utf8").trimEnd();
const [subject = "", ...bodyLines] = message.split("\n");
const conventional =
  /^(build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)(\([a-z0-9-]+\))?(!)?: .+/u;
const body = bodyLines.join("\n").trim();

if (!conventional.test(subject)) {
  console.error("commit subject must use Conventional Commit syntax");
  process.exit(1);
}

if (body.length === 0) {
  console.error("commit message must include a non-empty explanatory body");
  process.exit(1);
}
