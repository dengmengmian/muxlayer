import { describe, expect, it } from "vitest";
import {
  pickModels,
  normalizeModelsForProvider,
  pickModelsForProvider,
} from "./modelHeuristics";

describe("DeepSeek model heuristics", () => {
  it("keeps only v4 models for deepseek", () => {
    expect(
      normalizeModelsForProvider("deepseek", [
        "deepseek-chat",
        "deepseek-flash",
        "deepseek-reasoner",
        "deepseek-v4-pro",
      ])
    ).toEqual(["deepseek-flash", "deepseek-v4-pro"]);
  });

  it("defaults deepseek to flash and reasoning to pro", () => {
    expect(
      pickModelsForProvider("deepseek", [
        "deepseek-chat",
        "deepseek-flash",
        "deepseek-reasoner",
        "deepseek-v4-pro",
      ])
    ).toEqual({ default: "deepseek-flash", reasoning: "deepseek-v4-pro" });
  });
});

describe("pickModels (generic)", () => {
  it("returns empty strings for empty array", () => {
    expect(pickModels([])).toEqual({ default: "", reasoning: "" });
  });

  it("returns the single model as both default and reasoning", () => {
    expect(pickModels(["gpt-4"])).toEqual({
      default: "gpt-4",
      reasoning: "gpt-4",
    });
  });

  it("prefers production models over preview/beta", () => {
    expect(pickModels(["gpt-4-preview", "gpt-4"])).toEqual({
      default: "gpt-4",
      reasoning: "gpt-4",
    });
  });

  it("prefers main models over mini/nano variants", () => {
    expect(pickModels(["claude-sonnet-4", "claude-sonnet-4-mini"])).toEqual({
      default: "claude-sonnet-4",
      reasoning: "claude-sonnet-4",
    });
  });

  it("prefers higher version numbers", () => {
    expect(pickModels(["gpt-4", "gpt-5"])).toEqual({
      default: "gpt-5",
      reasoning: "gpt-5",
    });
  });

  it("separates reasoning models from standard models", () => {
    const result = pickModels(["gpt-4", "o1", "o3-mini"]);
    expect(result.default).toBe("gpt-4");
    expect(result.reasoning).toBe("o1");
  });

  it("picks reasoning from reasoning-keyword models", () => {
    const result = pickModels(["gpt-4", "claude-3-5-sonnet-thinking"]);
    expect(result.default).toBe("gpt-4");
    expect(result.reasoning).toBe("claude-3-5-sonnet-thinking");
  });

  it("falls back to default when no reasoning model exists", () => {
    const result = pickModels(["gpt-4", "gpt-4-turbo"]);
    expect(result.reasoning).toBe(result.default);
  });

  it("when all models are reasoning, default also picks a reasoning model", () => {
    const result = pickModels(["o1", "o3"]);
    expect(result.default).toBe("o3");
    expect(result.reasoning).toBe("o3");
  });

  it("handles date-stamped models without treating dates as version numbers", () => {
    const result = pickModels([
      "claude-haiku-4-5",
      "claude-haiku-4-5-20251001",
    ]);
    expect(result.default).toBe("claude-haiku-4-5");
  });

  it("ranks correctly with mixed tiers", () => {
    const result = pickModels([
      "gpt-4-preview",
      "gpt-4-mini",
      "gpt-4",
      "gpt-3.5-turbo",
    ]);
    expect(result.default).toBe("gpt-4");
  });
});
