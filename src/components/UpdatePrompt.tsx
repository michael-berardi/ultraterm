import { useEffect, useRef, type ReactElement } from "react";
import { createPortal } from "react-dom";
import { ArrowDownToLine, X } from "lucide-react";
import type { AppUpdateStatus, ThemeId } from "../types";
import type { UpdatePhase } from "../hooks/useAppUpdate";

interface UpdatePromptProps {
  theme: ThemeId;
  phase: UpdatePhase;
  status: AppUpdateStatus;
  error: string | null;
  autoUpdate: boolean;
  onAutoUpdateChange: (value: boolean) => void;
  onInstall: () => void;
  onDismiss: () => void;
}

export function UpdatePrompt({
  theme,
  phase,
  status,
  error,
  autoUpdate,
  onAutoUpdateChange,
  onInstall,
  onDismiss,
}: UpdatePromptProps): ReactElement | null {
  const modalRef = useRef<HTMLDivElement>(null);
  const installButtonRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const busy = phase === "installing";

  useEffect(() => {
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const frame = requestAnimationFrame(() => installButtonRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onDismiss();
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        modalRef.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", handleKeyDown);
      previousFocusRef.current?.focus();
    };
  }, [busy, onDismiss]);

  const title = phase === "installing"
    ? `Installing UltraTerm ${status.latestVersion}`
    : phase === "error"
      ? "Update failed"
      : `UltraTerm ${status.latestVersion} is available`;
  const detail = phase === "installing"
    ? "Downloading and verifying the release. UltraTerm relaunches itself — your terminals keep running and reattach."
    : phase === "error"
      ? error ?? "The update could not be installed."
      : `You're running ${status.currentVersion}. Updating takes a few seconds; terminals keep running and reattach after relaunch.`;

  return createPortal(
    <div
      className="update-prompt-overlay"
      data-theme={theme}
      role="presentation"
      onMouseDown={(event) => {
        if (!busy && event.target === event.currentTarget) onDismiss();
      }}
    >
      <div
        ref={modalRef}
        className="update-prompt"
        role="dialog"
        aria-modal="true"
        aria-busy={busy}
        aria-labelledby="update-prompt-title"
        aria-describedby="update-prompt-detail"
      >
        <header className="update-prompt__header">
          <span className="update-prompt__icon" aria-hidden="true">
            <ArrowDownToLine size={17} />
          </span>
          <div>
            <h2 id="update-prompt-title">{title}</h2>
            <p id="update-prompt-detail">{detail}</p>
          </div>
          {!busy && (
            <button type="button" className="update-prompt__close" aria-label="Dismiss update prompt" onClick={onDismiss}>
              <X size={15} />
            </button>
          )}
        </header>
        {phase === "error" ? (
          <footer className="update-prompt__actions">
            <button type="button" onClick={onDismiss}>Later</button>
            <button type="button" className="is-primary" onClick={onInstall}>Retry update</button>
          </footer>
        ) : (
          <footer className="update-prompt__actions">
            <label className="update-prompt__auto">
              <input
                type="checkbox"
                checked={autoUpdate}
                disabled={busy}
                onChange={(event) => onAutoUpdateChange(event.target.checked)}
              />
              <span>Install updates automatically</span>
            </label>
            <span className="update-prompt__spacer" />
            <button type="button" disabled={busy} onClick={onDismiss}>Later</button>
            <button
              ref={installButtonRef}
              type="button"
              className="is-primary"
              disabled={busy}
              onClick={onInstall}
            >
              {busy ? "Installing…" : "Update now"}
            </button>
          </footer>
        )}
      </div>
    </div>,
    document.body,
  );
}
