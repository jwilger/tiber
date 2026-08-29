import assert from "node:assert/strict";
import { test } from "node:test";
import { isExpectedOrigin, managedClonePath } from "../scripts/dev-install.mjs";

test("accepts only the canonical GitHub origin spellings", () => {
  for (const remote of [
    "git@github.com:jwilger/tiber.git",
    "git@github.com:jwilger/tiber",
    "https://github.com/jwilger/tiber.git",
    "https://github.com/jwilger/tiber",
    "ssh://git@github.com/jwilger/tiber.git",
  ]) {
    assert.equal(isExpectedOrigin(remote), true, remote);
  }

  for (const remote of [
    "git@github.com:someone/tiber.git",
    "https://example.com/jwilger/tiber.git",
    "/tmp/tiber",
  ]) {
    assert.equal(isExpectedOrigin(remote), false, remote);
  }
});

test("uses Pi's configured agent directory for the managed clone", () => {
  assert.equal(
    managedClonePath({ PI_CODING_AGENT_DIR: "/tmp/pi-agent" }, "/unused"),
    "/tmp/pi-agent/git/github.com/jwilger/tiber",
  );
});

test("defaults the managed clone beneath the user's Pi agent directory", () => {
  assert.equal(
    managedClonePath({}, "/home/tester"),
    "/home/tester/.pi/agent/git/github.com/jwilger/tiber",
  );
});
