import { describe, expect, it } from "vitest";

import { authorizeBootstrapTool } from "../../src/core/doctor/bootstrap-policy.js";

describe("bootstrap tool policy", () => {
  it("blocks arbitrary shell execution", () => {
    expect(authorizeBootstrapTool("bash")).toEqual({
      kind: "some",
      value: {
        block: true,
        reason:
          "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed",
      },
    });
  });

  it.each(["read", "edit", "write", "tiber_command"])(
    "defers %s to its governed implementation",
    (tool) => {
      expect(authorizeBootstrapTool(tool)).toEqual({ kind: "none" });
    },
  );
});
