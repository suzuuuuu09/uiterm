export type Locale = "ja" | "en";

export const SUPPORTED_LOCALES: readonly Locale[] = ["ja", "en"] as const;

export const DEFAULT_LOCALE: Locale = "ja";

export type TranslationParams = Readonly<Record<string, string | number>>;
