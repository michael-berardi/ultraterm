import type { ReactElement } from "react";
import { Fuel, RefreshCw, Settings } from "lucide-react";
import {
  formatResetLabel,
  isUsageStale,
  primaryWindow,
  usageStatusLabel,
  useProviderUsage,
} from "../hooks/useProviderUsage";
import type { ProviderUsage, ProviderUsageWindow } from "../types";

const LOW_REMAINING_THRESHOLD = 10;

/** Quota dials lead with the weekly window when the provider reports one. */
export function displayWindow(usage: ProviderUsage): ProviderUsageWindow | null {
  if (usage.windows.length === 0) return null;
  const weekly = usage.windows.find((window) => /week|7[-\s]?day/i.test(window.label));
  return weekly ?? primaryWindow(usage);
}

export function remainingPercent(window: ProviderUsageWindow): number {
  return Math.min(100, Math.max(0, 100 - window.usedPercent));
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

export function usagePlanLine(usage: ProviderUsage): string {
  const balance = usage.balance?.trim();
  const meaningfulBalance = balance && !/^0(?:\.0+)?$/.test(balance) ? balance : null;
  return [usage.plan, meaningfulBalance].filter(Boolean).join(" · ");
}

function dialAriaSummary(usage: ProviderUsage, window: ProviderUsageWindow): string {
  return [
    `${usage.displayName} usage`,
    usageStatusLabel(usage, false),
    `${Math.round(remainingPercent(window))} percent of ${window.label} remaining`,
    formatResetLabel(window.resetsAt),
  ].join(", ");
}

function UsageDial({ usage }: { usage: ProviderUsage }): ReactElement | null {
  const window = displayWindow(usage);
  if (!window) return null;
  const remaining = remainingPercent(window);
  const danger = remaining <= LOW_REMAINING_THRESHOLD;
  const secondaryWindows = usage.windows.filter((item) => item !== window).slice(0, 2);
  const planLine = usagePlanLine(usage);

  return (
    <div className="usage-dial usage-dial--connected" role="group" aria-label={dialAriaSummary(usage, window)}>
      <div className="usage-dial__header">
        <span className="usage-dial__name">{usage.displayName}</span>
        <span
          className="usage-dial__status usage-dial__status--connected"
          title={usageStatusLabel(usage, false)}
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
        </svg>
        <div className="usage-dial__reading">
          <strong className="usage-dial__percent">{Math.round(remaining)}%</strong>
        </div>
      </div>

      <span className="usage-dial__footer">{formatResetLabel(window.resetsAt)}</span>

      {planLine && <span className="usage-dial__plan">{planLine}</span>}

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
                <span className="usage-dial__window-reset">{formatResetLabel(item.resetsAt)}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function UsageDials({ onOpenSettings }: { onOpenSettings?: () => void }): ReactElement {
  const { usages, loading, refreshing, error, refresh } = useProviderUsage();
  const connected = usages.filter(
    (usage) => usage.status === "connected" && !isUsageStale(usage) && displayWindow(usage) !== null,
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
        {connected.map((usage) => (
          <UsageDial key={usage.provider} usage={usage} />
        ))}
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
