import * as v from "valibot";
import { describe, expect, it } from "vite-plus/test";
import { AppError } from "../../errors/appError.ts";
import {
  AttachIdSchema,
  IpcErrorSchema,
  parseExternalInput,
  parseTerminationEventData,
  ResizePayloadSchema,
  TerminalAttachParamsSchema,
  TerminalCreateParamsSchema,
} from "../ipcSchemas.ts";
import {
  DEFAULT_USER_SETTINGS,
  TerminalBackendIdSchema,
  UserSettingsSchema,
} from "../settingsSchemas.ts";

describe("Valibot validation schemas", () => {
  it("validates IPC attach/create/resize payloads and rejects invalid bounds", () => {
    expect(v.parse(AttachIdSchema, 1)).toBe(1);
    expect(() => v.parse(AttachIdSchema, 0)).toThrow();
    expect(() => v.parse(AttachIdSchema, -1)).toThrow();

    expect(
      v.parse(TerminalAttachParamsSchema, {
        sessionId: "t1",
        cols: 120,
        rows: 40,
        resizeNotifications: true,
      }),
    ).toMatchObject({ sessionId: "t1", cols: 120, rows: 40 });

    expect(() =>
      v.parse(TerminalCreateParamsSchema, {
        cols: 0,
        rows: 24,
      }),
    ).toThrow();

    expect(
      v.parse(IpcErrorSchema, {
        code: "core_disconnected",
        message: "socket closed",
        hint: "retry",
      }),
    ).toEqual({
      code: "core_disconnected",
      message: "socket closed",
      hint: "retry",
    });
  });

  it("wraps validation failures into structured AppError via parseExternalInput", () => {
    expect(() =>
      parseExternalInput(ResizePayloadSchema, { sessionId: "", cols: 0, rows: 24 }, "ipc.resize"),
    ).toThrow(AppError);

    try {
      parseExternalInput(ResizePayloadSchema, { sessionId: "", cols: 0, rows: 24 }, "ipc.resize");
    } catch (err) {
      expect(err).toBeInstanceOf(AppError);
      const appErr = err as AppError;
      expect(appErr.code).toBe("VALIDATION_FAILED");
      expect(appErr.messageKey).toBe("error.validation.failed");
      expect(appErr.diagnostics).toHaveProperty("issues");
    }
  });

  it("validates UserSettings and restricts TerminalBackendIdSchema", () => {
    expect(DEFAULT_USER_SETTINGS.renderer).toBe("wterm-ghostty-webgl");
    expect(v.parse(UserSettingsSchema, DEFAULT_USER_SETTINGS)).toEqual(DEFAULT_USER_SETTINGS);
    expect(() =>
      v.parse(UserSettingsSchema, { ...DEFAULT_USER_SETTINGS, scrollbackLines: 10 }),
    ).toThrow();
    expect(() => v.parse(TerminalBackendIdSchema, "xterm")).toThrow();
  });

  it("normalizes numeric and string signals in termination events and omits raw values from validation diagnostics", () => {
    expect(
      parseTerminationEventData({
        reason: "signaled",
        exit_code: null,
        signal: 15,
        finished_ms: 1234,
      }),
    ).toEqual({
      reason: "signaled",
      exitCode: null,
      signal: "SIGTERM",
      finishedMs: 1234,
    });

    expect(
      parseTerminationEventData({
        reason: "signaled",
        exit_code: null,
        signal: "SIGTERM",
        finished_ms: 2345,
      }),
    ).toEqual({
      reason: "signaled",
      exitCode: null,
      signal: "SIGTERM",
      finishedMs: 2345,
    });

    expect(
      parseTerminationEventData({
        reason: "exited",
        exit_code: 0,
        signal: null,
        finished_ms: 5678,
      }),
    ).toEqual({
      reason: "exited",
      exitCode: 0,
      signal: null,
      finishedMs: 5678,
    });

    try {
      parseExternalInput(
        TerminalCreateParamsSchema,
        { env: "TOP_SECRET_VALUE" },
        "terminal.create",
      );
      expect.unreachable("should throw AppError");
    } catch (err) {
      expect(err).toBeInstanceOf(AppError);
      const appErr = err as AppError;
      expect(appErr.message).not.toContain("TOP_SECRET_VALUE");
      expect(JSON.stringify(appErr.diagnostics)).not.toContain("TOP_SECRET_VALUE");
    }
    try {
      parseExternalInput(
        TerminalCreateParamsSchema,
        { env: { SECRET_API_KEY_NAME: 123 } },
        "terminal.create",
      );
      expect.unreachable("should throw AppError");
    } catch (err) {
      expect(err).toBeInstanceOf(AppError);
      const appErr = err as AppError;
      expect(appErr.message).toContain("env.[key]");
      expect(appErr.message).not.toContain("SECRET_API_KEY_NAME");
      expect(JSON.stringify(appErr.diagnostics)).not.toContain("SECRET_API_KEY_NAME");
    }
  });
});
