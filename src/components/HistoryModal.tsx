import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { createPortal } from "react-dom";
import { ChartColumn, X } from "lucide-react";
import { formatCacheHitPercent } from "../lib/tokenTelemetry";
import type { ThemeId, TokenCounts, TokenTelemetry } from "../types";

interface HistoryModalProps {
  open: boolean;
  theme: ThemeId;
  telemetry: TokenTelemetry;
  onClose: () => void;
}

type HistoryRange = "7d" | "30d" | "all";

const RANGE_OPTIONS: ReadonlyArray<{ id: HistoryRange; label: string; days: number | null }> = [
  { id: "7d", label: "7 days", days: 7 },
  { id: "30d", label: "30 days", days: 30 },
  { id: "all", label: "All", days: null },
];

/** Chromatic color is reserved for chart semantics; models repeat it in legend, bars, and table. */
const MODEL_PALETTE = [
  "#7aa2f7",
  "#9ece6a",
  "#bb9af7",
  "#e0af68",
  "#f7768e",
  "#7dcfff",
  "#ff9e64",
  "#b4f9f8",
];

const CHART_WIDTH = 720;
const CHART_HEIGHT = 190;
const CHART_PAD = { top: 12, right: 10, bottom: 28, left: 48 };

const dayLabelFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
});
const dayFullFormatter = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
});

/** History dates are local calendar days in YYYY-MM-DD form; parse them as local dates. */
function parseHistoryDate(date: string): Date {
  const [year, month, day] = date.split("-").map(Number);
  return new Date(year ?? 1970, (month ?? 1) - 1, day ?? 1);
}

function formatCompact(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return value.toLocaleString();
}

function emptyCounts(): TokenCounts {
  return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };
}

function addCounts(target: TokenCounts, source: TokenCounts): void {
  target.input += source.input;
  target.output += source.output;
  target.cacheRead += source.cacheRead;
  target.cacheWrite += source.cacheWrite;
  target.total += source.total;
}

interface ModelSeries {
  model: string;
  color: string;
  totals: TokenCounts;
}

