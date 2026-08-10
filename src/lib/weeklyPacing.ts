import type { ProviderUsageWindow } from "../types";

const WEEK_MILLISECONDS = 7 * 24 * 60 * 60 * 1000;
const ON_PACE_TOLERANCE_POINTS = 1;

export type WeeklyPaceStatus = "ahead" | "on-pace" | "behind";

export interface WeeklyQuotaPace {
  actualUsedPercent: number;
  targetUsedPercent: number;
  deltaPercent: number;
  status: WeeklyPaceStatus;
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

function timestampMillis(timestamp: number): number {
  return timestamp < 1_000_000_000_000 ? timestamp * 1000 : timestamp;
}

export function isWeeklyUsageWindow(window: ProviderUsageWindow): boolean {
  const normalized = window.label.trim().toLowerCase().replace(/[_\s]+/g, "-");
  return /week|(?:7|seven)-?day/.test(normalized);
}

export function weeklyQuotaPace(
  window: ProviderUsageWindow,
  now = Date.now(),
): WeeklyQuotaPace | null {
  if (!isWeeklyUsageWindow(window) || window.resetsAt === null) return null;
  const resetAt = timestampMillis(window.resetsAt);
  if (!Number.isFinite(resetAt) || !Number.isFinite(now)) return null;

  const targetUsedPercent = clampPercent(
    ((WEEK_MILLISECONDS - (resetAt - now)) / WEEK_MILLISECONDS) * 100,
  );
  const actualUsedPercent = clampPercent(window.usedPercent);
  const deltaPercent = actualUsedPercent - targetUsedPercent;
  const status = Math.abs(deltaPercent) < ON_PACE_TOLERANCE_POINTS
    ? "on-pace"
    : deltaPercent > 0
      ? "ahead"
      : "behind";

  return { actualUsedPercent, targetUsedPercent, deltaPercent, status };
}

export function weeklyPaceLabel(pace: WeeklyQuotaPace): string {
  if (pace.status === "on-pace") return "On pace";
  const percent = Math.max(1, Math.round(Math.abs(pace.deltaPercent)));
  return `${percent} percent ${pace.status}`;
}
