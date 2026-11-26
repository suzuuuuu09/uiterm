import { isTauriRuntime, TauriIpcClient } from "../ipc/client.ts";
import { MockIpcClient } from "../ipc/mock.ts";
import type { AttachId, OutputFrame, SessionId, UitermBackendClient } from "../ipc/types.ts";
import { pocStore } from "../store/pocStore.ts";
import {
  JAPANESE_TEST_PAYLOAD,
  VT_ANSI_TEST_PAYLOAD,
  VT_QUERY_TEST_PAYLOAD,
} from "../transport/mockHerdrTransport.ts";
import type { TerminalBackendId } from "../transport/schemas.ts";
import { createTerminalAdapter } from "./createAdapter.ts";
import type { TerminalAdapter } from "./types.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const ACK_BATCH_THRESHOLD = 16 * 1024;

function formatControlBytes(bytes: Uint8Array): string {
  const raw = decoder.decode(bytes);
  return raw
    .replaceAll("\x1b", "ESC")
    .replaceAll("\r", "\\r")
    .replaceAll("\n", "\\n")
    .replaceAll("\x07", "BEL");
}

export interface ManagedSession {
  readonly id: string;
  readonly sessionId: SessionId;
  attachId: AttachId;
  readonly adapter: TerminalAdapter;
  readonly container: HTMLDivElement;
  unackedBytes: number;
  mounted: boolean;
}

/**
 * Imperative session manager wired directly to UitermBackendClient (docs/poc/interface.md).
 * - Uses TauriIpcClient when running inside Tauri, MockIpcClient in standalone browser.
 * - Pipes binary OutputFrame bytes directly to TerminalAdapter.write(bytes) without React/Zustand.
 * - Separates user input ("user") from terminal auto-responses ("automatic" / suppressed).
 * - Batches CREDIT acknowledgments via terminalAck().
 */
class TerminalSessionManager {
  readonly client: UitermBackendClient = isTauriRuntime()
    ? new TauriIpcClient()
    : new MockIpcClient();
  readonly runtimeMode: "tauri" | "mock" = isTauriRuntime() ? "tauri" : "mock";

  private sessions = new Map<string, ManagedSession>();
  private pendingBytesDelta = 0;
  private pendingRepliesDelta = 0;
  private statsFlushScheduled = false;

  private scheduleStatsFlush(): void {
    if (this.statsFlushScheduled) return;
    this.statsFlushScheduled = true;
    queueMicrotask(() => {
      this.statsFlushScheduled = false;
      if (this.pendingBytesDelta > 0 || this.pendingRepliesDelta > 0) {
        const b = this.pendingBytesDelta;
        const r = this.pendingRepliesDelta;
        this.pendingBytesDelta = 0;
        this.pendingRepliesDelta = 0;
        pocStore.getState().addStats(b, r);
      }
    });
  }

  private handleOutputFrame(session: ManagedSession, frame: OutputFrame): void {
    if (frame.kind === "resize") {
      session.adapter.resize(frame.cols, frame.rows);
      pocStore.getState().updateTabSize(session.id, frame.cols, frame.rows);
      return;
    }
    if (frame.kind === "close") {
      pocStore
        .getState()
        .pushLog("system", `[${session.id}] closed (${frame.reason}, exit=${frame.exitCode ?? "-"})`);
      return;
    }
    // frame.kind === "data": direct binary handoff to terminal renderer
    const len = frame.bytes.byteLength;
    this.pendingBytesDelta += len;
    this.scheduleStatsFlush();
    session.adapter.write(frame.bytes);

    session.unackedBytes += len;
    if (session.unackedBytes >= ACK_BATCH_THRESHOLD) {
      const ack = session.unackedBytes;
      session.unackedBytes = 0;
      void this.client.terminalAck(session.attachId, ack);
    }
  }

  async createSession(
    id: string,
    backendId: TerminalBackendId,
    mountRoot: HTMLElement,
    hidden: boolean,
  ): Promise<ManagedSession> {
    const container = document.createElement("div");
    container.dataset.sessionId = id;
    container.className = "absolute inset-0 h-full w-full";
    container.style.display = hidden ? "none" : "block";
    mountRoot.appendChild(container);

    const termInfo = await this.client.terminalCreate({ name: id, cols: 80, rows: 24 });

    let sessionRef: ManagedSession | null = null;

    const adapter = createTerminalAdapter(backendId, {
      onUserInput: (bytes) => {
        pocStore
          .getState()
          .pushLog("input", `[${id}] user (${bytes.byteLength}B): ${formatControlBytes(bytes)}`);
        if (sessionRef) {
          void this.client.terminalWrite(sessionRef.attachId, bytes, "user");
        }
      },
      onAutoResponse: (bytes) => {
        const suppress = pocStore.getState().suppressAutoReplies;
        pocStore
          .getState()
          .pushLog(
            "auto-reply",
            `[${id}] ${suppress ? "BLOCKED" : "AUTO(kind=6)"} (${bytes.byteLength}B): ${formatControlBytes(bytes)}`,
          );
        if (suppress) {
          this.pendingRepliesDelta += 1;
          this.scheduleStatsFlush();
        } else if (sessionRef) {
          void this.client.terminalWrite(sessionRef.attachId, bytes, "automatic");
        }
      },
      onResize: (cols, rows) => {
        pocStore.getState().updateTabSize(id, cols, rows);
        if (sessionRef) {
          void this.client.terminalResize(sessionRef.attachId, cols, rows);
        }
      },
    });

    await adapter.mount(container);

    const session: ManagedSession = {
      id,
      sessionId: termInfo.address,
      attachId: 0,
      adapter,
      container,
      unackedBytes: 0,
      mounted: true,
    };
    sessionRef = session;
    this.sessions.set(id, session);

    if (hidden && pocStore.getState().pauseHiddenRendering) {
      adapter.setHidden(true);
    }

    const attachResult = await this.client.terminalAttach(
      {
        sessionId: termInfo.address,
        cols: adapter.cols,
        rows: adapter.rows,
        resizeNotifications: true,
      },
      (frame) => this.handleOutputFrame(session, frame),
    );
    session.attachId = attachResult.attachId;
    pocStore.getState().updateTabSize(id, adapter.cols, adapter.rows);
    return session;
  }

