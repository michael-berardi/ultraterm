import { useEffect, useRef, useState, type ReactElement } from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Activity,
  Bot,
  CircleOff,
  Cpu,
  Gauge,
  Gamepad2,
  HardDrive,
  LogOut,
  Mic,
  Maximize2,
  Minimize2,
  Settings,
  Plus,
  RotateCcw,
  Sparkles,
  TerminalSquare,
  X,
} from "lucide-react";
import { SettingsModal, type SettingsSection } from "./SettingsModal";
import { UsageDials } from "./UsageDials";
import type {
  EffectMode,
  MemorySnapshot,
  ThemeId,
  TerminalPreferences,
  TokenTelemetry,
  VoiceInputState,
  WorkspaceSession,
} from "../types";


interface WorkspaceRailProps {
  sessions: WorkspaceSession[];
  activeId: string | null;
  selectedIds: ReadonlySet<string>;
  maximizedId: string | null;
  metrics: MemorySnapshot;
  telemetry: TokenTelemetry;
  isBooting: boolean;
  theme: ThemeId;
  effectMode: EffectMode;
  terminalPreferences: TerminalPreferences;
  notice: string | null;
  controllerConnected: boolean;
  controllerName: string | null;
  controllerVoiceState: VoiceInputState;
  onSelect: (id: string, extendSelection: boolean) => void;
  onAddTerminal: () => void;
  onToggleMaximize: (id: string) => void;
  onRestart: (id: string) => void;
  onCloseSelected: () => void;
  onNewSession: (id: string) => void;
  onExitSession: (id: string) => void;
  onOpenController: () => void;
  onThemeChange: (theme: ThemeId) => void;
  onEffectModeChange: (mode: EffectMode) => void;
  onTerminalPreferencesChange: (preferences: TerminalPreferences) => void;
  onDismissNotice: () => void;
}

function formatTokens(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return value.toLocaleString();
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

function IncreasingTokenValue({ value, title }: { value: number; title: string }): ReactElement {
  const previousValue = useRef(value);
  const [increasing, setIncreasing] = useState(false);

  useEffect(() => {
    if (value <= previousValue.current) {
      previousValue.current = value;
      return;
    }
    previousValue.current = value;
    setIncreasing(true);
    const timer = window.setTimeout(() => setIncreasing(false), 440);
    return () => window.clearTimeout(timer);
  }, [value]);

  return (
    <strong className={increasing ? "is-increasing" : ""} title={title}>
      {formatTokens(value)}
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
  isBooting,
  theme,
  effectMode,
  terminalPreferences,
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
  onDismissNotice,
}: WorkspaceRailProps): ReactElement {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection | undefined>(undefined);
  const [contextMenu, setContextMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
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
  const totalMemory = metrics.appMemoryMib + metrics.terminalMemoryMib;
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
                <button
                  type="button"
                  className="glass-icon-button"
                  onClick={() => {
                    setSettingsSection("appearance");
                    setSettingsOpen(true);
                  }}
                  aria-label="Open settings"
                  title="Settings"
                >
                  <Settings size={13} />
                </button>
                <button
                  type="button"
                  className="glass-icon-button glass-icon-button--labeled"
                  onClick={onAddTerminal}
                  disabled={isBooting || sessions.length >= metrics.maxSessions}
                  aria-label="New terminal"
                  title="New terminal · ⌘T"
                >
                  <Plus size={13} />
                  <span>New terminal</span>
                </button>
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
                  const agentCount = tokens?.activeSubagents ?? 0;
                  const stateDetail = session.status === "live"
                    ? `${formatModel(tokens?.model)} · ${session.activity}${agentCount > 0 ? ` · ${agentCount} ${agentCount === 1 ? "agent" : "agents"}` : ""}`
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
                        <small>{formatTokens(tokens?.usage.total ?? 0)}</small>
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

            <div className="token-summary">
              <div>
                <span>Past 24hr</span>
                <IncreasingTokenValue
                  value={telemetry.past24Hours.total}
                  title={`${formatTokens(telemetry.past24Hours.cacheRead)} cached context tokens reused`}
                />
              </div>
              <div>
                <span>Past 7d</span>
                <IncreasingTokenValue
                  value={telemetry.past7Days.total}
                  title={`${formatTokens(telemetry.past7Days.cacheRead)} cached context tokens reused`}
                />
              </div>
              <div>
                <span>All time</span>
                <IncreasingTokenValue
                  value={telemetry.allTime.total}
                  title={`${formatTokens(telemetry.allTime.cacheRead)} cached context tokens reused`}
                />
              </div>
            </div>

            <div className="token-terminals">
              {telemetry.terminals.filter((terminal) => sessions.some((session) => session.slot === terminal.slot)).map((terminal) => (
                <div key={terminal.slot}>
                  <span>Terminal {terminal.slot}</span>
                  <span>
                    {terminal.activeSubagents > 0 && <i aria-label={`${terminal.activeSubagents} active sub-agents`} />}
                    {formatTokens(terminal.usage.total)}
                  </span>
                </div>
              ))}
            </div>

            <div className="agent-glance" aria-label="Sub-agent and delegation activity">
              <div className={telemetry.activeSubagents > 0 ? "is-live" : ""}>
                <Activity size={12} />
                <span>Active</span>
                <strong>{telemetry.activeSubagents}</strong>
              </div>
              <div>
                <Bot size={12} />
                <span>Finished</span>
                <strong>{telemetry.inactiveSubagents}</strong>
              </div>
              <div>
                <Sparkles size={12} />
                <span>Records</span>
                <strong>{telemetry.activeSubagents + telemetry.inactiveSubagents}</strong>
              </div>
            </div>
          </section>

          <UsageDials
            onOpenSettings={() => {
              setSettingsSection("integrations");
              setSettingsOpen(true);
            }}
          />
        </div>

        <footer className="workspace-rail__system" aria-label="System memory">
          <div className="sidebar-section__heading">
            <span><Cpu size={13} /> System</span>
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

      <SettingsModal
        open={settingsOpen}
        theme={theme}
        effectMode={effectMode}
        terminalPreferences={terminalPreferences}
        initialSection={settingsSection}
        onThemeChange={onThemeChange}
        onEffectModeChange={onEffectModeChange}
        onTerminalPreferencesChange={onTerminalPreferencesChange}
        onClose={() => {
          setSettingsOpen(false);
          setSettingsSection(undefined);
        }}
      />
    </>
  );
}
