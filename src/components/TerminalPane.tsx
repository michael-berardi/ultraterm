import { useEffect, useRef, useState, type ReactElement } from "react";
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
  DEFAULT_TERMINAL_PREFERENCES,
  type TerminalController,
  type TerminalPreferences,
  type WorkspaceSession,
} from "../types";

interface TerminalPaneProps {
  session: WorkspaceSession;
  scrollback: number;
  preferences: TerminalPreferences;
  active: boolean;
  maximized: boolean;
  exiting: boolean;
  reducedMotion: boolean;
  /** Mount without the entrance animation (used behind the boot splash). */
  suppressEntrance?: boolean;
  onActivate: (id: string) => void;
  onControllerReady: (id: string, controller: TerminalController | null) => void;
  onRestart: (id: string) => void;
}


const TERMINAL_THEME = {
  background: "#030304",
  foreground: "#ececf0",
  cursor: "#ffffff",
  cursorAccent: "#111114",
  selectionBackground: "#9f9aaf66",
  scrollbarSliderBackground: "rgba(116, 116, 124, 0.46)",
  scrollbarSliderHoverBackground: "rgba(142, 142, 150, 0.62)",
  scrollbarSliderActiveBackground: "rgba(162, 162, 170, 0.76)",
  black: "#08080a",
  red: "#ff8587",
  green: "#83e6ad",
  yellow: "#e8ca82",
  blue: "#8db7ff",
  magenta: "#d2a9ff",
  cyan: "#8ddde5",
  white: "#e3e3e8",
  brightBlack: "#6b6a72",
  brightRed: "#ffaaa8",
  brightGreen: "#a8f1c5",
  brightYellow: "#f3dfa8",
  brightBlue: "#b3ceff",
  brightMagenta: "#e2c6ff",
  brightCyan: "#b2edf2",
  brightWhite: "#ffffff",
} as const;
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
function quoteShellPath(path: string): string {
  return `'${path.split("'").join("'\\''")}'`;
}

export function TerminalPane({
  session,
  scrollback,
  preferences,
  active,
  maximized,
  exiting,
  reducedMotion,
  suppressEntrance = false,
  onActivate,
  onControllerReady,
  onRestart,
}: TerminalPaneProps): ReactElement {
  const hostRef = useRef<HTMLDivElement>(null);
  const [entering, setEntering] = useState(!reducedMotion && !suppressEntrance);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const preferencesRef = useRef(preferences);
  preferencesRef.current = preferences;
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
      theme: TERMINAL_THEME,
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
    let unlistenDragDrop: (() => void) | null = null;
    const appWindow = getCurrentWindow();
    void appWindow.onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const { paths, position: physicalPosition } = event.payload;
      void appWindow.scaleFactor().then((scaleFactor) => {
        const position = physicalPosition.toLogical(scaleFactor);
        const bounds = host.getBoundingClientRect();
        if (
          position.x < bounds.left
          || position.x > bounds.right
          || position.y < bounds.top
          || position.y > bounds.bottom
        ) return;
        sendInput(`${paths.map(quoteShellPath).join(" ")} `);
        terminal.focus();
      });
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenDragDrop = unlisten;
    }).catch((error) => {
      console.error(`UltraTerm file drop setup failed for ${terminalName(session.slot)}`, error);
    });
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
      terminal.open(host);
      try {
        const webglAddon = new WebglAddon();
        webglAddon.onContextLoss(() => webglAddon.dispose());
        terminal.loadAddon(webglAddon);
      } catch (error) {
        console.warn("UltraTerm WebGL renderer unavailable; using the DOM renderer.", error);
      }
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
    };
    void loadBundledTerminalFonts().then(openTerminal).catch((error) => {
      console.error(`UltraTerm bundled fonts failed to load for ${terminalName(session.slot)}`, error);
      openTerminal();
    });

    return () => {
      disposed = true;
      unlistenDragDrop?.();
      window.cancelAnimationFrame(scrollFrame);
      window.cancelAnimationFrame(resizeFrame);
      window.clearTimeout(resizeTimer);
      onControllerReady(session.id, null);
      resizeObserver.disconnect();
      host.removeEventListener("keydown", handleKeyDown, true);
      inputDisposable.dispose();
      resizeDisposable.dispose();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
    };
  }, [onControllerReady, session.id, session.slot]);

  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.scrollback = scrollback;
  }, [scrollback]);

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
