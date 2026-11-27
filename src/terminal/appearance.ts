import type { WTerm } from "@wterm/dom";
import {
  buildTerminalFontStack,
  clampFontSize,
  clampLineHeight,
  DEFAULT_FONT_SIZE,
  DEFAULT_LINE_HEIGHT,
  DEFAULT_PRIMARY_FONT_FAMILY,
} from "./fonts.ts";

type WtermThemeColors = NonNullable<Parameters<WTerm["setThemeColors"]>[0]>;

export type CursorStyle = "auto" | "block" | "bar" | "underline";
export type CursorBlinkMode = "auto" | "on" | "off";

type AnsiColorPalette = readonly [
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
];

export interface TerminalTheme {
  readonly foreground: string;
  readonly background: string;
  readonly cursor: string;
  readonly selectionBackground: string;
  readonly palette: AnsiColorPalette;
}

export interface TerminalAppearance {
  readonly fontFamily: string;
  readonly fontSize: number;
  readonly lineHeight: number;
  readonly nerdFontFallback: boolean;
  readonly cursorStyle: CursorStyle;
  readonly cursorBlink: CursorBlinkMode;
  readonly theme: TerminalTheme;
}

/**
 * Default dark terminal color palette for uiterm.
 * Normal (0-7) and bright (8-15) ANSI colors are kept distinct so fish,
 * Neovim, git diff, and Codex status output remain readable on `#09090b`.
 */
export const DEFAULT_TERMINAL_THEME: TerminalTheme = {
  foreground: "#f4f4f5",
  background: "#09090b",
  cursor: "#e4e4e7",
  selectionBackground: "rgba(96, 165, 250, 0.30)",
  palette: [
    // Normal (0-7): black, red, green, yellow, blue, magenta, cyan, white
    "#18181b",
    "#ef4444",
    "#22c55e",
    "#eab308",
    "#3b82f6",
    "#c084fc",
    "#06b6d4",
    "#e4e4e7",
    // Bright (8-15): bright black..bright white
    "#52525b",
    "#f87171",
    "#4ade80",
    "#facc15",
    "#60a5fa",
    "#d8b4fe",
    "#22d3ee",
    "#ffffff",
  ],
};

export const DEFAULT_TERMINAL_APPEARANCE: TerminalAppearance = {
  fontFamily: DEFAULT_PRIMARY_FONT_FAMILY,
  fontSize: DEFAULT_FONT_SIZE,
  lineHeight: DEFAULT_LINE_HEIGHT,
  nerdFontFallback: true,
  cursorStyle: "auto",
  cursorBlink: "auto",
  theme: DEFAULT_TERMINAL_THEME,
};

/**
 * Parses a `#RRGGBB` or `#RGB` hex string into a 24-bit `0xRRGGBB` integer.
 */
export function hexToRgbNumber(hex: string): number | null {
  const trimmed = hex.trim();
  const full = /^#([0-9a-fA-F]{6})$/.exec(trimmed);
  if (full) {
    return Number.parseInt(full[1]!, 16);
  }
  const short = /^#([0-9a-fA-F]{3})$/.exec(trimmed);
  if (short) {
    const [r, g, b] = short[1]!;
    return Number.parseInt(`${r}${r}${g}${g}${b}${b}`, 16);
  }
  return null;
}

/**
 * Converts a `TerminalTheme` to `@wterm/core`'s 24-bit RGB `TerminalThemeColors`
 * structure so OSC 10/11/12/4 color queries and WebGL rendering match CSS variables.
 */
export function toWtermThemeColors(theme: TerminalTheme): WtermThemeColors | null {
  const foreground = hexToRgbNumber(theme.foreground);
  const background = hexToRgbNumber(theme.background);
  const cursor = hexToRgbNumber(theme.cursor);
  if (foreground === null || background === null || cursor === null) {
    return null;
  }
  const palette: number[] = [];
  for (const color of theme.palette) {
    const parsed = hexToRgbNumber(color);
    if (parsed === null) return null;
    palette.push(parsed);
  }
  return { foreground, background, cursor, palette };
}

/**
 * Converts `CursorBlinkMode` to `@wterm/dom`'s `cursorBlink` option:
 * - `"auto"` -> `undefined` (follow terminal application DECSCUSR / DECSET 12)
 * - `"on"` -> `true`
 * - `"off"` -> `false`
 */
export function toWtermCursorBlink(mode: CursorBlinkMode): boolean | undefined {
  if (mode === "on") return true;
  if (mode === "off") return false;
  return undefined;
}

/**
 * Applies typography, cursor preferences, and ANSI palette CSS custom properties
 * directly onto a `.wterm` host element.
 */
export function applyAppearanceToHost(host: HTMLElement, appearance: TerminalAppearance): void {
  const fontStack = buildTerminalFontStack(appearance.fontFamily, {
    nerdFontFallback: appearance.nerdFontFallback,
  });
  const fontSize = clampFontSize(appearance.fontSize);
  const lineHeight = clampLineHeight(appearance.lineHeight);
  const rowHeightPx = Math.max(1, Math.ceil(fontSize * lineHeight));

  host.style.setProperty("--term-font-family", fontStack);
  host.style.setProperty("--term-font-size", `${fontSize}px`);
  host.style.setProperty("--term-line-height", String(lineHeight));
  host.style.setProperty("--term-row-height", `${rowHeightPx}px`);
  host.style.fontFamily = fontStack;
  host.style.fontSize = `${fontSize}px`;
  host.style.lineHeight = String(lineHeight);

  const { theme } = appearance;
  host.style.setProperty("--term-fg", theme.foreground);
  host.style.setProperty("--term-bg", theme.background);
  host.style.setProperty("--term-cursor", theme.cursor);
  host.style.setProperty("--term-selection-bg", theme.selectionBackground);
  for (let i = 0; i < theme.palette.length; i++) {
    host.style.setProperty(`--term-color-${i}`, theme.palette[i]!);
  }

  host.classList.toggle("cursor-blink", appearance.cursorBlink === "on");
  host.classList.toggle("cursor-steady", appearance.cursorBlink === "off");
}
