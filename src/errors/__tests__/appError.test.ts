import { describe, expect, it } from "vite-plus/test";
import { translateError } from "../../i18n/index.ts";
import { AppError, fromIpcError, normalizeError, runSafeAsync } from "../appError.ts";

describe("AppError & async error handling", () => {
  it("maps IpcError codes to structured AppError with localized user messages and dev diagnostics", () => {
    const err = fromIpcError(
      { code: "core_disconnected", message: "unix socket EOF", hint: "call coreReconnect" },
      "terminalWrite:t1",
    );

    expect(err).toBeInstanceOf(AppError);
    expect(err.code).toBe("IPC_DISCONNECTED");
    expect(err.ipcCode).toBe("core_disconnected");
    expect(err.hint).toBe("call coreReconnect");
    expect(translateError("ja", err)).toBe("バックエンド（herdr-core）との接続が切断されました。");
    expect(translateError("en", err)).toBe("Disconnected from the backend (herdr-core).");
    expect(err.formatDeveloperSummary()).toContain("[IPC_DISCONNECTED]");
    expect(err.formatDeveloperSummary()).toContain("unix socket EOF");
    expect(err.toDiagnostics().ipcCode).toBe("core_disconnected");
  });

  it("normalizes unknown values and preserves existing AppError instances", () => {
    const original = new AppError({
      code: "ACTION_NOT_FOUND",
      messageKey: "error.action.notFound",
      messageParams: { actionId: "foo.bar" },
      devMessage: "missing foo.bar",
    });
    expect(normalizeError(original)).toBe(original);
    expect(translateError("ja", original)).toBe("アクション「foo.bar」は登録されていません。");

    const fromString = normalizeError("unexpected string", { context: "test" });
    expect(fromString.code).toBe("UNKNOWN");
    expect(fromString.message).toBe("test: unexpected string");
  });

  it("captures async rejections via runSafeAsync", async () => {
    const captured: AppError[] = [];
    const ok = await runSafeAsync(async () => 42);
    expect(ok).toEqual({ ok: true, value: 42 });

    const failed = await runSafeAsync(
      async () => {
        throw { code: "timeout", message: "rpc timed out" };
      },
      {
        context: "coreStatus",
        onError: (e) => captured.push(e),
      },
    );

    expect(failed.ok).toBe(false);
    if (!failed.ok) {
      expect(failed.error.code).toBe("IPC_TIMEOUT");
    }
    expect(captured).toHaveLength(1);
  });

  it("maps Rust backend IPC error codes (core_timeout, herdr_core_not_found, untrusted_config)", () => {
    expect(fromIpcError({ code: "core_timeout", message: "rpc timeout" }).code).toBe("IPC_TIMEOUT");
    expect(fromIpcError({ code: "core_start_timeout", message: "start timeout" }).code).toBe(
      "IPC_TIMEOUT",
    );
    expect(fromIpcError({ code: "herdr_core_not_found", message: "missing bin" }).code).toBe(
      "IPC_UNAVAILABLE",
    );
    expect(fromIpcError({ code: "core_version_mismatch", message: "bad ver" }).code).toBe(
      "IPC_UNAVAILABLE",
    );

    const untrusted = fromIpcError({ code: "untrusted_config", message: "disabled" });
    expect(untrusted.code).toBe("IPC_UNTRUSTED_CONFIG");
    expect(translateError("ja", untrusted)).toBe(
      "信頼されていない設定ファイルの実行は拒否されました。",
    );
    expect(translateError("en", untrusted)).toBe(
      "Execution of an untrusted configuration file was denied.",
    );
  });
});
