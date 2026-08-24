import { describe, expect, it } from "vitest";

import {
  compileCommandCatalog,
  decideCommandExecution,
} from "../../src/core/commands/structured-command.js";

const command = {
  name: "unit-tests",
  executable: "/usr/bin/node",
  argv: ["--test", "tests/unit"],
  cwd: "worktree",
  environment: { CI: "true" },
  timeoutMs: 60_000,
  maxOutputBytes: 65_536,
};
const catalog = { schemaVersion: 1, commands: [command] };

function withCommand(changes: Readonly<Record<string, unknown>>): unknown {
  return { ...catalog, commands: [{ ...command, ...changes }] };
}

describe("structured command authority", () => {
  it("compiles byte-stable named executable data and a digest", () => {
    const result = compileCommandCatalog(catalog);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.canonicalJson).toBe(JSON.stringify(catalog));
    expect(result.value.digest).toMatch(/^sha256:[0-9a-f]{64}$/u);
  });

  it("accepts exact command, argument, environment, and catalog maxima", () => {
    const environment = Object.fromEntries(
      Array.from({ length: 32 }, (_, index) => [
        `KEY_${String(index).padStart(2, "0")}`,
        "x".repeat(4096),
      ]),
    );
    const commands = Array.from({ length: 64 }, (_, index) => ({
      ...command,
      name: `command-${String(index)}`,
      argv: Array.from({ length: 64 }, () => "x".repeat(4096)),
      environment,
      timeoutMs: index === 0 ? 1 : 3_600_000,
      maxOutputBytes: index === 0 ? 1 : 1_048_576,
    }));
    expect(compileCommandCatalog({ schemaVersion: 1, commands })).toMatchObject(
      {
        ok: true,
      },
    );
  });

  it("sorts environment keys into canonical digest input", () => {
    const result = compileCommandCatalog(
      withCommand({ environment: { ZED: "z", ALPHA: "a" } }),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.commands[0]?.environment).toEqual({
      ALPHA: "a",
      ZED: "z",
    });
    expect(result.value.canonicalJson.indexOf("ALPHA")).toBeLessThan(
      result.value.canonicalJson.indexOf("ZED"),
    );
  });

  it.each([
    null,
    {},
    { ...catalog, extra: true },
    { ...catalog, schemaVersion: 2 },
    { ...catalog, commands: [] },
    {
      ...catalog,
      commands: Array.from({ length: 65 }, (_, index) => ({
        ...command,
        name: `command-${String(index)}`,
      })),
    },
    { ...catalog, commands: [command, command] },
    {
      ...catalog,
      commands: [command, { ...command, name: "other", executable: "node" }],
    },
    withCommand({ name: "Bad Name" }),
    withCommand({ name: "valid!suffix" }),
    withCommand({ name: 1 }),
    withCommand({ executable: "node" }),
    withCommand({ executable: 1 }),
    withCommand({ argv: ["ok", 1] }),
    withCommand({ argv: ["bad\0arg"] }),
    withCommand({ argv: Array.from({ length: 65 }, () => "x") }),
    withCommand({ argv: ["x".repeat(4097)] }),
    withCommand({ cwd: "/tmp" }),
    withCommand({ environment: { PATH: 1 } }),
    withCommand({ environment: { "bad-key": "x" } }),
    withCommand({ environment: { xGOOD: "x" } }),
    withCommand({ environment: { "GOOD!": "x" } }),
    withCommand({ environment: { GOOD: "x".repeat(4097) } }),
    withCommand({
      environment: Object.fromEntries(
        Array.from({ length: 33 }, (_, index) => [`KEY_${String(index)}`, "x"]),
      ),
    }),
    withCommand({ timeoutMs: 0 }),
    withCommand({ timeoutMs: 3_600_001 }),
    withCommand({ timeoutMs: "1" }),
    withCommand({ maxOutputBytes: 0 }),
    withCommand({ maxOutputBytes: 1_048_577 }),
    withCommand({ maxOutputBytes: "1" }),
    withCommand({ callback: "code" }),
  ])("rejects executable or unbounded catalog data %j", (input) => {
    const result = compileCommandCatalog(input);
    expect(result).toMatchObject({
      ok: false,
      failure: { code: "TIBER_COMMAND_CATALOG_INVALID" },
    });
    if (!result.ok) expect(result.failure.message.length).toBeGreaterThan(0);
  });

  it("requires an exact local catalog digest grant and active claim", () => {
    const compiled = compileCommandCatalog(catalog);
    if (!compiled.ok) throw new Error("fixture must compile");
    expect(
      decideCommandExecution(compiled.value, "unit-tests", {
        activeClaim: true,
        grantedCatalogDigest: compiled.value.digest,
      }),
    ).toEqual({ ok: true, command: compiled.value.commands[0] });
    for (const authority of [
      { activeClaim: false, grantedCatalogDigest: compiled.value.digest },
      { activeClaim: true, grantedCatalogDigest: undefined },
      { activeClaim: true, grantedCatalogDigest: `sha256:${"f".repeat(64)}` },
    ]) {
      expect(
        decideCommandExecution(compiled.value, "unit-tests", authority),
      ).toEqual({ ok: false, code: "TIBER_COMMAND_DENIED" });
    }
    expect(
      decideCommandExecution(compiled.value, "unknown", {
        activeClaim: true,
        grantedCatalogDigest: compiled.value.digest,
      }),
    ).toEqual({ ok: false, code: "TIBER_COMMAND_UNKNOWN" });
  });
});
