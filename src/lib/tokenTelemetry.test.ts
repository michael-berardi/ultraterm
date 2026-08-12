import { describe, expect, it } from "vitest";
import { cacheHitPercent, formatCacheHitPercent } from "./tokenTelemetry";

describe("cacheHitPercent", () => {
  it("uses cached input divided by fresh plus cached input", () => {
    const counts = { input: 250, output: 40, cacheRead: 750, cacheWrite: 10, total: 300 };
    expect(cacheHitPercent(counts)).toBe(75);
    expect(formatCacheHitPercent(counts)).toBe("75.0%");
  });

  it("returns an unavailable marker when no input-side tokens exist", () => {
    const counts = { input: 0, output: 40, cacheRead: 0, cacheWrite: 0, total: 40 };
    expect(cacheHitPercent(counts)).toBeNull();
    expect(formatCacheHitPercent(counts)).toBe("—");
  });

  it("keeps coding-plan, paid-API, and blended rates independent", () => {
    const codingPlan = { input: 100, output: 0, cacheRead: 900, cacheWrite: 0, total: 100 };
    const paidApi = { input: 800, output: 0, cacheRead: 200, cacheWrite: 0, total: 800 };
    const blended = { input: 900, output: 0, cacheRead: 1_100, cacheWrite: 0, total: 900 };

    expect(formatCacheHitPercent(codingPlan)).toBe("90.0%");
    expect(formatCacheHitPercent(paidApi)).toBe("20.0%");
    expect(formatCacheHitPercent(blended)).toBe("55.0%");
  });
});
