import type { ReactElement } from "react";
import { Fuel, RefreshCw } from "lucide-react";
import {
  formatResetLabel,
  formatUpdatedAgo,
  isUsageStale,
  primaryWindow,
  usageStatusLabel,
  useProviderUsage,
} from "../hooks/useProviderUsage";
import type { ProviderUsage, ProviderUsageStatus } from "../types";

const DANGER_THRESHOLD = 90;
const ARC_PATH = "M 12 54 A 38 38 0 0 1 88 54";

const TICKS = [0, 25, 50, 75, 100].map((fraction) => {
  const angle = ((180 - fraction * 1.8) * Math.PI) / 180;
  const major = fraction % 50 === 0;
  const inner = major ? 31 : 32.5;
  return {
    fraction,
    major,
    x1: 50 + inner * Math.cos(angle),
    y1: 54 - inner * Math.sin(angle),
    x2: 50 + 35 * Math.cos(angle),
    y2: 54 - 35 * Math.sin(angle),
  };
});

type DialStatus = ProviderUsageStatus | "stale";

function dialFooterCopy(usage: ProviderUsage, status: DialStatus): string {
  switch (status) {
    case "loading":
      return "Checking live quota";
    case "disconnected":
      return "No credential stored";
    case "error":
      return usage.error ?? "Usage unavailable";
    case "stale":
      return `Stale · ${formatUpdatedAgo(usage.updatedAt)}`;
    default: {
      const window = primaryWindow(usage);
      return window ? formatResetLabel(window.resetsAt) : formatUpdatedAgo(usage.updatedAt);
    }
  }
}

function dialAriaSummary(usage: ProviderUsage, status: DialStatus): string {
  const window = primaryWindow(usage);
  const parts = [`${usage.displayName} usage`, usageStatusLabel(usage, status === "stale")];
  if (window && (status === "connected" || status === "stale")) {
    parts.push(`${Math.round(window.usedPercent)} percent of ${window.label} used`);
    parts.push(formatResetLabel(window.resetsAt));
  }
  if (status === "error" && usage.error) parts.push(usage.error);
  return parts.join(", ");
}

function UsageDial({
  usage,
  sectionLoading,
  onOpenSettings,
}: {
  usage: ProviderUsage;
  sectionLoading: boolean;
  onOpenSettings?: () => void;
}): ReactElement {
  const status: DialStatus = sectionLoading && usage.status === "disconnected"
    ? "loading"
    : isUsageStale(usage)
      ? "stale"
      : usage.status;
  const window = primaryWindow(usage);
  const hasReading = (status === "connected" || status === "stale" || status === "error") && window !== null;
  const percent = hasReading ? window.usedPercent : null;
  const danger = percent !== null && percent >= DANGER_THRESHOLD;
  const secondaryWindows = usage.windows.filter((item) => item !== window).slice(0, 2);
  const planLine = [usage.plan, usage.balance].filter(Boolean).join(" · ");

  return (
    <div className={`usage-dial usage-dial--${status}`} role="group" aria-label={dialAriaSummary(usage, status)}>
      <div className="usage-dial__header">
        <span className="usage-dial__name">{usage.displayName}</span>
        <span
          className={`usage-dial__status usage-dial__status--${status}`}
          title={usageStatusLabel(usage, status === "stale")}
        />
      </div>

      <div className="usage-dial__gauge-wrap">
        <svg className="usage-dial__gauge" viewBox="0 0 100 62" aria-hidden="true" focusable="false">
          {TICKS.map((tick) => (
            <line
              key={tick.fraction}
              className={`usage-dial__tick${tick.major ? " usage-dial__tick--major" : ""}`}
              x1={tick.x1}
              y1={tick.y1}
              x2={tick.x2}
              y2={tick.y2}
            />
          ))}
          <path className="usage-dial__track" d={ARC_PATH} pathLength={100} fill="none" />
          {percent !== null && percent > 0 && (
            <path
              className={`usage-dial__value${danger ? " is-danger" : ""}`}
              d={ARC_PATH}
              pathLength={100}
              fill="none"
              strokeDasharray={`${percent} 100`}
            />
          )}
        </svg>
        <div className="usage-dial__reading">
          <strong className="usage-dial__percent">
            {status === "loading" ? "··" : percent === null ? "—" : `${Math.round(percent)}%`}
          </strong>
          {hasReading && <span className="usage-dial__window">{window.label}</span>}
        </div>
      </div>

      <span
        className={`usage-dial__footer${status === "error" ? " is-error" : ""}`}
        title={status === "error" ? (usage.error ?? undefined) : undefined}
      >
        {dialFooterCopy(usage, status)}
      </span>

      {planLine && <span className="usage-dial__plan">{planLine}</span>}

      {secondaryWindows.length > 0 && (
        <div className="usage-dial__windows">
          {secondaryWindows.map((item) => (
            <div key={item.label} className="usage-dial__window-row">
              <span className="usage-dial__window-label" title={item.label}>{item.label}</span>
              <span className="usage-dial__mini-bar" aria-hidden="true">
                <span
                  className={item.usedPercent >= DANGER_THRESHOLD ? "is-danger" : ""}
                  style={{ width: `${item.usedPercent}%` }}
                />
              </span>
              <span className="usage-dial__window-value">{Math.round(item.usedPercent)}%</span>
            </div>
          ))}
        </div>
      )}

      {status === "disconnected" && onOpenSettings && (
        <button type="button" className="usage-dial__connect" onClick={onOpenSettings}>
          Connect in settings
        </button>
      )}
    </div>
  );
}

export function UsageDials({ onOpenSettings }: { onOpenSettings?: () => void }): ReactElement {
  const { usages, loading, refreshing, error, refresh } = useProviderUsage();

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

      <div className="usage-dials">
        {usages.map((usage) => (
          <UsageDial
            key={usage.provider}
            usage={usage}
            sectionLoading={loading}
            onOpenSettings={onOpenSettings}
          />
        ))}
      </div>
    </section>
  );
}
