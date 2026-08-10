import { describe, expect, it } from "vitest";
import {
  DARK_TERMINAL_THEME,
  TERMINAL_APPEARANCE_RESET_OUTPUT,
  WHITE_TERMINAL_THEME,
  ompAppearanceChangeFor,
  ompAppearanceReportFor,
  terminalThemeFor,
} from "./terminalThemes";

const ANSI_FOREGROUNDS = [
  "foreground",
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

function relativeLuminance(hex: string): number {
  const channels = hex.match(/[\da-f]{2}/gi);
  if (!channels || channels.length !== 3) throw new Error(`Expected a six-digit hex color, received ${hex}`);
  const [red, green, blue] = channels.map((channel) => {
    const value = Number.parseInt(channel, 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrastRatio(first: string, second: string): number {
  const lighter = Math.max(relativeLuminance(first), relativeLuminance(second));
  const darker = Math.min(relativeLuminance(first), relativeLuminance(second));
  return (lighter + 0.05) / (darker + 0.05);
}

describe("terminalThemeFor", () => {
  it("uses the white palette only for the white application theme", () => {
    expect(terminalThemeFor("white")).toBe(WHITE_TERMINAL_THEME);
    expect(terminalThemeFor("oled")).toBe(DARK_TERMINAL_THEME);
    expect(terminalThemeFor("aurora")).toBe(DARK_TERMINAL_THEME);
    expect(terminalThemeFor("titanium")).toBe(DARK_TERMINAL_THEME);
    expect(terminalThemeFor("ember")).toBe(DARK_TERMINAL_THEME);
  });

  it("notifies OMP without activating a user keybinding", () => {
    expect(ompAppearanceChangeFor("white")).toBe("\x1b[?997;2n");
    expect(ompAppearanceChangeFor("oled")).toBe("\x1b[?997;1n");
    expect(ompAppearanceChangeFor("white")).not.toContain("\x0c");
  });

  it("reports the selected appearance directly through tmux", () => {
    expect(ompAppearanceReportFor("white")).toBe("\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    for (const theme of ["oled", "aurora", "titanium", "ember"] as const) {
      expect(ompAppearanceReportFor(theme)).toBe("\x1b]11;rgb:0000/0000/0000\x1b\\");
    }
  });

  it("clears explicit true-color cells without erasing saved scrollback", () => {
    expect(TERMINAL_APPEARANCE_RESET_OUTPUT).toBe("\x1b[2J\x1b[H");
    expect(TERMINAL_APPEARANCE_RESET_OUTPUT).not.toContain("\x1b[3J");
  });

  it("keeps every white-theme ANSI foreground legible on white", () => {
    for (const key of ANSI_FOREGROUNDS) {
      const color = WHITE_TERMINAL_THEME[key];
      expect(color, `${key} must be defined`).toBeTypeOf("string");
      expect(contrastRatio(color!, WHITE_TERMINAL_THEME.background!)).toBeGreaterThanOrEqual(4.5);
    }
  });
});
