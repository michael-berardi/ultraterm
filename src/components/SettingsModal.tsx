import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  KeyRound,
  Palette,
  ShieldCheck,
  TerminalSquare,
  X,
} from "lucide-react";
import { IntegrationsSettings } from "./IntegrationsSettings";
import type {
  AppTelemetryConsent,
  ProviderUsagePreferences,
  TerminalCursorStyle,
  TerminalPreferences,
  ThemeId,
} from "../types";

const THEMES: Array<{ id: ThemeId; name: string; detail: string }> = [
  { id: "oled", name: "Obsidian OLED", detail: "Pure black · pixel-crisp contrast" },
  { id: "white", name: "White", detail: "Pure white · crisp dark contrast" },
  { id: "titanium", name: "Titanium", detail: "Machined silver · cool depth" },
  { id: "aurora", name: "Aurora", detail: "Polar color · midnight glass" },
  { id: "ember", name: "Ember", detail: "Warm carbon · quiet radiance" },
];

const FONT_SIZES = [9, 10, 11, 12, 13, 14, 15, 16, 17, 18] as const;
const CURSOR_STYLES: Array<{
  id: TerminalCursorStyle;
  name: string;
}> = [
  { id: "bar", name: "Bar" },
  { id: "block", name: "Block" },
  { id: "underline", name: "Underline" },
];

export type SettingsSection = "appearance" | "terminal" | "integrations" | "privacy";

interface SettingsModalProps {
  open: boolean;
  theme: ThemeId;
  terminalPreferences: TerminalPreferences;
  providerUsagePreferences: ProviderUsagePreferences;
  telemetryConsent: AppTelemetryConsent;
  /** When set, the modal jumps to this section the next time it opens. */
  initialSection?: SettingsSection;
  onThemeChange: (theme: ThemeId) => void;
  onTerminalPreferencesChange: (preferences: TerminalPreferences) => void;
  onProviderUsagePreferencesChange: (preferences: ProviderUsagePreferences) => void;
  onTelemetryConsentChange: (enabled: boolean) => void;
  onClose: () => void;
}

