export type Locale = "ja" | "en";

export const DEFAULT_LOCALE: Locale = "ja";

export type TranslationParams = Readonly<Record<string, string | number>>;
