import type { ReactElement } from "react";
import { Fuel, RefreshCw, Settings } from "lucide-react";
import {
  formatResetLabel,
  isUsageStale,
  primaryWindow,
  usageStatusLabel,
  useProviderUsage,
} from "../hooks/useProviderUsage";
import {
  isWeeklyUsageWindow,
  weeklyQuotaPace,
  type WeeklyQuotaPace,
} from "../lib/weeklyPacing";
import type {
  ProviderUsage,
  ProviderUsagePreferences,
  ProviderUsageWindow,
} from "../types";

const LOW_REMAINING_THRESHOLD = 10;

/** Only weekly quota windows receive a primary dial. */
export function displayWindow(usage: ProviderUsage): ProviderUsageWindow | null {
  return usage.windows.find(isWeeklyUsageWindow) ?? null;
}

export function remainingPercent(window: ProviderUsageWindow): number {
  return Math.min(100, Math.max(0, 100 - window.usedPercent));
}

/**
 * The sidebar shows ONE Codex dial. While the primary account's weekly quota
 * reads empty and the fallback account is healthy, the dial silently presents
 * the fallback account's windows instead — and flips back as soon as the
 * primary quota resets. The fallback never gets its own dial.
 */
export function codexDialSource(
  primary: ProviderUsage,
  fallback: ProviderUsage | undefined,
): { usage: ProviderUsage; isBackup: boolean } {
  const primaryWindow = displayWindow(primary);
  const primaryEmpty = primaryWindow !== null && remainingPercent(primaryWindow) <= 0;
  // The fallback's plan may not report a weekly window at all (pro-lite
  // accounts only expose a rolling 5-hour window), so readiness just needs
  // any live window — the dial falls back to the most constraining one.
  const fallbackReady = fallback !== undefined
    && fallback.status === "connected"
    && !isUsageStale(fallback)
    && fallback.windows.length > 0;
  if (primaryEmpty && fallbackReady) {
    // Label must say Fallback outright — showing the backup under the plain
    // "Codex" name would mislead about which account is being spent.
    return { usage: { ...fallback, displayName: `${primary.displayName} Fallback` }, isBackup: true };
  }
  return { usage: primary, isBackup: false };
}

export function formatUsageWindowLabel(label: string): string {
  const normalized = label.trim().toLowerCase().replace(/[_\s]+/g, "-");
  const duration = normalized.match(/^(\d+)-(minute|hour|day)s?$/);
  if (!duration) return label;
  const value = Number(duration[1]);
  const unit = duration[2];
  if (unit === "minute" && value % 60 === 0) {
    const hours = value / 60;
    return `${hours} ${hours === 1 ? "hour" : "hours"}`;
  }
  return `${value} ${value === 1 ? unit : `${unit}s`}`;
}

export function paceDialGeometry(pace: WeeklyQuotaPace): {
  actualRemaining: number;
  targetRemaining: number;
  gapStart: number;
  gapLength: number;
  targetMark: {
    innerX: number;
    innerY: number;
    outerX: number;
    outerY: number;
    leftX: number;
    leftY: number;
    rightX: number;
    rightY: number;
  };
} {
  const actualRemaining = 100 - pace.actualUsedPercent;
  const targetRemaining = 100 - pace.targetUsedPercent;
  const targetAngle = ((targetRemaining / 100) * Math.PI * 2) - (Math.PI / 2);
  const directionX = Math.cos(targetAngle);
  const directionY = Math.sin(targetAngle);
  const tangentX = -directionY;
  const tangentY = directionX;
  const targetMark = {
    innerX: 50 + (directionX * 32),
    innerY: 50 + (directionY * 32),
    outerX: 50 + (directionX * 44),
    outerY: 50 + (directionY * 44),
    leftX: 50 + (directionX * 38) + (tangentX * 4.5),
    leftY: 50 + (directionY * 38) + (tangentY * 4.5),
    rightX: 50 + (directionX * 38) - (tangentX * 4.5),
    rightY: 50 + (directionY * 38) - (tangentY * 4.5),
  };
  return {
    actualRemaining,
    targetRemaining,
    gapStart: Math.min(actualRemaining, targetRemaining),
    gapLength: Math.abs(actualRemaining - targetRemaining),
    targetMark,
  };
}

