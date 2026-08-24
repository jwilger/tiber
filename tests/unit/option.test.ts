import { describe, expect, it } from "vitest";

import {
  isSome,
  mapOption,
  none,
  some,
  type Option,
} from "../../src/core/types/option.js";

describe("Option", () => {
  it("represents presence without null or undefined", () => {
    const option: Option<string> = some("value");
    expect(isSome(option)).toBe(true);
    expect(option).toEqual({ kind: "some", value: "value" });
  });

  it("represents absence explicitly", () => {
    const option: Option<string> = none;
    expect(isSome(option)).toBe(false);
    expect(option).toEqual({ kind: "none" });
  });

  it("maps present values and preserves absence", () => {
    expect(mapOption(some(2), (value) => value * 3)).toEqual(some(6));
    expect(mapOption(none, (value: number) => value * 3)).toEqual(none);
  });
});
