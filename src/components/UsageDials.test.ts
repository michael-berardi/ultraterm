import { describe, expect, it } from "vitest";
import {
  displayWindow,
  formatUsageWindowLabel,
  paceDialGeometry,
  remainingPercent,
} from "./UsageDials";

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

describe("displayWindow", () => {
  it("leads with provider variants of the weekly quota window", () => {
    const shortWindow = { label: "5-hour", usedPercent: 80, resetsAt: null };
    const weeklyWindow = { label: "seven_day", usedPercent: 20, resetsAt: null };
    const usage = {
      provider: "codex" as const,
      displayName: "Codex",
      plan: null,
      status: "connected" as const,
      windows: [shortWindow, weeklyWindow],
      balance: null,
      updatedAt: null,
      error: null,
    };

    expect(displayWindow(usage)).toBe(weeklyWindow);
  });

  it("omits providers that do not report a weekly quota", () => {
    const usage = {
      provider: "codex" as const,
      displayName: "Codex",
      plan: "Pro",
      status: "connected" as const,
      windows: [{ label: "5-hour", usedPercent: 20, resetsAt: null }],
      balance: null,
      updatedAt: null,
      error: null,
    };

    expect(displayWindow(usage)).toBeNull();
  });
});

describe("paceDialGeometry", () => {
  it("places the variance arc strictly between actual and target remaining quota", () => {
    const overspend = paceDialGeometry({
      actualUsedPercent: 65,
      targetUsedPercent: 43,
      status: "ahead",
    });
    const underspend = paceDialGeometry({
      actualUsedPercent: 24,
      targetUsedPercent: 43,
      status: "behind",
    });

    expect(overspend).toMatchObject({
      actualRemaining: 35,
      targetRemaining: 57,
      gapStart: 35,
      gapLength: 22,
    });
    expect(underspend).toMatchObject({
      actualRemaining: 76,
      targetRemaining: 57,
      gapStart: 57,
      gapLength: 19,
    });
    expect(Object.values(overspend.targetMark).every(Number.isFinite)).toBe(true);
  });
});
