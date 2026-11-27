import type { TranslationKey } from "../i18n/locales/ja.ts";
import type { TranslationParams } from "../i18n/types.ts";
import type { IpcError } from "../ipc/types.ts";

export type AppErrorCode =
  | "UNKNOWN"
  | "IPC_UNAVAILABLE"
  | "IPC_DISCONNECTED"
  | "IPC_NOT_FOUND"
  | "IPC_INVALID_ARGS"
  | "IPC_UNSUPPORTED"
  | "IPC_UNTRUSTED_CONFIG"
  | "IPC_TIMEOUT"
  | "IPC_IO"
  | "IPC_ERROR"
  | "VALIDATION_FAILED"
  | "ACTION_NOT_FOUND"
  | "ACTION_INVALID_ARGS"
  | "ACTION_UNAVAILABLE"
  | "ACTION_EXECUTION_FAILED"
  | "TERMINAL_CREATE_FAILED"
  | "TERMINAL_ATTACH_FAILED"
  | "TERMINAL_RENDER_FAILED"
  | "TERMINAL_QUEUE_FULL";

type ErrorSeverity = "info" | "warn" | "error" | "fatal";

interface AppErrorOptions {
  code: AppErrorCode;
  messageKey: TranslationKey;
  messageParams?: TranslationParams;
  severity?: ErrorSeverity;
  devMessage: string;
  ipcCode?: string;
  hint?: string | null;
  diagnostics?: Readonly<Record<string, unknown>>;
  cause?: unknown;
}

interface ErrorDiagnostics {
  readonly code: AppErrorCode;
  readonly severity: ErrorSeverity;
  readonly devMessage: string;
  readonly ipcCode?: string;
  readonly hint?: string | null;
  readonly timestamp: number;
  readonly diagnostics?: Readonly<Record<string, unknown>>;
  readonly causeMessage?: string;
}

/**
 * Structured application error separating:
 * - user-facing localized message (`messageKey` + `messageParams`)
 * - developer-facing diagnostics (`devMessage`, `ipcCode`, `hint`, `diagnostics`, `cause`)
 */
export class AppError extends Error {
  readonly code: AppErrorCode;
  readonly severity: ErrorSeverity;
  readonly messageKey: TranslationKey;
  readonly messageParams?: TranslationParams;
  readonly ipcCode?: string;
  readonly hint?: string | null;
  readonly diagnostics?: Readonly<Record<string, unknown>>;
  readonly timestamp: number;

  constructor(options: AppErrorOptions) {
    super(options.devMessage, options.cause !== undefined ? { cause: options.cause } : undefined);
    this.name = "AppError";
    this.code = options.code;
    this.severity = options.severity ?? "error";
    this.messageKey = options.messageKey;
    this.messageParams = options.messageParams;
    this.ipcCode = options.ipcCode;
    this.hint = options.hint;
    this.diagnostics = options.diagnostics;
    this.timestamp = Date.now();
  }

  toDiagnostics(): ErrorDiagnostics {
    const causeMessage =
      this.cause instanceof Error
        ? this.cause.message
        : isIpcErrorLike(this.cause)
          ? this.cause.message
          : this.cause !== undefined
            ? String(this.cause)
            : undefined;

    return {
      code: this.code,
      severity: this.severity,
      devMessage: this.message,
      ...(this.ipcCode !== undefined ? { ipcCode: this.ipcCode } : {}),
      ...(this.hint !== undefined ? { hint: this.hint } : {}),
      timestamp: this.timestamp,
      ...(this.diagnostics !== undefined ? { diagnostics: this.diagnostics } : {}),
      ...(causeMessage !== undefined ? { causeMessage } : {}),
    };
  }

  formatDeveloperSummary(): string {
    const parts = [`[${this.code}]`];
    if (this.ipcCode) {
      parts.push(`(ipc:${this.ipcCode})`);
    }
    parts.push(this.message);
    if (this.hint) {
      parts.push(`hint=${this.hint}`);
    }
    return parts.join(" ");
  }
}

