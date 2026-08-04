import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Clock3, Folder, History, Search, X } from "lucide-react";
import { searchHistory } from "../lib/terminalApi";
import type { HistoryEntry, ThemeId } from "../types";

interface HistoryModalProps {
  open: boolean;
  theme: ThemeId;
  onClose: () => void;
}

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDate(epochSeconds: number): string {
  return dateFormatter.format(new Date(epochSeconds * 1_000));
}

function formatDirectory(cwd: string | null): string {
  if (!cwd) return "Unknown directory";
  const segments = cwd.split("/").filter(Boolean);
  return segments[segments.length - 1] ?? cwd;
}

export function HistoryModal({ open, theme, onClose }: HistoryModalProps) {
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const modalRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusFrame = requestAnimationFrame(() => searchRef.current?.focus());
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

  useEffect(() => {
    if (!open) return;
    let active = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      setError(null);
      void searchHistory(query)
        .then((results) => {
          if (active) setEntries(results);
        })
        .catch((searchError: unknown) => {
          if (active) {
            setEntries([]);
            setError(searchError instanceof Error ? searchError.message : String(searchError));
          }
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    }, query ? 180 : 0);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [open, query]);

  if (!open) return null;

  return createPortal(
    <div
      className="settings-overlay"
      data-theme={theme}
      role="presentation"
      onMouseDown={(event) => {
        if (event.target !== event.currentTarget) return;
        event.preventDefault();
        onClose();
      }}
    >
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
            <h2 id="history-title"><History size={17} /> OMP history</h2>
            <p>Search prompts across prior sessions</p>
          </div>
          <button type="button" aria-label="Close history" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="history-modal__search">
          <Search size={15} aria-hidden="true" />
          <input
            ref={searchRef}
            type="search"
            value={query}
            maxLength={256}
            placeholder="Search prompt history"
            aria-label="Search OMP prompt history"
            onChange={(event) => setQuery(event.target.value)}
          />
          {query && (
            <button type="button" onClick={() => setQuery("")} aria-label="Clear history search">
              <X size={13} />
            </button>
          )}
        </div>

        <div className="history-modal__status" aria-live="polite">
          <span>{loading ? "Searching…" : `${entries.length} ${entries.length === 1 ? "result" : "results"}`}</span>
          <small>Read-only · 20 result limit</small>
        </div>

        <div className="history-modal__results">
          {error ? (
            <div className="history-modal__empty" role="alert">
              <strong>History unavailable</strong>
              <span>{error}</span>
            </div>
          ) : !loading && entries.length === 0 ? (
            <div className="history-modal__empty">
              <Search size={18} />
              <strong>No matching prompts</strong>
              <span>Try fewer or broader terms.</span>
            </div>
          ) : (
            <ol>
              {entries.map((entry) => (
                <li key={entry.id}>
                  <article className="history-entry">
                    <div className="history-entry__meta">
                      <time dateTime={new Date(entry.createdAt * 1_000).toISOString()}>
                        <Clock3 size={11} /> {formatDate(entry.createdAt)}
                      </time>
                      <span title={entry.cwd ?? undefined}>
                        <Folder size={11} /> {formatDirectory(entry.cwd)}
                      </span>
                    </div>
                    <p>{entry.prompt}{entry.truncated ? "…" : ""}</p>
                    {entry.sessionId && <small>Session {entry.sessionId.slice(0, 8)}</small>}
                  </article>
                </li>
              ))}
            </ol>
          )}
        </div>
      </section>
    </div>,
    document.body,
  );
}
