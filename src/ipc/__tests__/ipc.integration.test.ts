import { describe, expect, it, vi } from "vite-plus/test";
import { createAppRuntime } from "../../app/runtime.ts";
import type { TerminalAdapter, TerminalAdapterCallbacks } from "../../terminal/types.ts";
import { MockIpcClient } from "../mock.ts";

const encoder = new TextEncoder();

function createFakeElement(): HTMLDivElement {
  return {
    dataset: {},
    className: "",
    style: { display: "" },
    appendChild(child: unknown) {
      return child;
    },
    remove() {},
  } as unknown as HTMLDivElement;
}

describe("Mock IPC integration (Runtime + Store + ActionRegistry + SessionManager)", () => {
  it("synchronizes terminal lifecycle, user input, OSC 7501 agent state, and missed event recovery", async () => {
    vi.stubGlobal("document", { createElement: createFakeElement });
    const mock = new MockIpcClient();
    const receivedChunks: Uint8Array[] = [];
    let adapterCallbacks: TerminalAdapterCallbacks | null = null;

    const runtime = createAppRuntime({
      client: mock,
      adapterFactory: (_backendId, callbacks): TerminalAdapter => {
        adapterCallbacks = callbacks;
        return {
          backendId: "wterm-ghostty-dom",
          cols: 80,
          rows: 24,
          async mount() {},
          write(bytes) {
            receivedChunks.push(new Uint8Array(bytes));
          },
          resize() {},
          fit() {},
          focus() {},
          setHidden() {},
          async getSelection() {
            return "";
          },
          dispose() {},
        };
      },
    });

    try {
      runtime.setMountRoot(createFakeElement());
      await runtime.sessionManager.startEventStream();
      expect(runtime.store.getState().coreConnection.status).toBe("connected");

      // Create terminal via ActionRegistry
      const created = await runtime.actions.execute(
        "terminal.create",
        { title: "Agent Shell", cols: 100, rows: 30 },
        { caller: "gui" },
      );
      expect(created.ok).toBe(true);
      if (!created.ok) return;

      const { tabId, sessionId } = created.value;
      // Wait for mock's initial screen microtask
      await Promise.resolve();
      expect(receivedChunks.length).toBeGreaterThan(0);

      // Verify herdr-core created event populated store.sessions
      expect(runtime.store.getState().sessions[sessionId]?.state).toBe("live");

      // Send Japanese UTF-8 user input; ensure it reaches mock IPC AND is NOT logged in appLogger
      const expectedInput = encoder.encode("echo こんにちは\r");
      adapterCallbacks!.onUserInput(expectedInput);
      await Promise.resolve();
      expect(mock.recordedInputs).toHaveLength(1);
      expect(mock.recordedInputs[0]?.kind).toBe("user");
      expect(mock.recordedInputs[0]?.bytes).toEqual(expectedInput);
      const allLogsJson = JSON.stringify(runtime.logger.getEntries());
      expect(allLogsJson).not.toContain("こんにちは");

      // Emit OSC 7501 status event from mock IPC -> reflected in store.agentStates
      mock.emitTestEvent({
        type: "status",
        seq: 100,
        terminal: sessionId,
        data: {
          status: { state: "working", app: "codex", title: "Implementing Phase 1" },
          records: {},
          reported: true,
        },
      });
      expect(runtime.store.getState().agentStates[sessionId]).toMatchObject({
        status: "working",
        app: "codex",
        title: "Implementing Phase 1",
      });

      // Missed event replaces store.sessions and advances lastSeq
      const currentSnapshot = (await mock.terminalList(false))[0]!;
      mock.emitTestEvent({
        type: "missed",
        seq: 150,
        terminals: [{ ...currentSnapshot, title: "recovered-after-missed" }],
      });
      expect(runtime.store.getState().coreConnection.lastSeq).toBe(150);
      expect(runtime.store.getState().sessions[sessionId]?.title).toBe("recovered-after-missed");

      // Simulate disconnect event
      mock.emitTestEvent({ type: "disconnected", reason: "mock-disconnect" });
      expect(runtime.store.getState().coreConnection.status).toBe("disconnected");

      // Resync via ActionRegistry reconnects and re-subscribes
      const resync = await runtime.actions.execute("terminal.resync", { tabId }, { caller: "cli" });
      expect(resync.ok).toBe(true);
      expect(runtime.store.getState().coreConnection.status).toBe("connected");

      // Close terminal via ActionRegistry kills session in mock backend
      const close = await runtime.actions.execute("terminal.close", { tabId }, { caller: "gui" });
      expect(close.ok).toBe(true);
      expect(await mock.terminalList(false)).toEqual([]);
      expect(await mock.terminalList(true)).toEqual([
        expect.objectContaining({ address: sessionId, state: "finished", reason: "killed" }),
      ]);

      // Partial dimensions (cols only) must still send both cols and rows so Rust validate() accepts it
      const createSpy = vi.spyOn(mock, "terminalCreate");
      const partial = await runtime.actions.execute(
        "terminal.create",
        { cols: 120 },
        { caller: "cli" },
      );
      expect(partial.ok).toBe(true);
      expect(createSpy).toHaveBeenCalledWith(
        expect.objectContaining({ cols: 120, rows: 24 }),
      );
    } finally {
      runtime.dispose();
      vi.unstubAllGlobals();
    }
  });
});
