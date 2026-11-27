import type { AppError } from "../errors/appError.ts";
import { en } from "./locales/en.ts";
import { ja, type TranslationKey } from "./locales/ja.ts";
import { DEFAULT_LOCALE, type Locale, type TranslationParams } from "./types.ts";

export {
  formatBytes,
  formatDateTime,
  formatNumber,
  formatRelativeTime,
  formatTime,
} from "./format.ts";
export type { TranslationKey } from "./locales/ja.ts";
export type { Locale, TranslationParams } from "./types.ts";

const dictionaries: Readonly<Record<Locale, Readonly<Record<TranslationKey, string>>>> = {
  ja,
  en,
};

const INTERPOLATION_RE = /\{([a-zA-Z0-9_]+)\}/g;

export function t(locale: Locale, key: TranslationKey, params?: TranslationParams): string {
  const template = dictionaries[locale][key];
  if (!params) {
    return template;
  }
  return template.replace(INTERPOLATION_RE, (match, name: string) => {
    const value = params[name];
    return value !== undefined ? String(value) : match;
  });
}

export function translateError(locale: Locale, error: AppError): string {
  return t(locale, error.messageKey, error.messageParams);
}

export function resolveSystemLocale(navigatorLanguage?: string): Locale {
  const lang =
    navigatorLanguage ??
    (typeof navigator !== "undefined" ? navigator.language : undefined) ??
    DEFAULT_LOCALE;
  return lang.toLowerCase().startsWith("en") ? "en" : "ja";
}
