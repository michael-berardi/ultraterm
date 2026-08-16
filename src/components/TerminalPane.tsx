import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "@xterm/xterm/css/xterm.css";
import "./TerminalPane.css";
import { bytesToBase64, resizeSession, scrollSession, writeToSession } from "../lib/terminalApi";
import {
  TERMINAL_APPEARANCE_RESET_OUTPUT,
  ompAppearanceChangeFor,
  ompAppearanceReportFor,
  terminalThemeFor,
} from "../lib/terminalThemes";
import {
  DEFAULT_TERMINAL_PREFERENCES,
  type TerminalController,
  type ThemeId,
  type TerminalPreferences,
  type WorkspaceSession,
} from "../types";

interface TerminalPaneProps {
  session: WorkspaceSession;
  scrollback: number;
  preferences: TerminalPreferences;
  theme: ThemeId;
  active: boolean;
  maximized: boolean;
  exiting: boolean;
  reducedMotion: boolean;
  /** Mount without the entrance animation (used behind the boot splash). */
  suppressEntrance?: boolean;
  onActivate: (id: string) => void;
  onControllerReady: (id: string, controller: TerminalController | null) => void;
  onRestart: (id: string) => void;
  onTerminalResize: (id: string) => void;
}


const TERMINAL_FONT_FAMILY = [
  '"SFMono-Regular"',
  '"Cascadia Code"',
  '"JetBrains Mono"',
  "Menlo",
  '"Symbols Nerd Font Mono"',
  '"Noto Sans Symbols"',
  '"Noto Sans Symbols 2"',
  '"Noto Sans Math"',
  '"Noto Emoji"',
  '"Apple Color Emoji"',
  '"Segoe UI Emoji"',
  "monospace",
].join(", ");
const OMP_FONT_PROBES = [
  { family: "Symbols Nerd Font Mono", glyphs: "\uf00c\uf12a\uf254\ue0b0\uea64\u{f02a0}" },
  { family: "Noto Sans Symbols", glyphs: "ⓘ" },
  { family: "Noto Sans Symbols 2", glyphs: "✔✘⚠⏳⏹❯➤" },
  { family: "Noto Sans Math", glyphs: "⦸⟳⟵" },
  {
    family: "Noto Emoji",
    glyphs: "🗺🏃🎯📁🌳🔍🗑📄🪙💲👻👥💾🖥🆔📦🪝🛠🔌⚖📎📘🎤📷",
  },
] as const;
let terminalFontsReady: Promise<void> | null = null;

function loadBundledTerminalFonts(): Promise<void> {
  terminalFontsReady ??= Promise.all(
    OMP_FONT_PROBES.map(({ family, glyphs }) => (
      document.fonts.load(`400 ${DEFAULT_TERMINAL_PREFERENCES.fontSize}px "${family}"`, glyphs)
    )),
  ).then(() => undefined);
  return terminalFontsReady;
}

const TEXT_ENCODER = new TextEncoder();
const REMOTE_SCROLL_CHUNK_LINES = 24;
const OMP_APPEARANCE_REPORT_DELAY_MS = 150;

function wheelScrollScale(event: WheelEvent, rows: number): number {
  if (event.deltaMode === 0) return 1 / 32;
  if (event.deltaMode === 2) return rows;
  return 1;
}

function nextPendingInputLength(current: number, data: string): number {
  let length = current;
  for (let index = 0; index < data.length; index += 1) {
    const code = data.charCodeAt(index);
    if (code === 0x1b) {
      if (data[index + 1] === "[") {
        index += 2;
        while (index < data.length && !(
          data.charCodeAt(index) >= 0x40 && data.charCodeAt(index) <= 0x7e
        )) index += 1;
      } else {
        index += 1;
      }
    } else if (code === 0x0d || code === 0x0a || code === 0x03 || code === 0x15) {
      length = 0;
    } else if (code === 0x08 || code === 0x7f) {
      length = Math.max(0, length - 1);
    } else if (code >= 0x20) {
      length += 1;
    }
  }
  return length;
}

function terminalName(slot: number): string {
  return `Terminal ${slot}`;
}