  get(id: string): ManagedSession | undefined {
    return this.sessions.get(id);
  }

  setActive(activeId: string | null): void {
    const pauseHidden = pocStore.getState().pauseHiddenRendering;
    for (const [id, session] of this.sessions) {
      const isActive = id === activeId;
      session.container.style.display = isActive ? "block" : "none";
      session.adapter.setHidden(pauseHidden && !isActive);
      if (session.attachId > 0) {
        void this.client.terminalFocus(session.attachId, isActive);
      }
      if (isActive) {
        session.adapter.fit();
        session.adapter.focus();
      }
    }
  }

  applyHiddenPausePolicy(pauseHidden: boolean, activeId: string | null): void {
    for (const [id, session] of this.sessions) {
      session.adapter.setHidden(pauseHidden && id !== activeId);
    }
  }

  /** Re-attach to herdr-core to trigger a full screen resync. */
  async resyncSession(id: string): Promise<void> {
    const session = this.sessions.get(id);
    if (!session) return;
    if (session.attachId > 0) {
      await this.client.terminalDetach(session.attachId);
    }
    const attached = await this.client.terminalAttach(
      {
        sessionId: session.sessionId,
        cols: session.adapter.cols,
        rows: session.adapter.rows,
        resizeNotifications: true,
      },
      (frame) => this.handleOutputFrame(session, frame),
    );
    session.attachId = attached.attachId;
    pocStore.getState().pushLog("system", `[${id}] resynced (attachId=${attached.attachId})`);
  }

  /** Direct renderer test probes (bypassing PTY echo to test VT/IME/rendering directly). */
  injectJapaneseTest(id: string): void {
    const session = this.sessions.get(id);
    if (!session) return;
    const bytes = encoder.encode(JAPANESE_TEST_PAYLOAD);
    this.pendingBytesDelta += bytes.byteLength;
    this.scheduleStatsFlush();
    session.adapter.write(bytes);
  }

  injectVtTest(id: string): void {
    const session = this.sessions.get(id);
    if (!session) return;
    const bytes = encoder.encode(VT_ANSI_TEST_PAYLOAD);
    this.pendingBytesDelta += bytes.byteLength;
    this.scheduleStatsFlush();
    session.adapter.write(bytes);
  }

  injectQueryProbe(id: string): void {
    const session = this.sessions.get(id);
    if (!session) return;
    const bytes = encoder.encode(VT_QUERY_TEST_PAYLOAD);
    this.pendingBytesDelta += bytes.byteLength;
    this.scheduleStatsFlush();
    session.adapter.write(bytes);
  }

  injectBurst(
    id: string,
    lines = 10_000,
    chunkLines = 200,
  ): { totalBytes: number; durationMs: number } {
    const session = this.sessions.get(id);
    if (!session) return { totalBytes: 0, durationMs: 0 };
    const start = performance.now();
    let totalBytes = 0;
    for (let i = 0; i < lines; i += chunkLines) {
      const end = Math.min(i + chunkLines, lines);
      let block = "";
      for (let line = i; line < end; line++) {
        block += `\x1b[36m[${String(line).padStart(5, "0")}]\x1b[0m \x1b[32mINFO\x1b[0m uiterm-bench: 日本語ログ出力テスト payload=0123456789abcdef\r\n`;
      }
      const bytes = encoder.encode(block);
      totalBytes += bytes.byteLength;
      session.adapter.write(bytes);
    }
    this.pendingBytesDelta += totalBytes;
    this.scheduleStatsFlush();
    return { totalBytes, durationMs: performance.now() - start };
  }

  disposeSession(id: string): void {
    const session = this.sessions.get(id);
    if (!session) return;
    this.sessions.delete(id);
    if (session.attachId > 0) {
      void this.client.terminalDetach(session.attachId);
    }
    void this.client.terminalKill(session.sessionId);
    session.adapter.dispose();
    session.container.remove();
  }

  disposeAll(): void {
    for (const id of Array.from(this.sessions.keys())) {
      this.disposeSession(id);
    }
  }
}

export const sessionManager = new TerminalSessionManager();
