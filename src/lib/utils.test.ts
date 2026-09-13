import { describe, it, expect } from "vitest";
import {
  cn,
  formatTimestamp,
  formatLatency,
  formatOptionalLatency,
  formatCost,
} from "./utils";

describe("cn", () => {
  it("merges class names", () => {
    expect(cn("a", "b")).toBe("a b");
  });

  it("handles conditional classes", () => {
    expect(cn("a", false && "b", "c")).toBe("a c");
  });

  it("handles undefined and null", () => {
    expect(cn("a", undefined, null, "b")).toBe("a b");
  });
});

describe("formatTimestamp", () => {
  it("formats ISO string to MM-DD HH:MM:SS", () => {
    const result = formatTimestamp("2024-01-15T08:30:45.000Z");
    expect(result).toMatch(/01-15\s+\d{2}:\d{2}:\d{2}/);
  });

  it("supports zh locale", () => {
    const result = formatTimestamp("2024-01-15T08:30:45.000Z", "zh");
    expect(result).toMatch(/01-15\s+\d{2}:\d{2}:\d{2}/);
  });
});

describe("formatLatency", () => {
  it("shows ms for values under 1000", () => {
    expect(formatLatency(500)).toBe("500ms");
    expect(formatLatency(0)).toBe("0ms");
    expect(formatLatency(999)).toBe("999ms");
  });

  it("shows seconds for values >= 1000", () => {
    expect(formatLatency(1000)).toBe("1.0s");
    expect(formatLatency(1500)).toBe("1.5s");
    expect(formatLatency(20000)).toBe("20.0s");
  });
});

describe("formatOptionalLatency", () => {
  it("shows dash for missing or unrecorded latency", () => {
    expect(formatOptionalLatency(null)).toBe("—");
    expect(formatOptionalLatency(0)).toBe("—");
  });

  it("formats positive latency", () => {
    expect(formatOptionalLatency(500)).toBe("500ms");
    expect(formatOptionalLatency(1500)).toBe("1.5s");
  });
});

describe("formatCost", () => {
  it("shows unknown cost as a dash", () => {
    expect(formatCost(null)).toBe("—");
    expect(formatCost(undefined)).toBe("—");
  });

  it("shows zero and negative cost as $0.00", () => {
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(-0.5)).toBe("$0.00");
  });

  it("keeps 4 decimals below one cent", () => {
    expect(formatCost(0.00123)).toBe("$0.0012");
  });

  it("uses 3 decimals below one dollar", () => {
    expect(formatCost(0.1234)).toBe("$0.123");
  });

  it("uses 2 decimals from one dollar up", () => {
    expect(formatCost(1)).toBe("$1.00");
    expect(formatCost(12.345)).toBe("$12.35");
  });
});
