import type { Locale } from "./types.ts";

const LOCALE_TAGS: Record<Locale, string> = {
  ja: "ja-JP",
  en: "en-US",
};

function toBcp47Tag(locale: Locale): string {
  return LOCALE_TAGS[locale];
}

export function formatNumber(
  locale: Locale,
  value: number,
  options?: Intl.NumberFormatOptions,
): string {
  return new Intl.NumberFormat(toBcp47Tag(locale), options).format(value);
}

const BYTE_UNITS = ["B", "KB", "MB", "GB"] as const;

export function formatBytes(locale: Locale, bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return `0 ${BYTE_UNITS[0]}`;
  }
  let unitIndex = 0;
  let value = bytes;
  while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
    value /= 1024;
    unitIndex++;
  }
  const fractionDigits = unitIndex === 0 ? 0 : 1;
  return `${formatNumber(locale, value, {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  })} ${BYTE_UNITS[unitIndex]}`;
}

export function formatDateTime(
  locale: Locale,
  value: Date | number,
  options: Intl.DateTimeFormatOptions = {
    dateStyle: "medium",
    timeStyle: "short",
  },
): string {
  return new Intl.DateTimeFormat(toBcp47Tag(locale), options).format(value);
}

export function formatTime(
  locale: Locale,
  value: Date | number,
  options: Intl.DateTimeFormatOptions = {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  },
): string {
  return new Intl.DateTimeFormat(toBcp47Tag(locale), options).format(value);
}

const RELATIVE_TIME_DIVISIONS: readonly {
  amount: number;
  unit: Intl.RelativeTimeFormatUnit;
}[] = [
  { amount: 60, unit: "second" },
  { amount: 60, unit: "minute" },
  { amount: 24, unit: "hour" },
  { amount: 7, unit: "day" },
  { amount: 4.34524, unit: "week" },
  { amount: 12, unit: "month" },
  { amount: Number.POSITIVE_INFINITY, unit: "year" },
];

export function formatRelativeTime(
  locale: Locale,
  targetMs: number,
  nowMs: number = Date.now(),
  options: Intl.RelativeTimeFormatOptions = { numeric: "auto" },
): string {
  const formatter = new Intl.RelativeTimeFormat(toBcp47Tag(locale), options);
  let duration = (targetMs - nowMs) / 1000;

  for (const division of RELATIVE_TIME_DIVISIONS) {
    const rounded = Math.round(duration);
    if (Math.abs(rounded) < division.amount) {
      return formatter.format(rounded, division.unit);
    }
    duration /= division.amount;
  }

  return formatter.format(Math.round(duration), "year");
}
