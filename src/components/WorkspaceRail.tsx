import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactElement } from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Activity,
  Check,
  ChevronDown,
  CircleOff,
  Cpu,
  Gauge,
  Gamepad2,
  HardDrive,
  History,
  LogOut,
  Mic,
  Maximize2,
  Minimize2,
  Settings,
  Plus,
  RotateCcw,
  TerminalSquare,
  X,
} from "lucide-react";
import { HistoryModal } from "./HistoryModal";
import { SettingsModal, type SettingsSection } from "./SettingsModal";
import { UsageDials } from "./UsageDials";
import { restartApp } from "../lib/terminalApi";
import { formatCacheHitPercent } from "../lib/tokenTelemetry";
import {
  LAUNCH_PROFILE_OPTIONS,
  launchProfileLabel,
  type EffectMode,
  type LaunchProfileId,
  type MemorySnapshot,
  type ProviderUsagePreferences,
  type ThemeId,
  type TerminalPreferences,
  type TokenTelemetry,
  type VoiceInputState,
  type WorkspaceSession,
} from "../types";

interface WorkspaceRailProps {
  sessions: WorkspaceSession[];
  activeId: string | null;
  selectedIds: ReadonlySet<string>;
  maximizedId: string | null;
  metrics: MemorySnapshot;
  telemetry: TokenTelemetry;
  launchProfile: LaunchProfileId;
  isBooting: boolean;
  theme: ThemeId;
  effectMode: EffectMode;
  terminalPreferences: TerminalPreferences;
  providerUsagePreferences: ProviderUsagePreferences;
  notice: string | null;
  controllerConnected: boolean;
  controllerName: string | null;
  controllerVoiceState: VoiceInputState;
  onSelect: (id: string, extendSelection: boolean) => void;
  onAddTerminal: (profile?: LaunchProfileId) => void;
  onToggleMaximize: (id: string) => void;
  onRestart: (id: string) => void;
  onCloseSelected: () => void;
  onNewSession: (id: string) => void;
  onExitSession: (id: string) => void;
  onOpenController: () => void;
  onThemeChange: (theme: ThemeId) => void;
  onEffectModeChange: (mode: EffectMode) => void;
  onTerminalPreferencesChange: (preferences: TerminalPreferences) => void;
  onProviderUsagePreferencesChange: (preferences: ProviderUsagePreferences) => void;
  onDismissNotice: () => void;
}

function formatModel(model: string | null | undefined): string {
  if (!model) return "OMP";
  const segments = model.split("/");
  return segments[segments.length - 1] ?? model;
}

function controllerVoiceMessage(state: VoiceInputState): string {
  switch (state) {
    case "recording":
      return "Recording · Press X to stop";
    case "preview":
      return "Preview ready · Press X to insert";
    case "idle":
      return "Connected · View controls";
    default:
      return "Preparing transcript";
  }
}

const COUNT_UP_DURATION_MS = 620;

/**
 * Exact, locale-formatted token count that interpolates toward new values.
 * Interpolation is skipped entirely under reduced motion.
 */
function CountUpTokens({ value }: { value: number }): ReactElement {
  const [displayed, setDisplayed] = useState(value);
  const displayedRef = useRef(value);
  const [increasing, setIncreasing] = useState(false);

  useEffect(() => {
    const from = displayedRef.current;
    if (from === value) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      displayedRef.current = value;
      setDisplayed(value);
      return;
    }
    setIncreasing(true);
    const flashTimer = window.setTimeout(() => setIncreasing(false), 440);
    const start = performance.now();
    let frame = 0;
    const tick = (now: number) => {
      const progress = Math.min(1, (now - start) / COUNT_UP_DURATION_MS);
      const eased = 1 - (1 - progress) ** 4;
      const current = Math.round(from + (value - from) * eased);
      displayedRef.current = current;
      setDisplayed(current);
      if (progress < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(flashTimer);
    };
  }, [value]);

  return (
    <strong
      className={`token-today__value${increasing ? " is-increasing" : ""}`}
      title={`${value.toLocaleString()} tokens today`}
    >
      {displayed.toLocaleString()}
    </strong>
  );
}

