import * as v from "valibot";
import { describe, expect, it, vi } from "vite-plus/test";
import { t } from "../../i18n/dict.ts";
import { MockIpcClient } from "../../ipc/mock.ts";
import type {
  AttachId,
  InputKind,
  OutputFrame,
  TerminalAttachParams,
  TerminalAttachResult,
} from "../../ipc/types.ts";
import { pocStore } from "../../store/pocStore.ts";
import { MockHerdrTransport } from "../../transport/mockHerdrTransport.ts";
import { TerminalBackendIdSchema, validateResizePayload } from "../../transport/schemas.ts";
import { TerminalSessionManager } from "../sessionManager.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "../types.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

describe("Phase 0 Frontend PoC core contracts", () => {
  it("validates resize payloads with Valibot", () => {
    expect(validateResizePayload({ sessionId: "term-1", cols: 120, rows: 40 })).toEqual({
      sessionId: "term-1",
      cols: 120,
      rows: 40,
    });
    expect(() => validateResizePayload({ sessionId: "term-1", cols: 0, rows: 24 })).toThrow();
  });

  it("resolves TypeScript dictionary i18n keys without i18next", () => {
    expect(t("ja", "appTitle")).toBe("uiterm Phase 0 PoC");
    expect(t("ja", "backend")).toBe("描画エンジン");
    expect(t("en", "backend")).toBe("Renderer");
  });

  it("pipes binary Uint8Array stream directly without mutating Zustand store with output", () => {
    const transport = new MockHerdrTransport("test-1");
    const chunks: Uint8Array[] = [];
    const unsub = transport.subscribeOutput((chunk) => {
      chunks.push(chunk.bytes);
    });

    transport.attach(80, 24);
    expect(chunks.length).toBe(1);
    expect(decoder.decode(chunks[0]!)).toContain("herdr-core mock");

    // Verify Japanese UTF-8 input echo
    transport.sendInput(encoder.encode("日本語テスト\r"));
    const combined = chunks.map((c) => decoder.decode(c)).join("");
    expect(combined).toContain("日本語テスト");

    // Verify resync
    transport.requestResync();
    expect(decoder.decode(chunks[chunks.length - 1]!)).toContain("herdr-core resync");

    // Verify Zustand store has no terminal output buffer field
    expect("terminalOutput" in pocStore.getState()).toBe(false);

    unsub();
    transport.dispose();
  });

  it("generates 10,000-line binary burst for benchmarking", () => {
    const transport = new MockHerdrTransport("bench-1");
    let receivedBytes = 0;
    transport.subscribeOutput((chunk) => {
      receivedBytes += chunk.bytes.byteLength;
    });

    const result = transport.emitBurst(10_000, 200);
    expect(result.totalBytes).toBeGreaterThan(800_000);
    expect(receivedBytes).toBe(result.totalBytes);
    transport.dispose();
  });
});

function createFakeElement(): HTMLDivElement {
  const children: unknown[] = [];
  const el = {
    dataset: {} as Record<string, string>,
    className: "",
    style: { display: "" },
    appendChild(child: unknown) {
      children.push(child);
      return child;
    },
    remove() {},
  };
  return el as unknown as HTMLDivElement;
}

function sessionAdapter(
  capture: (callbacks: TerminalAdapterCallbacks) => void,
  dispose = () => {},
  write?: (bytes: Uint8Array) => void,
) {
  let fakeCols = 80;
  let fakeRows = 24;
  return (_backendId: unknown, callbacks: TerminalAdapterCallbacks): TerminalAdapter => {
    capture(callbacks);
    return {
      backendId: "wterm-ghostty-dom",
      get cols() {
        return fakeCols;
      },
      get rows() {
        return fakeRows;
      },
      async mount() {},
      write(bytes: Uint8Array) {
        // Simulate wterm synchronously generating an automatic VT reply during write()
        if (write) write(bytes);
        else if (bytes.byteLength > 0) {
          callbacks.onAutoResponse(encoder.encode("\x1b[?62;22c"));
        }
      },
      resize(cols: number, rows: number) {
        fakeCols = cols;
        fakeRows = rows;
        // Simulate terminal renderer echoing onResize synchronously
        callbacks.onResize(cols, rows);
      },
      fit() {},
      focus() {},
      setHidden() {},
      async getSelection() {
        return "";
      },
      dispose,
    };
  };
}

