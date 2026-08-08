import { describe, expect, it } from "vitest";
import { formatUsageWindowLabel, remainingPercent, usagePlanLine } from "./UsageDials";

describe("formatUsageWindowLabel", () => {
  it("shows minute-based five-hour windows in human units", () => {
    expect(formatUsageWindowLabel("300-minute")).toBe("5 hours");
    expect(formatUsageWindowLabel("5-hour")).toBe("5 hours");
  });

  it("preserves named quota windows", () => {
    expect(formatUsageWindowLabel("Weekly")).toBe("Weekly");
  });
});

describe("remainingPercent", () => {
  it("clamps remaining quota to the zero-to-100 range", () => {
    expect(remainingPercent({ label: "Weekly", usedPercent: 21, resetsAt: null })).toBe(79);
    expect(remainingPercent({ label: "Weekly", usedPercent: 130, resetsAt: null })).toBe(0);
  });
});

describe("usagePlanLine", () => {
  it("hides a zero balance while retaining meaningful plan context", () => {
    const usage = {
      provider: "codex" as const,
      displayName: "Codex",
      plan: "pro",
      status: "connected" as const,
      windows: [],
      balance: "0",
      updatedAt: null,
      error: null,
    };
    expect(usagePlanLine(usage)).toBe("pro");
    expect(usagePlanLine({ ...usage, balance: "12.5" })).toBe("pro · 12.5");
  });
});
