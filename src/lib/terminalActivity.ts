export const RESIZE_ACTIVITY_SUPPRESSION_MS = 400;

export function resizeActivitySuppressionDeadline(nowMs: number): number {
  return nowMs + RESIZE_ACTIVITY_SUPPRESSION_MS;
}

export function isResizeActivitySuppressed(
  suppressedUntilMs: number | undefined,
  nowMs: number,
): boolean {
  return suppressedUntilMs !== undefined && nowMs < suppressedUntilMs;
}
