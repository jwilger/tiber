import { describe, expect, it } from "vitest";

import { parseInlineOutputMaximumBytes } from "../../src/core/artifacts/artifact-values.js";
import { virtualizeCommandOutput } from "../../src/core/artifacts/output-virtualization.js";
import {
  commandOutput,
  inlineOutputMaximumBytes as limit,
} from "../fixtures/artifact-values.js";

function output(stdout: string, stderr = "") {
  return commandOutput({ stdout, stderr, exitCode: 0, durationMs: 10 });
}

describe("bounded command output", () => {
  it("returns small UTF-8 output inline", () => {
    expect(virtualizeCommandOutput(output("passed\n"), limit(128))).toEqual({
      kind: "inline",
      output: output("passed\n"),
      byteLength: 7,
    });
  });

  it("records whether oversized output exited or was signaled", () => {
    const exited = virtualizeCommandOutput(
      commandOutput({
        stdout: "x".repeat(200),
        stderr: "",
        exitCode: 7,
        durationMs: 10,
      }),
      limit(64),
    );
    const signaled = virtualizeCommandOutput(
      commandOutput({
        stdout: "x".repeat(200),
        stderr: "",
        exitCode: null,
        durationMs: 10,
      }),
      limit(64),
    );
    expect(exited.kind === "artifact" ? exited.content : "").toContain(
      "exitCode: 7",
    );
    expect(signaled.kind === "artifact" ? signaled.content : "").toContain(
      "exitCode: signal",
    );
  });

  it("virtualizes oversized output with bounded head and tail previews", () => {
    const result = virtualizeCommandOutput(
      output("0123456789".repeat(100)),
      limit(64),
    );
    expect(result.kind).toBe("artifact");
    if (result.kind === "artifact") {
      expect(result.digest).toMatch(/^sha256:[0-9a-f]{64}$/u);
      expect(result.byteLength).toBeGreaterThan(1000);
      expect(result.preview.head.length).toBeGreaterThan(0);
      expect(result.preview.tail.length).toBeGreaterThan(0);
      expect(result.preview.omittedBytes).toBe(result.byteLength - 64);
      expect(Buffer.byteLength(result.content, "utf8")).toBe(result.byteLength);
      expect(result.content).toContain("0123456789");
    }
  });

  it("counts stderr and metadata without breaking UTF-8 preview bounds", () => {
    const result = virtualizeCommandOutput(
      output("🙂".repeat(40), "failure"),
      limit(32),
    );
    expect(result.kind).toBe("artifact");
    if (result.kind === "artifact") {
      expect(
        Buffer.byteLength(result.preview.head, "utf8"),
      ).toBeLessThanOrEqual(16);
      expect(
        Buffer.byteLength(result.preview.tail, "utf8"),
      ).toBeLessThanOrEqual(16);
      expect(result.content).toContain("--- stderr ---\nfailure");
    }
  });

  it("keeps every tiny preview on complete UTF-8 boundaries", () => {
    for (let bound = 1; bound <= 160; bound += 1) {
      const result = virtualizeCommandOutput(
        output("🙂".repeat(100), "é"),
        limit(bound),
      );
      expect(result.kind).toBe("artifact");
      if (result.kind === "artifact") {
        expect(result.preview.head).not.toContain("�");
        expect(result.preview.tail).not.toContain("�");
        expect(result.preview.head).not.toContain("Stryker was here!");
        expect(result.preview.tail).not.toContain("Stryker was here!");
        expect(
          Buffer.byteLength(result.preview.head) +
            Buffer.byteLength(result.preview.tail),
        ).toBeLessThanOrEqual(bound);
      }
    }
  });

  it("retains legitimate replacement characters inside previews", () => {
    const result = virtualizeCommandOutput(
      output(
        `prefix-${"x".repeat(20)}-�-middle-${"y".repeat(160)}`,
        `tail-${"z".repeat(80)}-�-end`,
      ),
      limit(256),
    );
    expect(result.kind).toBe("artifact");
    if (result.kind === "artifact") {
      expect(result.preview.head).toContain("�");
      expect(result.preview.tail).toContain("�");
    }
  });

  it("uses inclusive valid bounds and counts both streams", () => {
    expect(virtualizeCommandOutput(output("x"), limit(1)).kind).toBe("inline");
    expect(virtualizeCommandOutput(output("x", "y"), limit(1)).kind).toBe(
      "artifact",
    );
    expect(virtualizeCommandOutput(output("x"), limit(1_048_576)).kind).toBe(
      "inline",
    );
  });

  it.each([0, -1, 1.5, 1_048_577])(
    "rejects invalid output bound %j",
    (bound) => {
      expect(parseInlineOutputMaximumBytes(bound).ok).toBe(false);
    },
  );
});
