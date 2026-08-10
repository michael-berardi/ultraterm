import { describe, expect, it } from "vitest";
import { isWeeklyUsageWindow, weeklyQuotaPace } from "./weeklyPacing";

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 7, 8, 12);

describe("weekly quota pacing", () => {
  it("recognizes provider variants of a seven-day window", () => {
    expect(isWeeklyUsageWindow({ label: "Weekly", usedPercent: 0, resetsAt: null })).toBe(true);
    expect(isWeeklyUsageWindow({ label: "seven_day", usedPercent: 0, resetsAt: null })).toBe(true);
    expect(isWeeklyUsageWindow({ label: "7-day", usedPercent: 0, resetsAt: null })).toBe(true);
    expect(isWeeklyUsageWindow({ label: "5-hour", usedPercent: 0, resetsAt: null })).toBe(false);
  });

  it("targets one seventh of weekly spend after each elapsed day", () => {
    const oneDayElapsed = weeklyQuotaPace({
      label: "Weekly",
      usedPercent: 20,
      resetsAt: NOW + (6 * DAY),
    }, NOW);
    const fourDaysElapsed = weeklyQuotaPace({
      label: "Weekly",
      usedPercent: 50,
      resetsAt: NOW + (3 * DAY),
    }, NOW);

    expect(oneDayElapsed?.targetUsedPercent).toBeCloseTo(100 / 7, 5);
    expect(fourDaysElapsed?.targetUsedPercent).toBeCloseTo(400 / 7, 5);
  });

  it("classifies actual spend as ahead, on pace, or behind the target", () => {
    const resetAt = NOW + (5 * DAY);
    const ahead = weeklyQuotaPace({ label: "Weekly", usedPercent: 40, resetsAt: resetAt }, NOW);
    const onPace = weeklyQuotaPace({ label: "Weekly", usedPercent: 28.7, resetsAt: resetAt }, NOW);
    const behind = weeklyQuotaPace({ label: "Weekly", usedPercent: 10, resetsAt: resetAt }, NOW);

    expect(ahead?.status).toBe("ahead");
    expect(onPace?.status).toBe("on-pace");
    expect(behind?.status).toBe("behind");
  });

  it("returns no pace without a weekly reset anchor", () => {
    expect(weeklyQuotaPace({ label: "Weekly", usedPercent: 40, resetsAt: null }, NOW)).toBeNull();
    expect(weeklyQuotaPace({ label: "5-hour", usedPercent: 40, resetsAt: NOW + DAY }, NOW)).toBeNull();
  });
});