function dialAriaSummary(
  usage: ProviderUsage,
  window: ProviderUsageWindow,
  preferences: ProviderUsagePreferences,
  pace: WeeklyQuotaPace | null,
): string {
  const summary = [
    `${usage.displayName} usage`,
    usageStatusLabel(usage, false),
    `${Math.round(remainingPercent(window))} percent of ${window.label} remaining`,
  ];
  if (preferences.showResetTimes) summary.push(formatResetLabel(window.resetsAt));
  if (preferences.showWeeklyPace && pace) {
    const targetRemaining = Math.round(100 - pace.targetUsedPercent);
    const actualRemaining = Math.round(100 - pace.actualUsedPercent);
    const paceContext = pace.status === "ahead"
      ? "spending above the weekly target pace"
      : pace.status === "behind"
        ? "spending below the weekly target pace"
        : "spending on the weekly target pace";
    summary.push(
      `${actualRemaining} percent remaining against a ${targetRemaining} percent weekly target`,
      paceContext,
    );
  }
  return summary.join(", ");
}

function UsageDial({
  usage,
  preferences,
  isBackup = false,
  dialWindow,
}: {
  usage: ProviderUsage;
  preferences: ProviderUsagePreferences;
  isBackup?: boolean;
  /** Overrides the weekly default — used when the backup account's plan only
   *  reports shorter windows. */
  dialWindow?: ProviderUsageWindow;
}): ReactElement | null {
  const window = dialWindow ?? displayWindow(usage);
  if (!window) return null;
  const remaining = remainingPercent(window);
  const danger = remaining <= LOW_REMAINING_THRESHOLD;
  const secondaryWindows = preferences.showSecondaryWindows
    ? usage.windows.filter((item) => item !== window).slice(0, 2)
    : [];
  const pace = preferences.showWeeklyPace ? weeklyQuotaPace(window) : null;
  const paceGeometry = pace ? paceDialGeometry(pace) : null;

  return (
    <div
      className="usage-dial usage-dial--connected"
      role="group"
      aria-label={(isBackup ? "Backup account active. " : "") + dialAriaSummary(usage, window, preferences, pace)}
    >
      <div className="usage-dial__header">
        <span className="usage-dial__name">{usage.displayName}</span>
        <span
          className="usage-dial__status usage-dial__status--connected"
          title={isBackup
            ? "Primary account is out of quota — showing the fallback account until it resets"
            : usageStatusLabel(usage, false)}
        />
      </div>

      <div className="usage-dial__gauge-wrap">
        <svg className="usage-dial__gauge" viewBox="0 0 100 100" aria-hidden="true" focusable="false">
          <circle className="usage-dial__track" cx="50" cy="50" r="38" pathLength={100} fill="none" />
          {remaining > 0 && (
            <circle
              className={`usage-dial__value${danger ? " is-danger" : ""}`}
              cx="50"
              cy="50"
              r="38"
              pathLength={100}
              fill="none"
              strokeDasharray={`${remaining} 100`}
            />
          )}
          {pace && paceGeometry && pace.status !== "on-pace" && paceGeometry.gapLength > 0 && (
            <circle
              className={`usage-dial__variance is-${pace.status}`}
              cx="50"
              cy="50"
              r="38"
              pathLength={100}
              fill="none"
              strokeDasharray={`${paceGeometry.gapLength} ${100 - paceGeometry.gapLength}`}
              strokeDashoffset={-paceGeometry.gapStart}
            />
          )}
          {paceGeometry && (
            <polygon
              className="usage-dial__target-mark"
              points={`${paceGeometry.targetMark.innerX},${paceGeometry.targetMark.innerY} ${paceGeometry.targetMark.leftX},${paceGeometry.targetMark.leftY} ${paceGeometry.targetMark.outerX},${paceGeometry.targetMark.outerY} ${paceGeometry.targetMark.rightX},${paceGeometry.targetMark.rightY}`}
            />
          )}
        </svg>
        <div className="usage-dial__reading">
          <strong className="usage-dial__percent">{Math.round(remaining)}%</strong>
        </div>
      </div>

      {preferences.showResetTimes && (
        <span className="usage-dial__footer">{formatResetLabel(window.resetsAt)}</span>
      )}

      {secondaryWindows.length > 0 && (
        <div className="usage-dial__windows">
          {secondaryWindows.map((item) => {
            const itemRemaining = remainingPercent(item);
            const displayLabel = formatUsageWindowLabel(item.label);
            return (
              <div key={item.label} className="usage-dial__window-row">
                <span className="usage-dial__window-label" title={item.label}>{displayLabel}</span>
                <span
                  className="usage-dial__window-value"
                  aria-label={`${Math.round(itemRemaining)} percent of ${displayLabel} remaining`}
                >
                  {Math.round(itemRemaining)}%
                </span>
                <span className="usage-dial__mini-bar" aria-hidden="true">
                  <span
                    className={itemRemaining <= LOW_REMAINING_THRESHOLD ? "is-danger" : ""}
                    style={{ width: `${itemRemaining}%` }}
                  />
                </span>
                {preferences.showResetTimes && (
                  <span className="usage-dial__window-reset">{formatResetLabel(item.resetsAt)}</span>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function UsageDials({
  preferences,
  onOpenSettings,
}: {
  preferences: ProviderUsagePreferences;
  onOpenSettings?: () => void;
}): ReactElement {
  const { usages, loading, refreshing, error, refresh } = useProviderUsage();
  const fallback = usages.find((usage) => usage.provider === "codex-fallback");
  const connected = usages.filter(
    (usage) => usage.provider !== "codex-fallback"
      && usage.status === "connected"
      && !isUsageStale(usage)
      && displayWindow(usage) !== null,
  );

  let usageContent: ReactElement;
  if (loading && connected.length === 0) {
    usageContent = <p className="usage-empty">Checking provider quota…</p>;
  } else if (connected.length === 0) {
    usageContent = (
      <button type="button" className="usage-configure" onClick={onOpenSettings}>
        <Settings size={12} />
        <span>
          <strong>No live provider stats</strong>
          <small>Configure provider stats</small>
        </span>
      </button>
    );
  } else {
    usageContent = (
      <div className="usage-dials">
        {connected.map((usage) => {
          if (usage.provider === "codex") {
            const source = codexDialSource(usage, fallback);
            return (
              <UsageDial
                key={usage.provider}
                usage={source.usage}
                preferences={preferences}
                isBackup={source.isBackup}
                dialWindow={source.isBackup
                  ? displayWindow(source.usage) ?? primaryWindow(source.usage) ?? undefined
                  : undefined}
              />
            );
          }
          return <UsageDial key={usage.provider} usage={usage} preferences={preferences} />;
        })}
      </div>
    );
  }

  return (
    <section className="sidebar-section sidebar-section--usage" aria-labelledby="usage-heading">
      <div className="sidebar-section__heading">
        <span id="usage-heading"><Fuel size={13} /> Provider usage</span>
        <button
          type="button"
          className="glass-icon-button usage-refresh"
          onClick={() => void refresh()}
          disabled={refreshing}
          aria-label="Refresh provider usage"
          title="Refresh provider usage"
        >
          <RefreshCw size={11} className={refreshing ? "is-spinning" : ""} />
        </button>
      </div>

      {error && (
        <p className="usage-section-error" role="alert">
          {error}
        </p>
      )}

      {usageContent}
    </section>
  );
}
