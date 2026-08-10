import type { ITheme } from "@xterm/xterm";
import type { ThemeId } from "../types";

/**
 * Notify OMP of an outer-terminal appearance change using DEC Mode 2031's
 * non-keyboard response. OMP consumes this sequence, re-queries OSC 11, and
 * never routes it through user keybindings such as Ctrl+L (live voice mode).
 */
export function ompAppearanceChangeFor(theme: ThemeId): string {
  return `\x1b[?997;${theme === "white" ? "2" : "1"}n`;
}

/**
 * Report the selected terminal background directly after OMP re-queries OSC 11,
 * avoiding reliance on tmux to route the outer terminal's reply to its pane.
 */
export function ompAppearanceReportFor(theme: ThemeId): string {
  const channel = theme === "white" ? "ffff" : "0000";
  return `\x1b]11;rgb:${channel}/${channel}/${channel}\x1b\\`;
}

/**
 * Existing true-color cells do not inherit a new xterm palette. Clear only the
 * rendered viewport before asking OMP to replay it with its new theme; saved
 * scrollback must survive appearance changes.
 */
export const TERMINAL_APPEARANCE_RESET_OUTPUT = "\x1b[2J\x1b[H";

export const DARK_TERMINAL_THEME: Readonly<ITheme> = {
  background: "#000000",
  foreground: "#ececf0",
  cursor: "#ffffff",
  cursorAccent: "#000000",
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
};

export const WHITE_TERMINAL_THEME: Readonly<ITheme> = {
  background: "#ffffff",
  foreground: "#18181d",
  cursor: "#111116",
  cursorAccent: "#ffffff",
  selectionBackground: "#315a8538",
  scrollbarSliderBackground: "rgba(50, 50, 58, 0.28)",
  scrollbarSliderHoverBackground: "rgba(42, 42, 50, 0.42)",
  scrollbarSliderActiveBackground: "rgba(32, 32, 40, 0.56)",
  black: "#18181d",
  red: "#a51d28",
  green: "#086735",
  yellow: "#705400",
  blue: "#0057a8",
  magenta: "#70409a",
  cyan: "#006879",
  white: "#4b4b52",
  brightBlack: "#5b5b63",
  brightRed: "#941822",
  brightGreen: "#075d30",
  brightYellow: "#624a00",
  brightBlue: "#004d96",
  brightMagenta: "#623687",
  brightCyan: "#005b6b",
  brightWhite: "#111116",
};

export function terminalThemeFor(theme: ThemeId): Readonly<ITheme> {
  return theme === "white" ? WHITE_TERMINAL_THEME : DARK_TERMINAL_THEME;
}