export function TerminalPane({
  session,
  scrollback,
  theme,
  preferences,
  active,
  maximized,
  exiting,
  reducedMotion,
  suppressEntrance = false,
  onActivate,
  onControllerReady,
  onRestart,
  onTerminalResize,
}: TerminalPaneProps): ReactElement {
  const hostRef = useRef<HTMLDivElement>(null);
  const [entering, setEntering] = useState(!reducedMotion && !suppressEntrance);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const preferencesRef = useRef(preferences);
  preferencesRef.current = preferences;
  const sessionStatusRef = useRef(session.status);
  sessionStatusRef.current = session.status;
  const appearanceRefreshKeyRef = useRef<string | null>(null);
  const appearanceReportTimerRef = useRef(0);
  const requestOmpAppearanceRefresh = useCallback((selectedTheme: ThemeId) => {
    const terminal = terminalRef.current;
    if (!terminal || sessionStatusRef.current !== "live") return;

    const refreshKey = `${session.id}:${selectedTheme}`;
    if (appearanceRefreshKeyRef.current === refreshKey) return;
    appearanceRefreshKeyRef.current = refreshKey;
    window.clearTimeout(appearanceReportTimerRef.current);
    const fail = (error: unknown) => {
      if (appearanceRefreshKeyRef.current === refreshKey) {
        appearanceRefreshKeyRef.current = null;
      }
      console.error(`UltraTerm could not synchronize OMP appearance for ${terminalName(session.slot)}`, error);
    };

    terminal.write(TERMINAL_APPEARANCE_RESET_OUTPUT, () => {
      if (terminalRef.current !== terminal || sessionStatusRef.current !== "live") {
        if (appearanceRefreshKeyRef.current === refreshKey) {
          appearanceRefreshKeyRef.current = null;
        }
        return;
      }
      const appearanceInput = bytesToBase64(TEXT_ENCODER.encode(ompAppearanceChangeFor(selectedTheme)));
      void writeToSession(session.id, appearanceInput).then(() => {
        appearanceReportTimerRef.current = window.setTimeout(() => {
          appearanceReportTimerRef.current = 0;
          if (
            terminalRef.current !== terminal ||
            sessionStatusRef.current !== "live" ||
            appearanceRefreshKeyRef.current !== refreshKey
          ) return;
          const reportInput = bytesToBase64(TEXT_ENCODER.encode(ompAppearanceReportFor(selectedTheme)));
          void writeToSession(session.id, reportInput).catch(fail);
        }, OMP_APPEARANCE_REPORT_DELAY_MS);
      }).catch(fail);
    });
  }, [session.id, session.slot]);
  useEffect(() => {
    if (reducedMotion) setEntering(false);
  }, [reducedMotion]);


  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const initialPreferences = preferencesRef.current;
    const terminal = new Terminal({
      allowProposedApi: false,
      cursorBlink: initialPreferences.cursorBlink,
      cursorStyle: initialPreferences.cursorStyle,
      customGlyphs: true,
      drawBoldTextInBrightColors: true,
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: initialPreferences.fontSize,
      fontWeight: "400",
      fontWeightBold: "600",
      letterSpacing: 0,
      lineHeight: 1,
      macOptionIsMeta: true,
      minimumContrastRatio: 1,
      rescaleOverlappingGlyphs: true,
      rightClickSelectsWord: true,
      overviewRuler: { width: 5 },
      scrollback,
      scrollOnEraseInDisplay: false,
      scrollSensitivity: 1,
      smoothScrollDuration: 0,
      theme: terminalThemeFor(themeRef.current),
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    fitAddonRef.current = fitAddon;
    terminal.loadAddon(new WebLinksAddon((_event, uri) => {
      void openUrl(uri).catch((error) => {
        console.error(`UltraTerm could not open ${uri}`, error);
      });
    }));

    let pendingInputLength = 0;
    const trackInput = (data: string) => {
      pendingInputLength = nextPendingInputLength(pendingInputLength, data);
    };

    const sendInput = (data: string) => {
      trackInput(data);
      const encoded = bytesToBase64(TEXT_ENCODER.encode(data));
      void writeToSession(session.id, encoded).catch((error) => {
        console.error(`UltraTerm input failed for ${terminalName(session.slot)}`, error);
      });
    };
    let pendingScrollLines = 0;
    let scrollFrame = 0;
    terminal.attachCustomWheelEventHandler((event) => {
      if (terminal.buffer.active.type !== "alternate") return true;
      event.preventDefault();
      event.stopPropagation();
      const scale = wheelScrollScale(event, terminal.rows);
      pendingScrollLines += event.deltaY * scale;
      if (scrollFrame === 0) {
        scrollFrame = window.requestAnimationFrame(() => {
          scrollFrame = 0;
          const lines = Math.max(
            -REMOTE_SCROLL_CHUNK_LINES,
            Math.min(REMOTE_SCROLL_CHUNK_LINES, Math.trunc(pendingScrollLines)),
          );
          pendingScrollLines -= lines;
          if (lines === 0) return;
          void scrollSession(session.id, lines).catch((error) => {
            console.error(`UltraTerm scroll failed for ${terminalName(session.slot)}`, error);
          });
        });
      }
      return false;
    });
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.ctrlKey && !event.metaKey && !event.altKey && event.code === "KeyC") {
        event.preventDefault();
        event.stopImmediatePropagation();
        sendInput("\u0003");
      }
    };
    let disposed = false;
    host.addEventListener("keydown", handleKeyDown, true);

    let resizeFrame = 0;
    let resizeTimer = 0;
    let fittedWidth = -1;
    let fittedHeight = -1;
    const fit = (force = false) => {
      const width = host.clientWidth;
      const height = host.clientHeight;
      if (!force && width === fittedWidth && height === fittedHeight) return;
      window.cancelAnimationFrame(resizeFrame);
      resizeFrame = window.requestAnimationFrame(() => {
        const nextWidth = host.clientWidth;
        const nextHeight = host.clientHeight;
        if (!terminal.element || nextWidth === 0 || nextHeight === 0) return;
        if (!force && nextWidth === fittedWidth && nextHeight === fittedHeight) return;
        fittedWidth = nextWidth;
        fittedHeight = nextHeight;
        fitAddon.fit();
      });
    };
    const resizeObserver = new ResizeObserver(() => {
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => fit(), 80);
    });
    resizeObserver.observe(host);

    const inputDisposable = terminal.onData(sendInput);
    const resizeDisposable = terminal.onResize(({ cols, rows }) => {
      onTerminalResize(session.id);
      void resizeSession(session.id, cols, rows).catch((error) => {
        console.error(`UltraTerm resize failed for ${terminalName(session.slot)}`, error);
      });
    });

    const scrollRemote = async (lines: number) => {
      let remaining = Math.trunc(lines);
      while (remaining !== 0) {
        const chunk = Math.max(
          -REMOTE_SCROLL_CHUNK_LINES,
          Math.min(REMOTE_SCROLL_CHUNK_LINES, remaining),
        );
        await scrollSession(session.id, chunk);
        remaining -= chunk;
      }
    };
    const scrollLines = (lines: number) => {
      if (terminal.buffer.active.type === "alternate") {
        void scrollRemote(lines).catch((error) => {
          console.error(`UltraTerm scroll failed for ${terminalName(session.slot)}`, error);
        });
        return;
      }
      terminal.scrollLines(lines);
    };
    const scrollPages = (pages: number) => {
      if (terminal.buffer.active.type === "alternate") {
        void scrollRemote(pages * terminal.rows).catch((error) => {
          console.error(`UltraTerm page scroll failed for ${terminalName(session.slot)}`, error);
        });
        return;
      }
      terminal.scrollPages(pages);
    };
    const openTerminal = () => {
      if (disposed) return;
      const currentPreferences = preferencesRef.current;
      terminal.options.fontSize = currentPreferences.fontSize;
      terminal.options.cursorStyle = currentPreferences.cursorStyle;
      terminal.options.cursorBlink = currentPreferences.cursorBlink;
      terminal.options.theme = terminalThemeFor(themeRef.current);
      terminal.open(host);
      // Context loss (external-display sleep, GPU reset) silently degrades
      // xterm to the slow DOM renderer. Recover once after a short settle
      // instead of staying degraded for the life of the pane.
      const loadWebgl = (isRecovery: boolean) => {
        try {
          const webglAddon = new WebglAddon();
          webglAddon.onContextLoss(() => {
            webglAddon.dispose();
            if (!isRecovery && !disposed) {
              window.setTimeout(() => {
                if (!disposed) loadWebgl(true);
              }, 500);
            }
          });
          terminal.loadAddon(webglAddon);
        } catch (error) {
          console.warn("UltraTerm WebGL renderer unavailable; using the DOM renderer.", error);
        }
      };
      loadWebgl(false);
      terminalRef.current = terminal;
      onControllerReady(session.id, {
        scrollLines,
        scrollPages,
        scrollToBottom: () => terminal.scrollToBottom(),
        write: (data) => terminal.write(data),
        focus: () => terminal.focus(),
        fit: () => fit(true),
        hasPendingInput: () => pendingInputLength > 0,
        isAlternateBuffer: () => terminal.buffer.active.type === "alternate",
        trackInput,
      });
      fit(true);
      requestOmpAppearanceRefresh(themeRef.current);
    };
    void loadBundledTerminalFonts().then(openTerminal).catch((error) => {
      console.error(`UltraTerm bundled fonts failed to load for ${terminalName(session.slot)}`, error);
      openTerminal();
    });

    return () => {
      disposed = true;
      window.cancelAnimationFrame(scrollFrame);
      window.cancelAnimationFrame(resizeFrame);
      window.clearTimeout(resizeTimer);
      window.clearTimeout(appearanceReportTimerRef.current);
      onControllerReady(session.id, null);
      resizeObserver.disconnect();
      host.removeEventListener("keydown", handleKeyDown, true);
      inputDisposable.dispose();
      resizeDisposable.dispose();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
    };
  }, [onControllerReady, onTerminalResize, requestOmpAppearanceRefresh, session.id, session.slot]);

  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.scrollback = scrollback;
  }, [scrollback]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    terminal.options.theme = terminalThemeFor(theme);
    terminal.clearTextureAtlas();
    terminal.refresh(0, terminal.rows - 1);
    if (session.status !== "live") {
      appearanceRefreshKeyRef.current = null;
      return;
    }
    requestOmpAppearanceRefresh(theme);
  }, [requestOmpAppearanceRefresh, session.status, theme]);

  useEffect(() => {
    const terminal = terminalRef.current;
    const fitAddon = fitAddonRef.current;
    if (!terminal || !fitAddon) return;

    terminal.options.fontSize = preferences.fontSize;
    terminal.options.cursorStyle = preferences.cursorStyle;
    terminal.options.cursorBlink = preferences.cursorBlink;
    if (terminal.element) fitAddon.fit();
  }, [preferences]);


  return (
    <section
      className={`terminal-pane${entering ? " is-entering" : ""}${exiting ? " is-exiting" : ""}${active ? " is-active" : ""}${maximized ? " is-maximized" : ""}${session.activity === "working" ? " is-working" : " is-idle"}`}
      data-session-id={session.id}
      aria-label={`${terminalName(session.slot)} terminal, ${session.activity}`}
      onAnimationEnd={(event) => {
        if (
          event.target === event.currentTarget
          && event.animationName === "terminal-pane-enter"
        ) {
          setEntering(false);
        }
      }}
      onMouseDown={(event) => {
        if (exiting) return;
        onActivate(session.id);
        if (event.button === 0 && event.target === event.currentTarget) {
          void getCurrentWindow().startDragging();
        }
      }}
    >
      <div className="terminal-pane__glass" aria-hidden="true" />
      <div className="terminal-pane__viewport" ref={hostRef} />
      <div className="terminal-pane__focus-indicator" aria-hidden="true" />
      {session.status === "exited" && (
        <div className="terminal-pane__overlay">
          <p>Connection ended. Your persistent OMP work is preserved.</p>
          <button type="button" className="text-button" onClick={() => onRestart(session.id)}>
            Reconnect
          </button>
        </div>
      )}
    </section>
  );
}
