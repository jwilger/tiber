import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { FileCompactionArtifactStore } from "../../src/adapters/context/file-compaction-artifact-store.js";
import {
  boundCompactionText,
  previousEpochId,
} from "../../src/extension/headroom-compaction.js";

describe("Tiber-aware compaction provenance", () => {
  it("retains complete source privately while model input remains hard-bounded", async () => {
    const root = await mkdtemp(path.join(tmpdir(), "tiber-compaction-"));
    try {
      const source = `old-${"α".repeat(10_000)}-critical-tail`;
      const artifact = await new FileCompactionArtifactStore(
        root,
        "/repository",
      ).preserve(source);
      expect(artifact.ok).toBe(true);
      if (!artifact.ok) return;
      expect(artifact.value.digest).toBe(
        createHash("sha256").update(source).digest("hex"),
      );
      expect(await readFile(artifact.value.path, "utf8")).toBe(source);
      const bounded = boundCompactionText(source, 1024);
      expect(Buffer.byteLength(bounded, "utf8")).toBeLessThanOrEqual(1024);
      expect(bounded).toContain("Earlier context omitted");
      expect(bounded).toContain("critical-tail");
      expect(boundCompactionText("exact", 5)).toBe("exact");
      expect(
        Buffer.byteLength(boundCompactionText(source, 10), "utf8"),
      ).toBeLessThanOrEqual(10);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("continues only from a valid latest explicit epoch", () => {
    const old = "a".repeat(64);
    const latest = "b".repeat(64);
    expect(
      previousEpochId(
        [
          { details: { epoch: { epochId: old } } },
          { details: { epoch: { epochId: "forged" } } },
          { details: { epoch: { epochId: latest } } },
        ],
        "c".repeat(64),
      ),
    ).toBe(latest);
    expect(
      previousEpochId([{ details: { epoch: { epochId: "invalid" } } }], old),
    ).toBe(old);
  });
});
