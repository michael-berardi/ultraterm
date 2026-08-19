import { describe, expect, it } from "vitest";
import {
  codexDialSource,
  displayWindow,
  formatUsageWindowLabel,
  paceDialGeometry,
  remainingPercent,
} from "./UsageDials";
import type { ProviderUsage } from "../types";

function codexUsage(usedPercent: number, provider: "codex" | "codex-fallback" = "codex"): ProviderUsage {
  return {
    provider,
    displayName: provider === "codex" ? "Codex" : "Codex Fallback",
    plan: null,
    status: "connected",
    windows: [
      { label: "5-hour", usedPercent: 10, resetsAt: null },
      { label: "Weekly", usedPercent, resetsAt: null },
    ],
    balance: null,
    updatedAt: Date.now(),
    error: null,
  };
}

describe("codexDialSource", () => {
  it("shows the primary account while its weekly quota remains", () => {
    const source = codexDialSource(codexUsage(40), codexUsage(5, "codex-fallback"));
    expect(source.isBackup).toBe(false);
    expect(source.usage.provider).toBe("codex");
  });

  it("switches to the fallback account when the primary weekly quota is empty", () => {
    const source = codexDialSource(codexUsage(100), codexUsage(5, "codex-fallback"));
    expect(source.isBackup).toBe(true);
    expect(source.usage.provider).toBe("codex-fallback");
    // The dial must be unmistakably labeled as the fallback account.
    expect(source.usage.displayName).toBe("Codex Fallback");
  });

  it("stays on an empty primary when no fallback is connected", () => {
    expect(codexDialSource(codexUsage(100), undefined).isBackup).toBe(false);
    const disconnected = { ...codexUsage(0, "codex-fallback"), status: "disconnected" as const };
    expect(codexDialSource(codexUsage(100), disconnected).isBackup).toBe(false);
  });

  it("stays on an empty primary when the fallback data is stale", () => {
    const stale = { ...codexUsage(5, "codex-fallback"), status: "stale" as const };
    expect(codexDialSource(codexUsage(100), stale).isBackup).toBe(false);
  });

  it("switches to a fallback whose plan only reports a five-hour window", () => {
    const fiveHourOnly: ProviderUsage = {
      ...codexUsage(44, "codex-fallback"),
      windows: [{ label: "5-hour", usedPercent: 44, resetsAt: null }],
    };
    const source = codexDialSource(codexUsage(100), fiveHourOnly);
    expect(source.isBackup).toBe(true);
  });
});

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
