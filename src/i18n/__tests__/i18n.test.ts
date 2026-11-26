import { describe, expect, it } from "vite-plus/test";
import { AppError } from "../../errors/appError.ts";
import {
  formatBytes,
  formatDateTime,
  formatNumber,
  formatRelativeTime,
  formatTime,
  resolveSystemLocale,
  t,
  translateError,
} from "../index.ts";
import { en } from "../locales/en.ts";
import { ja } from "../locales/ja.ts";

describe("i18n (TypeScript dictionary + Intl API)", () => {
  it("keeps ja and en dictionaries in exact key parity", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(ja).sort());
  });

  it("interpolates parameters and translates AppError messages", () => {
    expect(t("ja", "error.action.notFound", { actionId: "terminal.split" })).toBe(
      "アクション「terminal.split」は登録されていません。",
    );
    expect(t("en", "error.action.notFound", { actionId: "terminal.split" })).toBe(
      'Action "terminal.split" is not registered.',
    );

    const err = new AppError({
      code: "IPC_ERROR",
      messageKey: "error.ipc.generic",
      messageParams: { code: "custom_err" },
      devMessage: "custom",
    });
    expect(translateError("ja", err)).toBe("バックエンド通信エラーが発生しました（custom_err）。");
    expect(translateError("en", err)).toBe("A backend communication error occurred (custom_err).");
  });

  it("formats numbers, bytes, dates, times, and relative times with Intl API", () => {
    expect(formatNumber("ja", 12345)).toBe("12,345");
    expect(formatNumber("en", 12345)).toBe("12,345");
    expect(formatBytes("ja", 512)).toBe("512 B");
    expect(formatBytes("en", 2048)).toBe("2.0 KB");

    const now = Date.UTC(2026, 10, 26, 12, 0, 0);
    expect(formatDateTime("ja", now)).toBeTruthy();
    expect(formatTime("en", now)).toMatch(/\d{2}:\d{2}:\d{2}/);
    expect(formatRelativeTime("en", now - 60_000, now)).toContain("minute");
    expect(formatRelativeTime("en", now + 59_600, now)).toContain("minute");
    expect(formatRelativeTime("ja", now - 60_000, now)).toContain("分前");
  });

  it("resolves system locale cleanly", () => {
    expect(resolveSystemLocale("ja-JP")).toBe("ja");
    expect(resolveSystemLocale("en-US")).toBe("en");
    expect(resolveSystemLocale("fr-FR")).toBe("ja");
  });
});
