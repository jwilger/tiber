import assert from "node:assert/strict";
import test from "node:test";

import { sourceViolations } from "./check-lint-policy.mjs";

test("accepts narrowly scoped reasoned Clippy expectations", () => {
  assert.deepEqual(
    sourceViolations(`#[expect(
      clippy::implicit_return,
      reason = "expression form is clearer"
    )]`),
    [],
  );
});

test("accepts a reasoned crate-inner Clippy expectation", () => {
  assert.deepEqual(
    sourceViolations(`#![expect(
      clippy::implicit_return,
      reason = "expression form is clearer"
    )]`),
    [],
  );
});

test("accepts the narrowly permitted module private-doc expectation", () => {
  assert.deepEqual(
    sourceViolations(`#[cfg_attr(
      not(test),
      expect(
        clippy::missing_docs_in_private_items,
        reason = "checked model internals are private implementation detail"
      )
    )]
    pub mod modeled;`),
    [],
  );
});

test("rejects direct and multiline Clippy allows", () => {
  assert.equal(sourceViolations("#[allow(clippy::panic)]").length, 1);
  assert.equal(
    sourceViolations(`#[cfg_attr(test, allow(
      clippy::panic
    ))]`).length,
    1,
  );
});

test("rejects non-Clippy and unreasoned expectations", () => {
  assert.equal(
    sourceViolations('#[expect(dead_code, reason = "temporary")]').length,
    1,
  );
  assert.equal(sourceViolations("#[expect(clippy::panic)]").length, 1);
  assert.equal(
    sourceViolations(
      '#[cfg_attr(test, expect(dead_code, reason = "not clippy"))]',
    ).length,
    1,
  );
  assert.equal(
    sourceViolations("#[cfg_attr(test, expect(clippy::panic))]").length,
    1,
  );
});

test("rejects conditional expectations in crate-inner attributes", () => {
  assert.deepEqual(
    sourceViolations(`#![cfg_attr(
      not(test),
      expect(
        clippy::missing_docs_in_private_items,
        reason = "conditional lint policy bypass"
      )
    )]`),
    [
      "source: conditional expect attributes are forbidden except for a reasoned non-test missing_docs_in_private_items expectation on a public module",
    ],
  );
});

test("rejects a conditional expectation outside the narrow module exception", () => {
  assert.equal(
    sourceViolations(`#[cfg_attr(
      not(test),
      expect(
        clippy::missing_docs_in_private_items,
        reason = "not a module"
      )
    )]
    fn modeled() {}`).length,
    1,
  );
  assert.equal(
    sourceViolations(`#[cfg_attr(
      not(test),
      expect(
        clippy::missing_errors_doc,
        reason = "wrong lint"
      )
    )]
    pub mod modeled;`).length,
    1,
  );
});
