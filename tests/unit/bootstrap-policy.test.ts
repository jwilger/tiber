import { describe, expect, it } from "vitest";

import { authorizeBootstrapTool } from "../../src/core/doctor/bootstrap-policy.js";

describe("bootstrap tool policy", () => {
  it.each(["bash", "edit", "write"])("blocks the %s mutation tool", (tool) => {
    expect(authorizeBootstrapTool(tool)).toEqual({
      block: true,
      reason:
        "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed",
    });
  });

  it("leaves a read request available", () => {
    expect(authorizeBootstrapTool("read")).toBeUndefined();
  });
});