export function SettingsModal({
  open,
  theme,
  terminalPreferences,
  providerUsagePreferences,
  telemetryConsent,
  initialSection,
  onThemeChange,
  onTerminalPreferencesChange,
  onProviderUsagePreferencesChange,
  onTelemetryConsentChange,
  onClose,
}: SettingsModalProps) {
  const [activeSection, setActiveSection] = useState<SettingsSection>("appearance");
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const modalRef = useRef<HTMLElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusFrame = requestAnimationFrame(() => closeButtonRef.current?.focus());
    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab" || !modalRef.current) return;

      const focusable = Array.from(
        modalRef.current.querySelectorAll<HTMLElement>(
          "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex='-1'])",
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
    if (open && initialSection) setActiveSection(initialSection);
  }, [open, initialSection]);

  const handleSectionKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const tabs = Array.from(
      event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("[role='tab']") ?? [],
    );
    const currentIndex = tabs.indexOf(event.currentTarget);
    let nextIndex = currentIndex;

    if (event.key === "ArrowDown" || event.key === "ArrowRight") {
      nextIndex = (currentIndex + 1) % tabs.length;
    } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
      nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = tabs.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    tabs[nextIndex]?.focus();
    tabs[nextIndex]?.click();
  };

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
        className="settings-modal"
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <div className="settings-modal__material" aria-hidden="true" />
        <header className="settings-modal__header">
          <div>
            <h2 id="settings-title">Settings</h2>
            <p>Workspace preferences</p>
          </div>
          <button ref={closeButtonRef} type="button" aria-label="Close settings" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="settings-modal__body">
          <nav
            className="settings-sidebar"
            role="tablist"
            aria-label="Settings sections"
            aria-orientation="vertical"
          >
            <button
              id="settings-tab-appearance"
              type="button"
              role="tab"
              aria-selected={activeSection === "appearance"}
              aria-controls="settings-panel-appearance"
              tabIndex={activeSection === "appearance" ? 0 : -1}
              onClick={() => setActiveSection("appearance")}
              onKeyDown={handleSectionKeyDown}
            >
              <Palette size={15} aria-hidden="true" />
              Appearance
            </button>
            <button
              id="settings-tab-terminal"
              type="button"
              role="tab"
              aria-selected={activeSection === "terminal"}
              aria-controls="settings-panel-terminal"
              tabIndex={activeSection === "terminal" ? 0 : -1}
              onClick={() => setActiveSection("terminal")}
              onKeyDown={handleSectionKeyDown}
            >
              <TerminalSquare size={15} aria-hidden="true" />
              Terminal
            </button>
            <button
              id="settings-tab-integrations"
              type="button"
              role="tab"
              aria-selected={activeSection === "integrations"}
              aria-controls="settings-panel-integrations"
              tabIndex={activeSection === "integrations" ? 0 : -1}
              onClick={() => setActiveSection("integrations")}
              onKeyDown={handleSectionKeyDown}
            >
              <KeyRound size={15} aria-hidden="true" />
              Integrations
            </button>
            <button
              id="settings-tab-privacy"
              type="button"
              role="tab"
              aria-selected={activeSection === "privacy"}
              aria-controls="settings-panel-privacy"
              tabIndex={activeSection === "privacy" ? 0 : -1}
              onClick={() => setActiveSection("privacy")}
              onKeyDown={handleSectionKeyDown}
            >
              <ShieldCheck size={15} aria-hidden="true" />
              Privacy
            </button>
          </nav>

          {activeSection === "privacy" ? (
            <section
              id="settings-panel-privacy"
              className="settings-panel"
              role="tabpanel"
              aria-labelledby="settings-tab-privacy"
              tabIndex={0}
            >
              <header className="settings-section-intro">
                <h3>Privacy</h3>
                <p>What UltraTerm shares, and what it never touches.</p>
              </header>

              <section className="settings-group" aria-labelledby="telemetry-heading">
                <div>
                  <h4 id="telemetry-heading">Anonymous usage stats</h4>
                  <p>
                    Sends only the app version, OS, and terminal counts to
                    analytics.libertydesign.studio to help guide development. Never names,
                    file paths, prompts, or terminal content.
                  </p>
                </div>
                <label className="settings-toggle">
                  <input
                    type="checkbox"
                    checked={telemetryConsent === "enabled"}
                    onChange={(event) => onTelemetryConsentChange(event.target.checked)}
                  />
                  <span>
                    <strong>Help improve UltraTerm</strong>
                    <small>Anonymous and optional — a launch ping plus one heartbeat per day.</small>
                  </span>
                </label>
              </section>
            </section>
          ) : activeSection === "integrations" ? (
            <section
              id="settings-panel-integrations"
              className="settings-panel"
              role="tabpanel"
              aria-labelledby="settings-tab-integrations"
              tabIndex={0}
            >
              <IntegrationsSettings
                preferences={providerUsagePreferences}
                onPreferencesChange={onProviderUsagePreferencesChange}
              />
            </section>
          ) : activeSection === "appearance" ? (
            <section
              id="settings-panel-appearance"
              className="settings-panel"
              role="tabpanel"
              aria-labelledby="settings-tab-appearance"
              tabIndex={0}
            >
              <header className="settings-section-intro">
                <h3>Appearance</h3>
                <p>Choose the material and motion used around your terminals.</p>
              </header>

              <section className="settings-group" aria-labelledby="material-heading">
                <div>
                  <h4 id="material-heading">Material</h4>
                  <p>Set the color and depth of the workspace.</p>
                </div>
                <div className="settings-option-grid">
                  {THEMES.map((option) => (
                    <button
                      key={option.id}
                      type="button"
                      className={`settings-choice settings-choice--theme-${option.id}${theme === option.id ? " is-selected" : ""}`}
                      aria-pressed={theme === option.id}
                      onClick={() => onThemeChange(option.id)}
                    >
                      <span className="settings-choice__theme-preview" aria-hidden="true" />
                      <span>
                        <strong>{option.name}</strong>
                        <small>{option.detail}</small>
                      </span>
                      {theme === option.id && <Check size={14} aria-hidden="true" />}
                    </button>
                  ))}
                </div>
              </section>
            </section>
          ) : (
            <section
              id="settings-panel-terminal"
              className="settings-panel"
              role="tabpanel"
              aria-labelledby="settings-tab-terminal"
              tabIndex={0}
            >
              <header className="settings-section-intro">
                <h3>Terminal</h3>
                <p>Changes apply immediately to every open terminal.</p>
              </header>

              <div className="settings-preference-group">
                <div className="settings-preference-row">
                  <label htmlFor="terminal-font-size">
                    <strong>Font size</strong>
                    <small>Adjust terminal text without clearing session output.</small>
                  </label>
                  <select
                    id="terminal-font-size"
                    value={terminalPreferences.fontSize}
                    onChange={(event) => onTerminalPreferencesChange({
                      ...terminalPreferences,
                      fontSize: Number(event.currentTarget.value),
                    })}
                  >
                    {FONT_SIZES.map((fontSize) => (
                      <option key={fontSize} value={fontSize}>{fontSize} px</option>
                    ))}
                  </select>
                </div>

                <div className="settings-preference-row">
                  <label htmlFor="terminal-cursor-style">
                    <strong>Cursor shape</strong>
                    <small>Choose the insertion point shown in each terminal.</small>
                  </label>
                  <select
                    id="terminal-cursor-style"
                    value={terminalPreferences.cursorStyle}
                    onChange={(event) => onTerminalPreferencesChange({
                      ...terminalPreferences,
                      cursorStyle: event.currentTarget.value as TerminalCursorStyle,
                    })}
                  >
                    {CURSOR_STYLES.map((cursorStyle) => (
                      <option key={cursorStyle.id} value={cursorStyle.id}>{cursorStyle.name}</option>
                    ))}
                  </select>
                </div>

                <label className="settings-preference-row settings-preference-row--toggle" htmlFor="terminal-cursor-blink">
                  <span>
                    <strong>Cursor blinking</strong>
                    <small>Animate the cursor while a terminal is focused.</small>
                  </span>
                  <span className="settings-switch">
                    <input
                      id="terminal-cursor-blink"
                      type="checkbox"
                      checked={terminalPreferences.cursorBlink}
                      onChange={(event) => onTerminalPreferencesChange({
                        ...terminalPreferences,
                        cursorBlink: event.currentTarget.checked,
                      })}
                    />
                    <span aria-hidden="true" />
                  </span>
                </label>
              </div>
            </section>
          )}
        </div>
      </section>
    </div>,
    document.body,
  );
}
