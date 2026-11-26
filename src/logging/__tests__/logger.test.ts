import { describe, expect, it } from "vite-plus/test";
import { Logger } from "../logger.ts";

describe("Logger (levels, bounded retention, privacy redaction)", () => {
  it("filters entries by minimum log level", () => {
    const logger = new Logger({ minLevel: "warn", maxEntries: 10 });
    expect(logger.debug("app", "debug msg")).toBeNull();
    expect(logger.info("app", "info msg")).toBeNull();
    expect(logger.warn("app", "warn msg")).not.toBeNull();
    expect(logger.error("app", "error msg")).not.toBeNull();
    expect(logger.getEntries().map((e) => e.level)).toEqual(["warn", "error"]);
  });

  it("enforces bounded retention limit", () => {
    const logger = new Logger({ minLevel: "debug", maxEntries: 3 });
    for (let i = 1; i <= 5; i++) {
      logger.info("test", `msg-${i}`);
    }
    expect(logger.getEntries().map((e) => e.message)).toEqual(["msg-3", "msg-4", "msg-5"]);

    logger.setMaxEntries(2);
    expect(logger.getEntries().map((e) => e.message)).toEqual(["msg-4", "msg-5"]);

    // Non-finite limits must fall back to a bounded default instead of disabling eviction
    const nonFinite = new Logger({ maxEntries: Number.POSITIVE_INFINITY });
    expect(Number.isFinite(nonFinite.getMaxEntries())).toBe(true);
    nonFinite.setMaxEntries(Number.NaN);
    expect(Number.isFinite(nonFinite.getMaxEntries())).toBe(true);
  });

  it("redacts credentials, terminal input, IME text, env maps, and binary buffers", () => {
    const logger = new Logger({ minLevel: "debug", maxEntries: 10 });
    const entry = logger.info("security", "test payload", {
      sessionId: "t1",
      password: "super-secret-password",
      apiKey: "sk-12345",
      privateKey: "-----BEGIN PRIVATE KEY-----",
      sshKey: "ssh-ed25519 AAAA",
      authorization: "Bearer xyz",
      input: "echo secret",
      userInput: "ls -la",
      imeData: "機密の日本語テキスト",
      committedText: "確定テキスト",
      compositionText: "変換中",
      env: { AWS_SECRET_ACCESS_KEY: "abc" },
      payload: new TextEncoder().encode("raw bytes"),
      typedView: new Int16Array([1, 2, 3]),
      nested: {
        token: "tok_999",
        safeCount: 42,
      },
    });

    expect(entry?.data).toEqual({
      sessionId: "t1",
      password: "[REDACTED]",
      apiKey: "[REDACTED]",
      privateKey: "[REDACTED]",
      sshKey: "[REDACTED]",
      authorization: "[REDACTED]",
      input: "[REDACTED]",
      userInput: "[REDACTED]",
      imeData: "[REDACTED]",
      committedText: "[REDACTED]",
      compositionText: "[REDACTED]",
      env: "[REDACTED]",
      payload: "[binary 9B redacted]",
      typedView: "[binary 6B redacted]",
      nested: {
        token: "[REDACTED]",
        safeCount: 42,
      },
    });
  });
});
