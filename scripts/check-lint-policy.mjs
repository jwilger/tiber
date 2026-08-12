import fs from "node:fs";
import path from "node:path";
import process from "node:process";

function filesBelow(root, basename) {
  return fs
    .readdirSync(root, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name === basename)
    .map((entry) => path.join(entry.parentPath, entry.name))
    .sort();
}

function rustFilesBelow(root) {
  return fs
    .readdirSync(root, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
    .map((entry) => path.join(entry.parentPath, entry.name))
    .sort();
}

export function sourceViolations(source, label = "source") {
  const violations = [];
  if (/allow\s*\(\s*clippy\s*::/su.test(source)) {
    violations.push(`${label}: clippy allow attributes are forbidden`);
  }

  const conditionalAttributes = source.matchAll(
    /(#!?)\[\s*cfg_attr\s*\(([\s\S]*?)\)\s*\]/gu,
  );
  for (const attribute of conditionalAttributes) {
    const [fullAttribute, innerMarker, body] = attribute;
    if (!/\bexpect\s*\(/u.test(body)) {
      continue;
    }
    const permittedModuleExpectation =
      innerMarker === "#" &&
      /^\s*not\s*\(\s*test\s*\)\s*,\s*expect\s*\(\s*clippy::missing_docs_in_private_items\s*,\s*reason\s*=\s*"[^"]+"\s*\)\s*$/su.test(
        body,
      ) &&
      /^\s*pub\s+mod\s+[a-zA-Z_][a-zA-Z0-9_]*\s*;/u.test(
        source.slice(attribute.index + fullAttribute.length),
      );
    if (!permittedModuleExpectation) {
      violations.push(
        `${label}: conditional expect attributes are forbidden except for a reasoned non-test missing_docs_in_private_items expectation on a public module`,
      );
    }
  }

  const expectations = source.matchAll(/#!?\[\s*expect\s*\(([\s\S]*?)\)\s*\]/gu);
  for (const expectation of expectations) {
    const body = expectation[1];
    const reasonIndex = body.search(/\breason\s*=/u);
    if (reasonIndex < 0) {
      violations.push(`${label}: expect attribute lacks a reason`);
      continue;
    }
    const lints = body
      .slice(0, reasonIndex)
      .replace(/,\s*$/u, "")
      .split(",")
      .map((lint) => lint.trim())
      .filter(Boolean);
    if (
      lints.length === 0 ||
      lints.some((lint) => !/^clippy::[a-z0-9_]+$/u.test(lint))
    ) {
      violations.push(
        `${label}: expect attributes may contain only explicit clippy lints`,
      );
    }
  }
  return violations;
}

export function validateTree(root) {
  const violations = [];
  const crates = path.join(root, "crates");
  for (const manifest of filesBelow(crates, "Cargo.toml")) {
    const content = fs.readFileSync(manifest, "utf8");
    if (!/^\[lints\]\s*\nworkspace = true$/mu.test(content)) {
      violations.push(`${manifest}: workspace lint inheritance is required`);
    }
  }
  for (const source of rustFilesBelow(crates)) {
    violations.push(
      ...sourceViolations(fs.readFileSync(source, "utf8"), source),
    );
  }
  return [...new Set(violations)];
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const root = process.argv[2];
  if (!root) {
    process.stderr.write("usage: check-lint-policy.mjs <workspace-root>\n");
    process.exitCode = 2;
  } else {
    const violations = validateTree(root);
    if (violations.length > 0) {
      process.stderr.write(`${violations.join("\n")}\n`);
      process.exitCode = 1;
    }
  }
}
