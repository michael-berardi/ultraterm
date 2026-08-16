import { useEffect, useRef, type ReactElement } from "react";
import { createPortal } from "react-dom";
import { ChartNoAxesCombined } from "lucide-react";
import type { ThemeId } from "../types";

interface TelemetryConsentProps {
  theme: ThemeId;
  onChoice: (enabled: boolean) => void;
}

/**
 * One-time anonymous-telemetry consent shown on first launch. Either choice
 * is persisted and the prompt never appears again; the setting can be
 * revisited under Settings → Privacy.
 */
export function TelemetryConsent({ theme, onChoice }: TelemetryConsentProps): ReactElement {
  const modalRef = useRef<HTMLDivElement>(null);
  const allowButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const frame = requestAnimationFrame(() => allowButtonRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onChoice(false);
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        modalRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])") ?? [],
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
    };
  }, [onChoice]);

  return createPortal(
    <div className="update-prompt-overlay" data-theme={theme} role="presentation">
      <div
        ref={modalRef}
        className="update-prompt"
        role="dialog"
        aria-modal="true"
        aria-labelledby="telemetry-consent-title"
        aria-describedby="telemetry-consent-detail"
      >
        <header className="update-prompt__header">
          <span className="update-prompt__icon" aria-hidden="true">
            <ChartNoAxesCombined size={17} />
          </span>
          <div>
            <h2 id="telemetry-consent-title">Help improve UltraTerm</h2>
            <p id="telemetry-consent-detail">
              Share anonymous usage stats — app version, OS, and terminal counts — to help
              guide development. Never names, paths, prompts, or terminal content. Optional;
              change anytime under Settings → Privacy.
            </p>
          </div>
        </header>
        <footer className="update-prompt__actions">
          <span className="update-prompt__spacer" />
          <button type="button" onClick={() => onChoice(false)}>No thanks</button>
          <button
            ref={allowButtonRef}
            type="button"
            className="is-primary"
            onClick={() => onChoice(true)}
          >
            Share anonymous stats
          </button>
        </footer>
      </div>
    </div>,
    document.body,
  );
}