describe("Phase 0 Integration guards (attachId=0 race, resize loop, IPC errors)", () => {
  it("never sends ACK or auto-response with attachId=0 before terminalAttach resolves", async () => {
    const prevDoc = globalThis.document;
    (globalThis as unknown as { document: { createElement: () => HTMLDivElement } }).document = {
      createElement: createFakeElement,
    };

    const ackCalls: Array<{ attachId: AttachId; bytes: number }> = [];
    const writeCalls: Array<{ attachId: AttachId; kind: InputKind; len: number }> = [];
    const resizeCalls: Array<{ attachId: AttachId; cols: number; rows: number }> = [];

    const mock = new MockIpcClient();
    let emitFrame!: (frame: OutputFrame) => void;
    let resolveAttach!: (res: TerminalAttachResult) => void;

    mock.terminalAck = async (attachId, bytes) => {
      ackCalls.push({ attachId, bytes });
    };
    mock.terminalWrite = async (attachId, data, kind = "user") => {
      const len = typeof data === "string" ? data.length : data.byteLength;
      writeCalls.push({ attachId, kind, len });
    };
    mock.terminalResize = async (attachId, cols, rows) => {
      resizeCalls.push({ attachId, cols, rows });
    };
    mock.terminalAttach = (_params: TerminalAttachParams, onFrame: (frame: OutputFrame) => void) => {
      emitFrame = onFrame;
      return new Promise<TerminalAttachResult>((resolve) => {
        resolveAttach = resolve;
      });
    };

    let capturedCallbacks!: TerminalAdapterCallbacks;
    const fakeAdapterFactory = sessionAdapter((callbacks) => {
      capturedCallbacks = callbacks;
    });

    try {
      pocStore.getState().setSuppressAutoReplies(false);
      const mgr = new TerminalSessionManager(mock, fakeAdapterFactory as never);
      const createPromise = mgr.createSession("race-1", "wterm-ghostty-dom", createFakeElement(), false);

      // Wait a microtask so terminalAttach has been invoked but NOT yet resolved
      await Promise.resolve();
      await Promise.resolve();

      // Emit a 20 KiB initial repaint BEFORE terminalAttach resolves!
      emitFrame({ kind: "data", bytes: new Uint8Array(20 * 1024).fill(65) });

      // Must NOT have sent anything with attachId = 0!
      expect(ackCalls).toEqual([]);
      expect(writeCalls).toEqual([]);

      // Now resolve terminalAttach with real attachId = 7
      resolveAttach({
        attachId: 7,
        terminal: {
          address: "t1",
          name: "race-1",
          argv: ["/bin/sh"],
          cwd: "/tmp",
          state: "live",
          cols: 80,
          rows: 24,
          title: "race-1",
          pid: 1,
          createdMs: Date.now(),
          finishedMs: null,
          exitCode: null,
          signal: null,
          reason: null,
          status: null,
          statusMs: null,
          turn: 0,
          labels: {},
          foreground: null,
        },
        automaticInput: true,
        resizeNotifications: true,
        initialCreditBytes: 256 * 1024,
      });

      await createPromise;

      // Queued ACK and automatic reply must now flush with real attachId = 7!
      expect(ackCalls).toEqual([{ attachId: 7, bytes: 20 * 1024 }]);
      expect(writeCalls).toEqual([{ attachId: 7, kind: "automatic", len: 9 }]);

      // Backend-originated RESIZE must NOT echo back to client.terminalResize!
      emitFrame({ kind: "resize", cols: 120, rows: 40 });
      expect(resizeCalls).toEqual([]);

      // User-originated resize to a different size MUST send terminalResize!
      capturedCallbacks.onResize(132, 43);
      expect(resizeCalls).toEqual([{ attachId: 7, cols: 132, rows: 43 }]);

      // Duplicate resize to same (132, 43) is deduplicated
      capturedCallbacks.onResize(132, 43);
      expect(resizeCalls).toEqual([{ attachId: 7, cols: 132, rows: 43 }]);

      mgr.disposeAll();
    } finally {
      pocStore.getState().setSuppressAutoReplies(true);
      (globalThis as unknown as { document: typeof prevDoc }).document = prevDoc;
    }
  });
  it("does not revive a closed attachment or replay its input after resync", async () => {
    vi.stubGlobal("document", { createElement: createFakeElement });
    try {
      const mock = new MockIpcClient();
      const attachMock = mock.terminalAttach.bind(mock);
      const callbacks: Array<(frame: OutputFrame) => void> = [];
      const writes = vi.spyOn(mock, "terminalWrite");
      mock.terminalAttach = async (params, onFrame) => {
        const index = callbacks.push(onFrame);
        const result = await attachMock(params, (frame) => {
          if (index > 1) onFrame(frame);
        });
        if (index === 1) {
          onFrame({ kind: "close", reason: "disconnected", exitCode: null, signal: null });
        }
        return result;
      };
      let adapterCallbacks!: TerminalAdapterCallbacks;
      const render = vi.fn();
      const mgr = new TerminalSessionManager(mock, sessionAdapter((callbacks) => {
        adapterCallbacks = callbacks;
      }, undefined, render));
      const session = await mgr.createSession("closed", "wterm-ghostty-dom", createFakeElement(), false);
      adapterCallbacks.onUserInput(encoder.encode("stale"));
      expect(session.attachId).toBe(0);
      expect(writes).not.toHaveBeenCalled();

      await Promise.all([mgr.resyncSession("closed"), mgr.resyncSession("closed")]);
      expect(callbacks).toHaveLength(2);
      expect(session.attachId).toBeGreaterThan(0);
      const rendered = render.mock.calls.length;
      callbacks[0]!({ kind: "data", bytes: encoder.encode("stale") });
      expect(render).toHaveBeenCalledTimes(rendered);
      adapterCallbacks.onUserInput(encoder.encode("新しい入力"));
      expect(writes).toHaveBeenCalledExactlyOnceWith(session.attachId, encoder.encode("新しい入力"), "user");
      mgr.disposeAll();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("rolls back failed attach and cancels a pending terminal create", async () => {
    vi.stubGlobal("document", { createElement: createFakeElement });
    try {
      const mock = new MockIpcClient();
      const dispose = vi.fn();
      const mgr = new TerminalSessionManager(mock, sessionAdapter(() => {}, dispose));
      mock.terminalAttach = vi.fn().mockRejectedValue(new Error("attach failed"));
      const host = createFakeElement();
      await expect(mgr.createSession("failed", "wterm-ghostty-dom", host, false)).rejects.toThrow("attach failed");
      expect(dispose).toHaveBeenCalledTimes(1);
      expect(await mock.terminalList()).toEqual([]);
      expect(mgr.get("failed")).toBeUndefined();

      const cancelled = mgr.createSession("pending", "wterm-ghostty-dom", host, false);
      mgr.disposeAll();
      await expect(cancelled).rejects.toMatchObject({ name: "AbortError" });
      expect(await mock.terminalList()).toEqual([]);
      expect(mock.terminalAttach).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("waits for the first IPC write before sending later user input", async () => {
    vi.stubGlobal("document", { createElement: createFakeElement });
    try {
      const mock = new MockIpcClient();
      let release!: () => void;
      const firstWrite = new Promise<void>((resolve) => { release = resolve; });
      const writes = vi.spyOn(mock, "terminalWrite").mockReturnValueOnce(firstWrite);
      let adapterCallbacks!: TerminalAdapterCallbacks;
      const mgr = new TerminalSessionManager(mock, sessionAdapter((callbacks) => {
        adapterCallbacks = callbacks;
      }, undefined, () => {}));
      const session = await mgr.createSession("order", "wterm-ghostty-dom", createFakeElement(), false);
      adapterCallbacks.onUserInput(encoder.encode("日本語"));
      adapterCallbacks.onUserInput(encoder.encode("\r"));
      expect(writes).toHaveBeenCalledExactlyOnceWith(session.attachId, encoder.encode("日本語"), "user");
      release();
      await firstWrite;
      expect(writes).toHaveBeenNthCalledWith(2, session.attachId, encoder.encode("\r"), "user");
      mgr.disposeAll();
    } finally {
      vi.unstubAllGlobals();
    }
  });

});

describe("Phase 0 Optimization guards (renderer restriction, focus dedup, bounded logs, cleanup)", () => {
  it("restricts TerminalBackendIdSchema to wterm + ghostty only", () => {
    expect(v.parse(TerminalBackendIdSchema, "wterm-ghostty-dom")).toBe("wterm-ghostty-dom");
    expect(v.parse(TerminalBackendIdSchema, "wterm-ghostty-webgl")).toBe("wterm-ghostty-webgl");
    expect(() => v.parse(TerminalBackendIdSchema, "gespenst")).toThrow();
    expect(() => v.parse(TerminalBackendIdSchema, "ghostty-web")).toThrow();
    expect(() => v.parse(TerminalBackendIdSchema, "xterm")).toThrow();
  });

  it("avoids redundant store updates on unchanged tab dimensions and bounds diagnostic logs", () => {
    pocStore.getState().setTabs([{ id: "t1", title: "t1", cols: 80, rows: 24 }], "t1");
    const tabsBefore = pocStore.getState().tabs;
    pocStore.getState().updateTabSize("t1", 80, 24);
    expect(pocStore.getState().tabs).toBe(tabsBefore);

    pocStore.getState().updateTabSize("t1", 100, 30);
    expect(pocStore.getState().tabs).not.toBe(tabsBefore);
    expect(pocStore.getState().tabs[0]).toEqual({ id: "t1", title: "t1", cols: 100, rows: 30 });

    pocStore.getState().clearLogs();
    pocStore.getState().setDiagnosticsEnabled(true);
    pocStore.getState().pushLog("input", "x".repeat(500));
    expect(pocStore.getState().logs.length).toBe(1);
    expect(pocStore.getState().logs[0]!.detail.length).toBeLessThanOrEqual(160);

    pocStore.getState().setDiagnosticsEnabled(false);
    pocStore.getState().pushLog("ime", "should-be-skipped");
    expect(pocStore.getState().logs.length).toBe(1);
    pocStore.getState().pushLog("system", "system-still-logged");
    expect(pocStore.getState().logs.length).toBe(2);

    pocStore.getState().setDiagnosticsEnabled(true);
    pocStore.getState().clearLogs();
    pocStore.getState().setTabs([], null);
  });

  it("initializes hidden terminals paused, deduplicates focus IPC, and unsubscribes events", async () => {
    const prevDoc = globalThis.document;
    (globalThis as unknown as { document: { createElement: () => HTMLDivElement } }).document = {
      createElement: createFakeElement,
    };

    const focusCalls: Array<{ attachId: AttachId; focused: boolean }> = [];
    const unsubCalls: number[] = [];
    const mountOptions: Array<TerminalAdapterOptions | undefined> = [];
    let disposedCount = 0;

    const mock = new MockIpcClient();
    const origFocus = mock.terminalFocus.bind(mock);
    mock.terminalFocus = async (attachId, focused) => {
      focusCalls.push({ attachId, focused });
      return origFocus(attachId, focused);
    };
    const origUnsub = mock.eventsUnsubscribe.bind(mock);
    mock.eventsUnsubscribe = async (subId) => {
      unsubCalls.push(subId);
      return origUnsub(subId);
    };

    const fakeAdapterFactory = (
      _backendId: unknown,
      _callbacks: TerminalAdapterCallbacks,
      options?: TerminalAdapterOptions,
    ): TerminalAdapter => {
      mountOptions.push(options);
      return {
        backendId: "wterm-ghostty-dom",
        cols: 80,
        rows: 24,
        async mount() {},
        write() {},
        resize() {},
        fit() {},
        focus() {},
        setHidden() {},
        async getSelection() {
          return "";
        },
        dispose() {
          disposedCount++;
        },
      };
    };

    try {
      const mgr = new TerminalSessionManager(mock, fakeAdapterFactory as never);
      await mgr.startEventStream();

      const root = createFakeElement();
      const s1 = await mgr.createSession("s1", "wterm-ghostty-dom", root, false);
      const s2 = await mgr.createSession("s2", "wterm-ghostty-dom", root, true);
      const s3 = await mgr.createSession("s3", "wterm-ghostty-dom", root, true);

      // Hidden sessions start with renderingPaused: true
      expect(mountOptions[0]?.renderingPaused).toBe(false);
      expect(mountOptions[1]?.renderingPaused).toBe(true);
      expect(mountOptions[2]?.renderingPaused).toBe(true);

      // Switching from s1 -> s2 should only send focus IPC for s1 (false) and s2 (true), NOT s3!
      mgr.setActive("s2");
      expect(focusCalls).toEqual([
        { attachId: s1.attachId, focused: false },
        { attachId: s2.attachId, focused: true },
      ]);

      // Repeating setActive("s2") should send zero focus IPC calls
      focusCalls.length = 0;
      mgr.setActive("s2");
      expect(focusCalls).toEqual([]);

      // Switching s2 -> s3 should only touch s2 and s3
      mgr.setActive("s3");
      expect(focusCalls).toEqual([
        { attachId: s2.attachId, focused: false },
        { attachId: s3.attachId, focused: true },
      ]);

      mgr.stopEventStream();
      expect(unsubCalls.length).toBe(1);

      mgr.disposeAll();
      expect(disposedCount).toBe(3);
    } finally {
      (globalThis as unknown as { document: typeof prevDoc }).document = prevDoc;
    }
  });
});