export function WorkspaceRail({
  sessions,
  activeId,
  selectedIds,
  maximizedId,
  metrics,
  telemetry,
  launchProfile,
  isBooting,
  theme,
  effectMode,
  terminalPreferences,
  providerUsagePreferences,
  notice,
  controllerConnected,
  controllerName,
  controllerVoiceState,
  onSelect,
  onAddTerminal,
  onToggleMaximize,
  onRestart,
  onCloseSelected,
  onNewSession,
  onExitSession,
  onOpenController,
  onThemeChange,
  onEffectModeChange,
  onTerminalPreferencesChange,
  onProviderUsagePreferencesChange,
  onDismissNotice,
}: WorkspaceRailProps): ReactElement {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection | undefined>(undefined);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [profileMenu, setProfileMenu] = useState<{ x: number; y: number } | null>(null);
  const [contextMenu, setContextMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const profileMenuRef = useRef<HTMLDivElement>(null);
  const profileToggleRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!contextMenu) return;
    const closeOnOutside = (event: PointerEvent) => {
      if (!contextMenuRef.current?.contains(event.target as Node)) setContextMenu(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setContextMenu(null);
    };
    window.addEventListener("pointerdown", closeOnOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOnOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [contextMenu]);

  const runContextAction = (action: () => void) => {
    setContextMenu(null);
    action();
  };

  const openProfileMenu = () => {
    const anchor = profileToggleRef.current?.getBoundingClientRect();
    if (!anchor) return;
    setProfileMenu({
      x: Math.min(anchor.right - 188, window.innerWidth - 196),
      y: anchor.bottom + 6,
    });
  };

  useEffect(() => {
    if (!profileMenu) return;
    const closeOnOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (profileMenuRef.current?.contains(target)) return;
      if (profileToggleRef.current?.contains(target)) return;
      setProfileMenu(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setProfileMenu(null);
      profileToggleRef.current?.focus();
    };
    const focusFrame = requestAnimationFrame(() => {
      const items = profileMenuRef.current?.querySelectorAll<HTMLElement>("[role='menuitemradio']");
      const current = profileMenuRef.current?.querySelector<HTMLElement>("[aria-checked='true']");
      (current ?? items?.[0])?.focus();
    });
    window.addEventListener("pointerdown", closeOnOutside);
    window.addEventListener("keydown", closeOnEscape, true);
    return () => {
      cancelAnimationFrame(focusFrame);
      window.removeEventListener("pointerdown", closeOnOutside);
      window.removeEventListener("keydown", closeOnEscape, true);
    };
  }, [profileMenu]);

  const onProfileMenuKeyDown = (event: ReactKeyboardEvent) => {
    const items = Array.from(
      profileMenuRef.current?.querySelectorAll<HTMLElement>("[role='menuitemradio']") ?? [],
    );
    if (items.length === 0) return;
    const index = items.findIndex((item) => item === document.activeElement);
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") nextIndex = (index + 1) % items.length;
    else if (event.key === "ArrowUp") nextIndex = (index - 1 + items.length) % items.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = items.length - 1;
    else if (event.key === "Tab") {
      setProfileMenu(null);
      return;
    }
    if (nextIndex !== null) {
      event.preventDefault();
      items[nextIndex]?.focus();
    }
  };

  const launchWithProfile = (profile: LaunchProfileId) => {
    setProfileMenu(null);
    onAddTerminal(profile);
  };
  const totalMemory = metrics.appMemoryMib + metrics.terminalMemoryMib;
  const launchProfileName = launchProfileLabel(launchProfile);
  const activeSession = activeId
    ? sessions.find((session) => session.id === activeId)
    : null;
  const contextMenuSession = contextMenu
    ? sessions.find((session) => session.id === contextMenu.id)
    : null;
  const frameDragStart = useRef<{ x: number; y: number } | null>(null);

  return (
    <>
      <aside className="workspace-rail" aria-label="UltraTerm control center">
        <div className="workspace-rail__material" aria-hidden="true">
          <span className="workspace-rail__specular" />
          <span className="workspace-rail__refraction" />
        </div>

        <header
          className="workspace-rail__header"
          onDoubleClick={(event) => {
            if ((event.target as HTMLElement).closest("button")) return;
            void getCurrentWindow().toggleMaximize();
          }}
          onPointerDown={(event) => {
            if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
            frameDragStart.current = { x: event.screenX, y: event.screenY };
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => {
            const start = frameDragStart.current;
            if (!start || Math.hypot(event.screenX - start.x, event.screenY - start.y) < 4) return;
            frameDragStart.current = null;
            event.currentTarget.releasePointerCapture(event.pointerId);
            void getCurrentWindow().startDragging();
          }}
          onPointerUp={(event) => {
            frameDragStart.current = null;
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId);
            }
          }}
          onPointerCancel={() => {
            frameDragStart.current = null;
          }}
          onMouseDown={(event) => {
            if (event.button === 0 && !(event.target as HTMLElement).closest("button")) {
              event.preventDefault();
            }
          }}
        >
          <div className="window-controls" aria-label="Window controls">
            <button
              type="button"
              className="window-control window-control--close"
              aria-label="Close UltraTerm"
              onMouseDown={(event) => event.stopPropagation()}
              onClick={() => void getCurrentWindow().close()}
            />
            <button
              type="button"
              className="window-control window-control--minimize"
              aria-label="Minimize UltraTerm"
              onMouseDown={(event) => event.stopPropagation()}
              onClick={() => void getCurrentWindow().minimize()}
            />
            <button
              type="button"
              className="window-control window-control--zoom"
              aria-label="Zoom UltraTerm"
              onMouseDown={(event) => event.stopPropagation()}
              onClick={() => void getCurrentWindow().toggleMaximize()}
            />
          </div>
          <div className="brand-mark">
            <strong className="brand-wordmark">UltraTerm</strong>
            <small>By Implose Labs</small>
          </div>
        </header>

        <div className="workspace-rail__scroll">
          {notice && (
            <div className="sidebar-notice" role="alert">
              <div>
                <strong>UltraTerm needs attention</strong>
                <p>{notice}</p>
              </div>
              <button type="button" aria-label="Dismiss notice" onClick={onDismissNotice}>
                <X size={13} />
              </button>
            </div>
          )}

          <section className="sidebar-section sidebar-section--terminals" aria-label="Terminals">
            <div className="sidebar-section__heading sidebar-section__heading--actions-only">
              <div className="terminal-header-actions">
                <div className="terminal-launch">
                  <button
                    type="button"
                    className="glass-icon-button glass-icon-button--labeled terminal-launch__primary"
                    onClick={() => onAddTerminal()}
                    disabled={isBooting || sessions.length >= metrics.maxSessions}
                    aria-label={`New terminal with ${launchProfileName} profile`}
                    title={`New terminal · ⌘T · ${launchProfileName} profile`}
                  >
                    <Plus size={13} />
                    <span>New terminal</span>
                    <small>{launchProfileName}</small>
                  </button>
                  <button
                    type="button"
                    ref={profileToggleRef}
                    className="glass-icon-button terminal-launch__toggle"
                    onClick={() => (profileMenu ? setProfileMenu(null) : openProfileMenu())}
                    disabled={isBooting || sessions.length >= metrics.maxSessions}
                    aria-haspopup="menu"
                    aria-expanded={profileMenu !== null}
                    aria-label="Choose launch profile"
                    title="Choose launch profile"
                  >
                    <ChevronDown size={12} />
                  </button>
                </div>
              </div>
            </div>

            <div className="terminal-list" aria-label="Terminal windows">
              {sessions.length === 0 ? (
                <div className="terminal-list__empty">
                  <CircleOff size={14} />
                  <span>No terminals attached</span>
                </div>
              ) : (
                sessions.map((session) => {
                  const tokens = telemetry.terminals.find((item) => item.slot === session.slot);
                  const profileName = launchProfileLabel(session.launchProfile);
                  const stateDetail = session.status === "live"
                    ? `${formatModel(tokens?.model)} · ${session.activity}`
                    : session.status;
                  return (
                    <button
                      key={session.id}
                      type="button"
                      className={`terminal-list__item${session.id === activeId ? " is-active" : ""}${selectedIds.has(session.id) ? " is-selected" : ""}${session.activity === "working" ? " is-working" : " is-idle"}`}
                      aria-pressed={selectedIds.has(session.id)}
                      onClick={(event) => onSelect(session.id, event.shiftKey)}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        if (!selectedIds.has(session.id)) onSelect(session.id, false);
                        setContextMenu({
                          id: session.id,
                          x: Math.min(event.clientX, window.innerWidth - 190),
                          y: Math.min(event.clientY, window.innerHeight - 244),
                        });
                      }}
                    >
                      <span className="terminal-list__icon">
                        <TerminalSquare size={14} />
                      </span>
                      <span className="terminal-list__copy">
                        <strong>Terminal {session.slot}</strong>
                        <small title={tokens?.model ?? "OMP model pending"}>{stateDetail}</small>
                      </span>
                      <span className="terminal-list__meta">
                        <small title={`Launch profile: ${profileName}`}>
                          {profileName}
                        </small>
                        <kbd>⌘{session.slot}</kbd>
                      </span>
                    </button>
                  );
                })
              )}
            </div>

            {activeId && (
              <div
                className="terminal-actions"
                aria-label={selectedIds.size > 1
                  ? `${selectedIds.size} selected terminal actions`
                  : `Terminal ${activeSession?.slot ?? ""} actions`}
              >
                <button type="button" onClick={() => onNewSession(activeId)}>
                  <Plus size={11} /> New session
                </button>
                <button type="button" onClick={() => onExitSession(activeId)}>
                  <LogOut size={11} /> Exit session
                </button>
                <button type="button" onClick={() => onRestart(activeId)}>
                  <RotateCcw size={11} /> Reconnect
                </button>
                <button type="button" onClick={() => onToggleMaximize(activeId)}>
                  {maximizedId === activeId ? <Minimize2 size={11} /> : <Maximize2 size={11} />}
                  {maximizedId === activeId ? "Restore" : "Focus"}
                </button>
                <div className="terminal-actions__footer">
                  <button type="button" className="is-danger" onClick={onCloseSelected}>
                    <X size={11} /> {selectedIds.size > 1 ? `Close ${selectedIds.size}` : "Close"}
                  </button>
                  <button
                    type="button"
                    className="terminal-actions__settings"
                    onClick={() => setSettingsOpen(true)}
                    aria-label="Open settings"
                    title="Settings"
                  >
                    <Settings size={12} />
                  </button>
                </div>
              </div>
            )}
            {controllerConnected && (
              <button
                type="button"
                className={`controller-status-button${controllerVoiceState !== "idle" ? " is-recording" : ""}`}
                onClick={onOpenController}
                title={controllerName ?? "PS4 Controller"}
              >
                <Gamepad2 size={14} />
                <span>
                  <strong>PS4 Controller</strong>
                  <small>{controllerVoiceMessage(controllerVoiceState)}</small>
                </span>
                {controllerVoiceState !== "idle" && <Mic size={13} aria-label="Voice input active" />}
              </button>
            )}
          </section>

          <section className="sidebar-section sidebar-section--telemetry" aria-labelledby="telemetry-heading">
            <div className="sidebar-section__heading">
              <span id="telemetry-heading"><Gauge size={13} /> Token telemetry</span>
              <small>{metrics.sessionCount} live</small>
            </div>

            <div
              className="token-cache-summary"
              title="Cached input tokens divided by all input-side tokens during the past 24 hours"
            >
              <span>24h cache hit</span>
              <strong>{formatCacheHitPercent(telemetry.past24Hours)}</strong>
            </div>

            <div className="token-today">
              <span className="token-today__label">Today</span>
              <CountUpTokens value={telemetry.today.total} />
              <button
                type="button"
                className="token-today__history"
                onClick={() => setHistoryOpen(true)}
                aria-label="View full token usage history"
              >
                <History size={11} />
                <span>View history</span>
              </button>
            </div>
          </section>

          <UsageDials
            preferences={providerUsagePreferences}
            onOpenSettings={() => {
              setSettingsSection("integrations");
              setSettingsOpen(true);
            }}
          />
        </div>

        <footer className="workspace-rail__system" aria-label="System memory">
          <div className="sidebar-section__heading">
            <span><Cpu size={13} /> System</span>
            <button
              type="button"
              className="workspace-rail__restart"
              onClick={() => {
                void restartApp().catch((error) => {
                  console.error("UltraTerm restart failed", error);
                });
              }}
              aria-label="Restart UltraTerm"
              title="Restart UltraTerm — terminals keep running and reattach"
            >
              <RotateCcw size={11} />
              <span>Restart</span>
            </button>
          </div>
          <dl className="system-metrics">
            <div>
              <dt><Gauge size={12} /> UltraTerm</dt>
              <dd>{metrics.appMemoryMib.toFixed(0)} MB</dd>
            </div>
            <div>
              <dt><HardDrive size={12} /> Terminals</dt>
              <dd>{metrics.terminalMemoryMib.toFixed(0)} MB</dd>
            </div>
            <div>
              <dt><Activity size={12} /> Total</dt>
              <dd>{totalMemory.toFixed(0)} MB</dd>
            </div>
          </dl>
        </footer>
      </aside>
      {profileMenu && createPortal(
        <div
          ref={profileMenuRef}
          className="terminal-context-menu terminal-context-menu--profiles"
          role="menu"
          aria-label="Launch profile"
          style={{ left: profileMenu.x, top: profileMenu.y }}
          onKeyDown={onProfileMenuKeyDown}
        >
          <strong>Launch profile</strong>
          {LAUNCH_PROFILE_OPTIONS.map((option) => (
            <button
              key={option.id}
              type="button"
              role="menuitemradio"
              aria-checked={launchProfile === option.id}
              title={option.description}
              onClick={() => launchWithProfile(option.id)}
            >
              <span className="terminal-context-menu__check" aria-hidden="true">
                {launchProfile === option.id && <Check size={12} />}
              </span>
              {option.label}
            </button>
          ))}
        </div>,
        document.body,
      )}
      {contextMenu && createPortal(
        <div
          ref={contextMenuRef}
          className="terminal-context-menu"
          role="menu"
          aria-label={`Terminal ${contextMenuSession?.slot ?? ""} quick actions`}
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <strong>Terminal {contextMenuSession?.slot}</strong>
          <button type="button" role="menuitem" onClick={() => runContextAction(() => onSelect(contextMenu.id, false))}>
            <TerminalSquare size={12} /> Focus terminal
          </button>
          <button type="button" role="menuitem" onClick={() => runContextAction(() => onNewSession(contextMenu.id))}>
            <Plus size={12} /> New OMP session
          </button>
          <button type="button" role="menuitem" onClick={() => runContextAction(() => onExitSession(contextMenu.id))}>
            <LogOut size={12} /> Exit OMP session
          </button>
          <button type="button" role="menuitem" onClick={() => runContextAction(() => onRestart(contextMenu.id))}>
            <RotateCcw size={12} /> Reset terminal client
          </button>
          <button type="button" role="menuitem" className="is-danger" onClick={() => runContextAction(onCloseSelected)}>
            <X size={12} /> Close selected terminal clients
          </button>
        </div>,
        document.body,
      )}

      <HistoryModal
        open={historyOpen}
        theme={theme}
        telemetry={telemetry}
        onClose={() => setHistoryOpen(false)}
      />

      <SettingsModal
        open={settingsOpen}
        theme={theme}
        effectMode={effectMode}
        terminalPreferences={terminalPreferences}
        providerUsagePreferences={providerUsagePreferences}
        initialSection={settingsSection}
        onThemeChange={onThemeChange}
        onEffectModeChange={onEffectModeChange}
        onTerminalPreferencesChange={onTerminalPreferencesChange}
        onProviderUsagePreferencesChange={onProviderUsagePreferencesChange}
        onClose={() => {
          setSettingsOpen(false);
          setSettingsSection(undefined);
        }}
      />
    </>
  );
}
