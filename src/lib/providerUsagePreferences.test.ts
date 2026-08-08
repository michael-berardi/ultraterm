import { describe, expect, it, vi } from "vitest";
import {
  parseProviderUsagePreferences,
  persistProviderUsagePreferences,
  PROVIDER_USAGE_PREFERENCES_STORAGE_KEY,
  readProviderUsagePreferences,
} from "./providerUsagePreferences";

describe("provider usage preferences", () => {
  it("defaults every display option on for existing users", () => {
    expect(parseProviderUsagePreferences(null)).toEqual({
      showWeeklyPace: true,
      showResetTimes: true,
      showSecondaryWindows: true,
      showPlanDetails: true,
    });
  });

  it("preserves valid choices while defaulting missing or invalid fields", () => {
    expect(parseProviderUsagePreferences(JSON.stringify({
      showWeeklyPace: false,
      showResetTimes: "no",
      showSecondaryWindows: false,
    }))).toEqual({
      showWeeklyPace: false,
      showResetTimes: true,
      showSecondaryWindows: false,
      showPlanDetails: true,
    });
  });

  it("reads and persists the preference object under one stable key", () => {
    const setItem = vi.fn();
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue(JSON.stringify({ showWeeklyPace: false })),
      setItem,
    });

    const preferences = readProviderUsagePreferences();
    expect(preferences.showWeeklyPace).toBe(false);
    persistProviderUsagePreferences(preferences);
    expect(setItem).toHaveBeenCalledWith(
      PROVIDER_USAGE_PREFERENCES_STORAGE_KEY,
      JSON.stringify(preferences),
    );
  });
});
