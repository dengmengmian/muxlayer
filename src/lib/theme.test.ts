import { describe, expect, it } from "vitest";
import { normalizeTheme } from "./theme";

describe("normalizeTheme", () => {
  it.each([
    ["dark", "dark"],
    ["slate", "dark"],
    ["forest", "dark"],
    ["violet", "dark"],
    ["light", "light"],
    ["latte", "light"],
    ["linen", "light"],
    ["mist", "light"],
    ["sakura", "light"],
    ["unknown", "light"],
    [null, "light"],
  ])("maps %s to %s", (stored, expected) => {
    expect(normalizeTheme(stored)).toBe(expected);
  });
});
