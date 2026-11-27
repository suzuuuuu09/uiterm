/**
 * Internal terminal font configuration and fallback stack builder (Phase 2).
 *
 * Design principles:
 * - Uses system-installed fonts only; never bundles or forces a specific font.
 * - Preserves primary monospace font for ASCII/Latin while allowing Nerd Font
 *   Private Use Area (PUA) glyphs, Japanese CJK glyphs, and emoji to fall back
 *   cleanly to system-installed fonts.
 * - Full settings UI and TypeScript config integration are deferred to Phase 4.
 */

interface FontFallbackOptions {
  /** Append common system-installed Nerd Font / symbol fallbacks for PUA icons. */
  readonly nerdFontFallback?: boolean;
  /** Append Japanese monospace/gothic fallbacks for CJK full-width & half-width kana. */
  readonly cjkFallback?: boolean;
  /** Append system color emoji fallbacks. */
  readonly emojiFallback?: boolean;
}

export interface CellMetrics {
  readonly charWidth: number;
  readonly rowHeight: number;
}

export const MIN_FONT_SIZE = 8;
export const MAX_FONT_SIZE = 48;
export const DEFAULT_FONT_SIZE = 14;
export const FONT_SIZE_STEP = 1;

export const MIN_LINE_HEIGHT = 1.0;
export const MAX_LINE_HEIGHT = 2.0;
export const DEFAULT_LINE_HEIGHT = 1.25;

/**
 * Default primary monospace stack.
 * Relies on OS-provided monospace fonts without forcing any third-party font.
 */
export const DEFAULT_PRIMARY_FONT_FAMILY = 'ui-monospace, "SF Mono", "Menlo", "JetBrains Mono"';

/**
 * Common system-installed Nerd Font & symbol families used as PUA fallbacks
 * (U+E000-U+F8FF, U+F0000-U+FFFFD) when the primary font lacks icon glyphs.
 * Dedicated symbol fonts are listed first so ASCII/Latin stays on the primary font.
 */
export const NERD_FONT_FALLBACK_FAMILIES: readonly string[] = [
  '"Symbols Nerd Font Mono"',
  '"Symbols Nerd Font"',
  '"UDEV Gothic 35NF"',
  '"UDEV Gothic NF"',
  '"JetBrainsMono Nerd Font Mono"',
  '"Hack Nerd Font Mono"',
  '"MesloLGS NF"',
  '"FiraCode Nerd Font Mono"',
  '"CaskaydiaCove Nerd Font Mono"',
];

/**
 * Japanese monospace / gothic fallback families for macOS and Linux.
 */
export const JAPANESE_FONT_FALLBACK_FAMILIES: readonly string[] = [
  '"Hiragino Sans"',
  '"Hiragino Kaku Gothic ProN"',
  '"Noto Sans Mono CJK JP"',
  '"BIZ UDGothic"',
];

/**
 * System color emoji fallback families.
 */
const EMOJI_FONT_FALLBACK_FAMILIES: readonly string[] = [
  '"Apple Color Emoji"',
  '"Noto Color Emoji"',
];

function normalizeFamilyToken(token: string): string {
  return token
    .trim()
    .replace(/^["']|["']$/g, "")
    .toLowerCase();
}

function splitFontList(fontFamily: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  for (const ch of fontFamily) {
    if (quote) {
      current += ch;
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      current += ch;
    } else if (ch === ",") {
      const trimmed = current.trim();
      if (trimmed) parts.push(trimmed);
      current = "";
    } else {
      current += ch;
    }
  }
  const tail = (quote ? current.replaceAll(quote, "") : current).trim();
  if (tail) parts.push(tail);
  return parts;
}

/**
 * Builds a deduplicated CSS `font-family` stack from a primary font setting and
 * optional Nerd Font / Japanese / Emoji fallback chains.
 */
export function buildTerminalFontStack(
  primaryFontFamily: string = DEFAULT_PRIMARY_FONT_FAMILY,
  options: FontFallbackOptions = {},
): string {
  const { nerdFontFallback = true, cjkFallback = true, emojiFallback = true } = options;

  const seen = new Set<string>();
  const result: string[] = [];

  const append = (families: readonly string[]) => {
    for (const raw of families) {
      const trimmed = raw.trim();
      if (!trimmed) continue;
      const key = normalizeFamilyToken(trimmed);
      if (key === "monospace") continue;
      if (seen.has(key)) continue;
      seen.add(key);
      result.push(trimmed);
    }
  };

  const primaryList = splitFontList(primaryFontFamily);
  append(primaryList.length > 0 ? primaryList : splitFontList(DEFAULT_PRIMARY_FONT_FAMILY));

  if (nerdFontFallback) {
    append(NERD_FONT_FALLBACK_FAMILIES);
  }
  if (cjkFallback) {
    append(JAPANESE_FONT_FALLBACK_FAMILIES);
  }
  if (emojiFallback) {
    append(EMOJI_FONT_FALLBACK_FAMILIES);
  }

  result.push("monospace");
  return result.join(", ");
}

export function clampFontSize(size: number): number {
  if (!Number.isFinite(size)) return DEFAULT_FONT_SIZE;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(size * 2) / 2));
}

export function clampLineHeight(lineHeight: number): number {
  if (!Number.isFinite(lineHeight)) return DEFAULT_LINE_HEIGHT;
  return Math.min(MAX_LINE_HEIGHT, Math.max(MIN_LINE_HEIGHT, Math.round(lineHeight * 100) / 100));
}
