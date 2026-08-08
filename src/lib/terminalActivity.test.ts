import { describe, expect, it } from "vitest";
import {
  RESIZE_ACTIVITY_SUPPRESSION_MS,
  isResizeActivitySuppressed,
  resizeActivitySuppressionDeadline,
} from "./terminalActivity";

describe("terminal resize activity suppression", () => {
  it("suppresses redraw output only during the resize window", () => {
    const now = 1_000;
    const deadline = resizeActivitySuppressionDeadline(now);

    expect(deadline).toBe(now + RESIZE_ACTIVITY_SUPPRESSION_MS);
    expect(isResizeActivitySuppressed(deadline, now)).toBe(true);
    expect(isResizeActivitySuppressed(deadline, deadline - 1)).toBe(true);
    expect(isResizeActivitySuppressed(deadline, deadline)).toBe(false);
  });

  it("does not suppress ordinary terminal output", () => {
    expect(isResizeActivitySuppressed(undefined, 1_000)).toBe(false);
  });
});
