import * as v from "valibot";
import type { Locale } from "../i18n/types.ts";
import {
  type CursorBlinkMode,
  type CursorStyle,
  DEFAULT_TERMINAL_APPEARANCE,
} from "../terminal/appearance.ts";
import {
  DEFAULT_FONT_SIZE,
  DEFAULT_LINE_HEIGHT,
  DEFAULT_PRIMARY_FONT_FAMILY,
  MAX_FONT_SIZE,
  MAX_LINE_HEIGHT,
  MIN_FONT_SIZE,
  MIN_LINE_HEIGHT,
} from "../terminal/fonts.ts";

export const TerminalBackendIdSchema = v.picklist([
  "wterm-ghostty-dom",
  "wterm-ghostty-webgl",
]);

export type TerminalBackendId = v.InferOutput<typeof TerminalBackendIdSchema>;

export const LocaleSchema = v.picklist(["ja", "en"]) satisfies v.GenericSchema<Locale>;

export const CursorStyleSchema = v.picklist([
  "auto",
  "block",
  "bar",
  "underline",
]) satisfies v.GenericSchema<CursorStyle>;

export const CursorBlinkModeSchema = v.picklist([
  "auto",
  "on",
  "off",
]) satisfies v.GenericSchema<CursorBlinkMode>;

export const UserSettingsSchema = v.object({
  locale: LocaleSchema,
  renderer: TerminalBackendIdSchema,
  scrollbackLines: v.pipe(v.number(), v.integer(), v.minValue(100), v.maxValue(50_000)),
  fontSize: v.pipe(v.number(), v.minValue(MIN_FONT_SIZE), v.maxValue(MAX_FONT_SIZE)),
  lineHeight: v.pipe(v.number(), v.minValue(MIN_LINE_HEIGHT), v.maxValue(MAX_LINE_HEIGHT)),
  fontFamily: v.pipe(v.string(), v.minLength(1), v.maxLength(512)),
  nerdFontFallback: v.boolean(),
  cursorStyle: CursorStyleSchema,
  cursorBlink: CursorBlinkModeSchema,
  macOptionAsMeta: v.boolean(),
  suppressAutoReplies: v.boolean(),
  pauseHiddenRendering: v.boolean(),
});

export type UserSettings = v.InferOutput<typeof UserSettingsSchema>;

export const DEFAULT_FONT_FAMILY = DEFAULT_PRIMARY_FONT_FAMILY;

export const DEFAULT_USER_SETTINGS: UserSettings = {
  locale: "ja",
  renderer: "wterm-ghostty-dom",
  scrollbackLines: 2000,
  fontSize: DEFAULT_FONT_SIZE,
  lineHeight: DEFAULT_LINE_HEIGHT,
  fontFamily: DEFAULT_FONT_FAMILY,
  nerdFontFallback: DEFAULT_TERMINAL_APPEARANCE.nerdFontFallback,
  cursorStyle: DEFAULT_TERMINAL_APPEARANCE.cursorStyle,
  cursorBlink: DEFAULT_TERMINAL_APPEARANCE.cursorBlink,
  macOptionAsMeta: true,
  suppressAutoReplies: true,
  pauseHiddenRendering: true,
};