function isIpcErrorLike(value: unknown): value is IpcError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    typeof (value as { code: unknown }).code === "string" &&
    "message" in value &&
    typeof (value as { message: unknown }).message === "string"
  );
}

function mapIpcCode(ipcCode: string): { code: AppErrorCode; messageKey: TranslationKey } {
  switch (ipcCode) {
    case "unavailable":
    case "connect_failed":
    case "herdr_core_not_found":
    case "core_start_failed":
    case "core_version_mismatch":
    case "invalid_core_path":
    case "spawn_failed":
      return { code: "IPC_UNAVAILABLE", messageKey: "error.ipc.unavailable" };
    case "core_disconnected":
      return { code: "IPC_DISCONNECTED", messageKey: "error.ipc.disconnected" };
    case "not_found":
    case "invalid_attach":
      return { code: "IPC_NOT_FOUND", messageKey: "error.ipc.notFound" };
    case "invalid_args":
      return { code: "IPC_INVALID_ARGS", messageKey: "error.ipc.invalidArgs" };
    case "unsupported":
      return { code: "IPC_UNSUPPORTED", messageKey: "error.ipc.unsupported" };
    case "untrusted_config":
      return { code: "IPC_UNTRUSTED_CONFIG", messageKey: "error.ipc.untrustedConfig" };
    case "timeout":
    case "core_timeout":
    case "core_start_timeout":
    case "config_timeout":
      return { code: "IPC_TIMEOUT", messageKey: "error.ipc.timeout" };
    case "io_error":
      return { code: "IPC_IO", messageKey: "error.ipc.io" };
    default:
      return { code: "IPC_ERROR", messageKey: "error.ipc.generic" };
  }
}

export function fromIpcError(err: IpcError, context?: string): AppError {
  const mapped = mapIpcCode(err.code);
  const devMessage = context ? `${context}: ${err.message}` : err.message;
  return new AppError({
    code: mapped.code,
    messageKey: mapped.messageKey,
    messageParams: mapped.messageKey === "error.ipc.generic" ? { code: err.code } : undefined,
    devMessage,
    ipcCode: err.code,
    hint: err.hint ?? null,
    diagnostics: context ? { context } : undefined,
    cause: err,
  });
}

export function normalizeError(
  err: unknown,
  fallback: {
    code?: AppErrorCode;
    messageKey?: TranslationKey;
    messageParams?: TranslationParams;
    context?: string;
    diagnostics?: Readonly<Record<string, unknown>>;
  } = {},
): AppError {
  if (err instanceof AppError) {
    return err;
  }
  if (isIpcErrorLike(err)) {
    return fromIpcError(err, fallback.context);
  }
  const rawMessage = err instanceof Error ? err.message : String(err);
  const devMessage = fallback.context ? `${fallback.context}: ${rawMessage}` : rawMessage;
  return new AppError({
    code: fallback.code ?? "UNKNOWN",
    messageKey: fallback.messageKey ?? "error.unknown",
    messageParams: fallback.messageParams,
    devMessage,
    diagnostics: fallback.diagnostics,
    cause: err,
  });
}

type AsyncResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: AppError };

/**
 * Wraps an async operation so rejections are always captured and normalized into AppError.
 */
export async function runSafeAsync<T>(
  operation: () => Promise<T>,
  options?: {
    code?: AppErrorCode;
    messageKey?: TranslationKey;
    context?: string;
    onError?: (error: AppError) => void;
  },
): Promise<AsyncResult<T>> {
  try {
    const value = await operation();
    return { ok: true, value };
  } catch (err) {
    const error = normalizeError(err, {
      code: options?.code,
      messageKey: options?.messageKey,
      context: options?.context,
    });
    try {
      options?.onError?.(error);
    } catch {
      // Error observers must not replace the normalized operation result.
    }
    return { ok: false, error };
  }
}
