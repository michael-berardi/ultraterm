import {
  DEFAULT_PROVIDER_USAGE_PREFERENCES,
  type ProviderUsagePreferences,
} from "../types";

export const PROVIDER_USAGE_PREFERENCES_STORAGE_KEY = "ultraterm.provider-usage-preferences";

function booleanPreference(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

export function parseProviderUsagePreferences(value: string | null): ProviderUsagePreferences {
  if (value === null) return { ...DEFAULT_PROVIDER_USAGE_PREFERENCES };
  try {
    const parsed = JSON.parse(value) as Partial<ProviderUsagePreferences> | null;
    if (typeof parsed !== "object" || parsed === null) {
      return { ...DEFAULT_PROVIDER_USAGE_PREFERENCES };
    }
    return {
      showWeeklyPace: booleanPreference(
        parsed.showWeeklyPace,
        DEFAULT_PROVIDER_USAGE_PREFERENCES.showWeeklyPace,
      ),
      showResetTimes: booleanPreference(
        parsed.showResetTimes,
        DEFAULT_PROVIDER_USAGE_PREFERENCES.showResetTimes,
      ),
      showSecondaryWindows: booleanPreference(
        parsed.showSecondaryWindows,
        DEFAULT_PROVIDER_USAGE_PREFERENCES.showSecondaryWindows,
      ),
    };
  } catch {
    return { ...DEFAULT_PROVIDER_USAGE_PREFERENCES };
  }
}

export function readProviderUsagePreferences(): ProviderUsagePreferences {
  try {
    return parseProviderUsagePreferences(
      localStorage.getItem(PROVIDER_USAGE_PREFERENCES_STORAGE_KEY),
    );
  } catch {
    return { ...DEFAULT_PROVIDER_USAGE_PREFERENCES };
  }
}

export function persistProviderUsagePreferences(preferences: ProviderUsagePreferences): void {
  try {
    localStorage.setItem(PROVIDER_USAGE_PREFERENCES_STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Preferences remain live for this session when persistent storage is unavailable.
  }
}
