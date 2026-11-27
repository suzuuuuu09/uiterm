import { AppError } from "../errors/appError.ts";

export type LogLevel = "debug" | "info" | "warn" | "error";

const LOG_LEVEL_PRIORITY: Readonly<Record<LogLevel, number>> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};

export interface LogEntry {
  readonly id: number;
  readonly timestamp: number;
  readonly level: LogLevel;
  readonly scope: string;
  readonly message: string;
  readonly data?: Readonly<Record<string, unknown>>;
}

export interface LoggerOptions {
  minLevel?: LogLevel;
  maxEntries?: number;
  sink?: (entry: LogEntry) => void;
}

export const DEFAULT_MAX_LOG_ENTRIES = 200;
const MAX_STRING_CHARS = 160;
const MAX_DEPTH = 4;

/**
 * Keys whose values MUST NEVER be persisted in normal application logs:
 * - Credentials / auth secrets
 * - Raw terminal input bytes or text
 * - Japanese IME composition / committed text
 */
const REDACTED_KEY_PATTERNS: readonly RegExp[] = [
  /pass(word|phrase)?/i,
  /secret/i,
  /token/i,
  /api[-_]?key/i,
  /private[-_]?key/i,
  /ssh[-_]?key/i,
  /auth(orization)?/i,
  /cookie/i,
  /credential/i,
  /^input$/i,
  /(user|terminal)[-_]?input/i,
  /input[-_]?text/i,
  /raw[-_]?bytes?/i,
  /ime[-_]?(text|data|composition)/i,
  /(composed|committed)[-_]?text/i,
  /composition([-_]?text)?/i,
  /^env$/i,
];

function isSensitiveKey(key: string): boolean {
  return REDACTED_KEY_PATTERNS.some((pattern) => pattern.test(key));
}

function truncateString(value: string): string {
  if (value.length <= MAX_STRING_CHARS) return value;
  return `${value.slice(0, MAX_STRING_CHARS - 3)}...`;
}

function sanitizeValue(value: unknown, depth: number): unknown {
  if (value === null || value === undefined) return value;
  if (typeof value === "string") return truncateString(value);
  if (typeof value === "number" || typeof value === "boolean") return value;
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    const byteLength = value.byteLength;
    return `[binary ${byteLength}B redacted]`;
  }
  if (value instanceof AppError) {
    return sanitizeRecord(value.toDiagnostics() as unknown as Record<string, unknown>, depth + 1);
  }
  if (value instanceof Error) {
    return {
      name: value.name,
      message: truncateString(value.message),
    };
  }
  if (depth >= MAX_DEPTH) {
    return "[truncated]";
  }
  if (Array.isArray(value)) {
    return value.slice(0, 16).map((item) => sanitizeValue(item, depth + 1));
  }
  if (typeof value === "object") {
    return sanitizeRecord(value as Record<string, unknown>, depth + 1);
  }
  return truncateString(String(value));
}

export function sanitizeLogData(
  data: Readonly<Record<string, unknown>>,
): Readonly<Record<string, unknown>> {
  return sanitizeRecord(data, 0);
}

function sanitizeRecord(
  record: Readonly<Record<string, unknown>>,
  depth: number,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(record)) {
    if (isSensitiveKey(key)) {
      out[key] = "[REDACTED]";
    } else {
      out[key] = sanitizeValue(value, depth);
    }
  }
  return out;
}

function normalizeMaxEntries(limit: number | undefined): number {
  if (typeof limit !== "number" || !Number.isFinite(limit)) {
    return DEFAULT_MAX_LOG_ENTRIES;
  }
  return Math.max(1, Math.floor(limit));
}

export class Logger {
  private minLevel: LogLevel;
  private maxEntries: number;
  private readonly sink?: (entry: LogEntry) => void;
  private entries: LogEntry[] = [];
  private listeners = new Set<(entries: readonly LogEntry[]) => void>();
  private nextId = 1;

  constructor(options: LoggerOptions = {}) {
    this.minLevel = options.minLevel ?? "info";
    this.maxEntries = normalizeMaxEntries(options.maxEntries);
    this.sink = options.sink;
  }

  getMinLevel(): LogLevel {
    return this.minLevel;
  }

  setMinLevel(level: LogLevel): void {
    this.minLevel = level;
  }

  getMaxEntries(): number {
    return this.maxEntries;
  }

  setMaxEntries(limit: number): void {
    this.maxEntries = normalizeMaxEntries(limit);
    if (this.entries.length > this.maxEntries) {
      this.entries = this.entries.slice(this.entries.length - this.maxEntries);
      this.notify();
    }
  }

  getEntries(): readonly LogEntry[] {
    return this.entries;
  }

  clear(): void {
    if (this.entries.length === 0) return;
    this.entries = [];
    this.notify();
  }

  subscribe(listener: (entries: readonly LogEntry[]) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  log(
    level: LogLevel,
    scope: string,
    message: string,
    data?: Readonly<Record<string, unknown>>,
  ): LogEntry | null {
    if (LOG_LEVEL_PRIORITY[level] < LOG_LEVEL_PRIORITY[this.minLevel]) {
      return null;
    }

    const entry: LogEntry = {
      id: this.nextId++,
      timestamp: Date.now(),
      level,
      scope,
      message: truncateString(message),
      ...(data !== undefined ? { data: sanitizeLogData(data) } : {}),
    };

    if (this.entries.length >= this.maxEntries) {
      this.entries = [...this.entries.slice(this.entries.length - this.maxEntries + 1), entry];
    } else {
      this.entries = [...this.entries, entry];
    }

    this.sink?.(entry);
    this.notify();
    return entry;
  }

  debug(scope: string, message: string, data?: Readonly<Record<string, unknown>>): LogEntry | null {
    return this.log("debug", scope, message, data);
  }

  info(scope: string, message: string, data?: Readonly<Record<string, unknown>>): LogEntry | null {
    return this.log("info", scope, message, data);
  }

  warn(scope: string, message: string, data?: Readonly<Record<string, unknown>>): LogEntry | null {
    return this.log("warn", scope, message, data);
  }

  error(scope: string, message: string, data?: Readonly<Record<string, unknown>>): LogEntry | null {
    return this.log("error", scope, message, data);
  }

  logError(
    scope: string,
    error: AppError,
    extra?: Readonly<Record<string, unknown>>,
  ): LogEntry | null {
    const level: LogLevel =
      error.severity === "info" ? "info" : error.severity === "warn" ? "warn" : "error";
    return this.log(level, scope, error.formatDeveloperSummary(), {
      ...error.toDiagnostics(),
      ...extra,
    });
  }

  private notify(): void {
    for (const listener of this.listeners) {
      listener(this.entries);
    }
  }
}

const devConsoleSink =
  typeof import.meta !== "undefined" && import.meta.env?.DEV && import.meta.env?.MODE !== "test"
    ? (entry: LogEntry) => {
        if (entry.level === "error") {
          console.error(`[${entry.scope}] ${entry.message}`, entry.data ?? "");
        } else if (entry.level === "warn") {
          console.warn(`[${entry.scope}] ${entry.message}`, entry.data ?? "");
        }
      }
    : undefined;

export const appLogger = new Logger({
  minLevel: "info",
  maxEntries: DEFAULT_MAX_LOG_ENTRIES,
  sink: devConsoleSink,
});