export function HistoryModal({
  open,
  theme,
  telemetry,
  onClose,
}: HistoryModalProps): ReactElement | null {
  const [range, setRange] = useState<HistoryRange>("7d");
  const [selectedModel, setSelectedModel] = useState<string | null>(null);
  const modalRef = useRef<HTMLElement>(null);
  const initialFocusRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const days = useMemo(() => {
    const sorted = [...telemetry.history].sort((left, right) => left.date.localeCompare(right.date));
    const option = RANGE_OPTIONS.find((candidate) => candidate.id === range);
    if (!option || option.days === null) return sorted;
    const cutoff = new Date();
    cutoff.setHours(0, 0, 0, 0);
    cutoff.setDate(cutoff.getDate() - (option.days - 1));
    return sorted.filter((day) => parseHistoryDate(day.date) >= cutoff);
  }, [range, telemetry.history]);

  const models = useMemo<ModelSeries[]>(() => {
    const byModel = new Map<string, TokenCounts>();
    for (const day of days) {
      for (const entry of day.models) {
        const totals = byModel.get(entry.model) ?? emptyCounts();
        addCounts(totals, entry.usage);
        byModel.set(entry.model, totals);
      }
    }
    return Array.from(byModel, ([model, totals]) => ({ model, totals, color: "" }))
      .sort((left, right) => right.totals.total - left.totals.total)
      .map((entry, index) => ({ ...entry, color: MODEL_PALETTE[index % MODEL_PALETTE.length] }));
  }, [days]);

  const totals = useMemo<TokenCounts>(() => {
    const aggregate = emptyCounts();
    for (const day of days) addCounts(aggregate, day.usage);
    return aggregate;
  }, [days]);

  const colorByModel = useMemo(
    () => new Map(models.map((entry) => [entry.model, entry.color])),
    [models],
  );

  // A range change can drop the selected model entirely; clear the filter
  // rather than showing an empty view with a hidden cause.
  useEffect(() => {
    if (selectedModel && !models.some((entry) => entry.model === selectedModel)) {
      setSelectedModel(null);
    }
  }, [models, selectedModel]);

  const visibleModels = useMemo(
    () => selectedModel
      ? models.filter((entry) => entry.model === selectedModel)
      : models,
    [models, selectedModel],
  );

  const visibleTotals = useMemo<TokenCounts>(() => {
    if (!selectedModel) return totals;
    return models.find((entry) => entry.model === selectedModel)?.totals ?? emptyCounts();
  }, [models, selectedModel, totals]);

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusFrame = requestAnimationFrame(() => initialFocusRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab" || !modalRef.current) return;

      const focusable = Array.from(
        modalRef.current.querySelectorAll<HTMLElement>(
          "button:not(:disabled), input:not(:disabled), [href], [tabindex]:not([tabindex='-1'])",
        ),
      );
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;

      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      } else if (!modalRef.current.contains(document.activeElement)) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(focusFrame);
      window.removeEventListener("keydown", handleKeyDown);
      previousFocusRef.current?.focus();
    };
  }, [open]);

  if (!open) return null;

  const chartInnerWidth = CHART_WIDTH - CHART_PAD.left - CHART_PAD.right;
  const chartInnerHeight = CHART_HEIGHT - CHART_PAD.top - CHART_PAD.bottom;
  const baseline = CHART_PAD.top + chartInnerHeight;
  const dayVisibleTotal = (day: (typeof days)[number]): number => visibleModels.reduce(
    (sum, entry) => sum + (day.models.find((item) => item.model === entry.model)?.usage.total ?? 0),
    0,
  );
  const maxDayTotal = Math.max(1, ...days.map(dayVisibleTotal));
  const barSlot = days.length > 0 ? chartInnerWidth / days.length : chartInnerWidth;
  const barWidth = Math.max(3, Math.min(30, barSlot * 0.62));
  const tickLabelEvery = Math.max(1, Math.ceil(days.length / 8));

  return createPortal(
    <div className="settings-overlay" data-theme={theme} role="presentation">
      <section
        className="history-modal"
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="history-title"
      >
        <div className="settings-modal__material" aria-hidden="true" />
        <header className="history-modal__header">
          <div>
            <h2 id="history-title"><ChartColumn size={17} /> Token history</h2>
            <p>Daily OMP token usage split by model — click a model to filter</p>
          </div>
          <button type="button" aria-label="Close token history" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="history-modal__toolbar">
          <div className="history-range" role="group" aria-label="History range">
            {RANGE_OPTIONS.map((option, index) => (
              <button
                key={option.id}
                type="button"
                ref={index === 0 ? initialFocusRef : undefined}
                className={range === option.id ? "is-active" : ""}
                aria-pressed={range === option.id}
                onClick={() => setRange(option.id)}
              >
                {option.label}
              </button>
            ))}
          </div>
          {models.length > 0 && (
            <ul className="history-legend" aria-label="Models — click to filter">
              {models.map((entry) => {
                const active = selectedModel === entry.model;
                return (
                  <li key={entry.model}>
                    <button
                      type="button"
                      className={active ? "is-active" : ""}
                      aria-pressed={active}
                      title={active ? `Clear the ${entry.model} filter` : `Filter to ${entry.model}`}
                      onClick={() => setSelectedModel(active ? null : entry.model)}
                    >
                      <i style={{ background: entry.color }} aria-hidden="true" />
                      <span>{entry.model}</span>
                      <strong>{entry.totals.total.toLocaleString()}</strong>
                    </button>
                  </li>
                );
              })}
              {selectedModel && (
                <li>
                  <button
                    type="button"
                    className="history-legend__clear"
                    onClick={() => setSelectedModel(null)}
                  >
                    Clear filter ×
                  </button>
                </li>
              )}
            </ul>
          )}
        </div>

        {days.length === 0 ? (
          <div className="history-modal__empty">
            <ChartColumn size={18} />
            <strong>No token activity in this window</strong>
            <span>Usage appears here once UltraTerm has indexed transcript telemetry for these days.</span>
          </div>
        ) : (
          <>
            <dl className="history-summary" aria-label={selectedModel ? `Totals for ${selectedModel} in selected range` : "Totals for selected range"}>
              <div>
                <dt>Total</dt>
                <dd>{visibleTotals.total.toLocaleString()}</dd>
              </div>
              <div>
                <dt>Input</dt>
                <dd>{visibleTotals.input.toLocaleString()}</dd>
              </div>
              <div>
                <dt>Output</dt>
                <dd>{visibleTotals.output.toLocaleString()}</dd>
              </div>
              <div>
                <dt>Cache read</dt>
                <dd>{visibleTotals.cacheRead.toLocaleString()}</dd>
              </div>
              <div>
                <dt>Cache hit</dt>
                <dd>{formatCacheHitPercent(visibleTotals)}</dd>
              </div>
              <div>
                <dt>Cache write</dt>
                <dd>{visibleTotals.cacheWrite.toLocaleString()}</dd>
              </div>
            </dl>

            <div
              className="history-chart"
              role="img"
              aria-label={`Stacked daily token usage for ${days.length} days, peaking at ${maxDayTotal.toLocaleString()} tokens`}
            >
              <svg
                viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
                preserveAspectRatio="none"
                aria-hidden="true"
                focusable="false"
              >
                {[0.25, 0.5, 0.75, 1].map((fraction) => {
                  const y = baseline - chartInnerHeight * fraction;
                  return (
                    <g key={fraction}>
                      <line
                        className="history-chart__gridline"
                        x1={CHART_PAD.left}
                        y1={y}
                        x2={CHART_WIDTH - CHART_PAD.right}
                        y2={y}
                      />
                      <text className="history-chart__tick" x={CHART_PAD.left - 6} y={y + 3}>
                        {formatCompact(Math.round(maxDayTotal * fraction))}
                      </text>
                    </g>
                  );
                })}
                <line
                  className="history-chart__baseline"
                  x1={CHART_PAD.left}
                  y1={baseline}
                  x2={CHART_WIDTH - CHART_PAD.right}
                  y2={baseline}
                />
                {days.map((day, dayIndex) => {
                  const x = CHART_PAD.left + dayIndex * barSlot + (barSlot - barWidth) / 2;
                  let stack = 0;
                  return (
                    <g key={day.date}>
                      {visibleModels.map((entry) => {
                        const modelDay = day.models.find((item) => item.model === entry.model);
                        const value = modelDay?.usage.total ?? 0;
                        if (value <= 0) return null;
                        const height = (value / maxDayTotal) * chartInnerHeight;
                        const y = baseline - stack - height;
                        stack += height;
                        return (
                          <rect
                            key={entry.model}
                            className="history-chart__bar"
                            x={x}
                            y={y}
                            width={barWidth}
                            height={height}
                            fill={entry.color}
                            role="button"
                            tabIndex={-1}
                            onClick={() => setSelectedModel(
                              selectedModel === entry.model ? null : entry.model,
                            )}
                          >
                            <title>{`${day.date} · ${entry.model} · ${value.toLocaleString()} tokens`}</title>
                          </rect>
                        );
                      })}
                      {dayIndex % tickLabelEvery === 0 && (
                        <text
                          className="history-chart__tick history-chart__tick--date"
                          x={x + barWidth / 2}
                          y={CHART_HEIGHT - 8}
                        >
                          {dayLabelFormatter.format(parseHistoryDate(day.date))}
                        </text>
                      )}
                    </g>
                  );
                })}
              </svg>
            </div>

            <div className="history-table-wrap">
              <table className="history-table">
                <caption>
                  Exact daily token usage by model for the selected range
                </caption>
                <thead>
                  <tr>
                    <th scope="col">Date</th>
                    <th scope="col">Models</th>
                    <th scope="col">Input</th>
                    <th scope="col">Output</th>
                    <th scope="col">Cached</th>
                    <th scope="col">Cache hit</th>
                    <th scope="col">Total</th>
                  </tr>
                </thead>
                <tbody>
                  {[...days].reverse().map((day) => {
                    const rowModels = selectedModel
                      ? day.models.filter((entry) => entry.model === selectedModel)
                      : day.models;
                    if (selectedModel && rowModels.length === 0) return null;
                    const rowUsage = selectedModel ? rowModels[0].usage : day.usage;
                    return (
                      <tr key={day.date}>
                        <th scope="row">
                          <time dateTime={day.date}>
                            {dayFullFormatter.format(parseHistoryDate(day.date))}
                          </time>
                        </th>
                        <td>
                          <ul>
                            {rowModels.map((entry) => (
                              <li key={entry.model}>
                                <button
                                  type="button"
                                  aria-pressed={selectedModel === entry.model}
                                  title={selectedModel === entry.model
                                    ? `Clear the ${entry.model} filter`
                                    : `Filter to ${entry.model}`}
                                  onClick={() => setSelectedModel(
                                    selectedModel === entry.model ? null : entry.model,
                                  )}
                                >
                                  <i
                                    style={{ background: colorByModel.get(entry.model) ?? "var(--text-faint)" }}
                                    aria-hidden="true"
                                  />
                                  {entry.model} · {entry.usage.total.toLocaleString()}
                                </button>
                              </li>
                            ))}
                          </ul>
                        </td>
                        <td>{rowUsage.input.toLocaleString()}</td>
                        <td>{rowUsage.output.toLocaleString()}</td>
                        <td>{(rowUsage.cacheRead + rowUsage.cacheWrite).toLocaleString()}</td>
                        <td>{formatCacheHitPercent(rowUsage)}</td>
                        <td>{rowUsage.total.toLocaleString()}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </>
        )}
      </section>
    </div>,
    document.body,
  );
}
