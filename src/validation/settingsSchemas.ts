import * as v from "valibot";
import type { Locale } from "../i18n/types.ts";

export const TerminalBackendIdSchema = v.picklist([
  "wterm-ghostty-dom",
  "wterm-ghostty-webgl",
]);

export type TerminalBackendId = v.InferOutput<typeof TerminalBackendIdSchema>;

export const LocaleSchema = v.picklist(["ja", "en"]) satisfies v.GenericSchema<Locale>;

export const UserSettingsSchema = v.object({
  locale: LocaleSchema,
  renderer: TerminalBackendIdSchema,
  scrollbackLines: v.pipe(v.number(), v.integer(), v.minValue(100), v.maxValue(50_000)),
  fontSize: v.pipe(v.number(), v.minValue(8), v.maxValue(48)),
  fontFamily: v.pipe(v.string(), v.minLength(1), v.maxLength(512)),
  suppressAutoReplies: v.boolean(),
  pauseHiddenRendering: v.boolean(),
});

export type UserSettings = v.InferOutput<typeof UserSettingsSchema>;

export const DEFAULT_FONT_FAMILY =
  '"JetBrains Mono", "SF Mono", "Menlo", "Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans Mono CJK JP", "BIZ UDGothic", monospace';

export const DEFAULT_USER_SETTINGS: UserSettings = {
  locale: "ja",
  renderer: "wterm-ghostty-dom",
  scrollbackLines: 2000,
  fontSize: 14,
  fontFamily: DEFAULT_FONT_FAMILY,
  suppressAutoReplies: true,
  pauseHiddenRendering: true,
};
