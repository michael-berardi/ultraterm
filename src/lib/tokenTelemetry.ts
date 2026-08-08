import type { TokenCounts } from "../types";

/** Cached input as a share of all input-side tokens (fresh + cache reads). */
export function cacheHitPercent(counts: TokenCounts): number | null {
  const inputSideTokens = counts.input + counts.cacheRead;
  if (!Number.isFinite(inputSideTokens) || inputSideTokens <= 0) return null;
  return Math.min(100, Math.max(0, (counts.cacheRead / inputSideTokens) * 100));
}

export function formatCacheHitPercent(counts: TokenCounts): string {
  const percent = cacheHitPercent(counts);
  if (percent === null) return "—";
  return `${percent.toFixed(1)}%`;
}
