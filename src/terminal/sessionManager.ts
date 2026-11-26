import { isTauriRuntime, TauriIpcClient } from "../ipc/client.ts";
import { MockIpcClient } from "../ipc/mock.ts";
import type {
  AttachId,
  CoreEvent,
  InputKind,
  IpcError,
  OutputFrame,
  SessionId,
  UitermBackendClient,
} from "../ipc/types.ts";
import { pocStore } from "../store/pocStore.ts";
import {
  JAPANESE_TEST_PAYLOAD,
  VT_ANSI_TEST_PAYLOAD,
  VT_QUERY_TEST_PAYLOAD,
} from "../transport/mockHerdrTransport.ts";
import type { TerminalBackendId } from "../transport/schemas.ts";
import { createTerminalAdapter } from "./createAdapter.ts";
import type { TerminalAdapter, TerminalAdapterCallbacks } from "./types.ts";

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

function formatIpcError(err: unknown): string {
  if (err && typeof err === "object" && "code" in err && "message" in err) {
    const ipc = err as IpcError;
    return ipc.hint
      ? `[${ipc.code}] ${ipc.message} (${ipc.hint})`
      : `[${ipc.code}] ${ipc.message}`;
  }
  return err instanceof Error ? err.message : String(err);
}

interface PendingWrite {
  readonly bytes: Uint8Array;
  readonly kind: InputKind;
}

export interface ManagedSession {
  readonly id: string;
  readonly sessionId: SessionId;
  attachId: AttachId;
  readonly adapter: TerminalAdapter;
  readonly container: HTMLDivElement;
  unackedBytes: number;
  mounted: boolean;
  disposed: boolean;
  applyingBackendResize: boolean;
  lastSentCols: number;
  lastSentRows: number;
  pendingWrites: PendingWrite[];
  pendingResize: { cols: number; rows: number } | null;
}

export type AdapterFactory = (
  backendId: TerminalBackendId,
  callbacks: TerminalAdapterCallbacks,
) => TerminalAdapter;

/**
 * Imperative session manager wired directly to UitermBackendClient (docs/poc/interface.md).
 * - Uses TauriIpcClient when running inside Tauri, MockIpcClient in standalone browser.
 * - Pipes binary OutputFrame bytes directly to TerminalAdapter.write(bytes) without React/Zustand.
 * - Prevents attachId=0 IPC calls before terminalAttach() resolves by queuing ACK/writes/resizes.
 * - Prevents backend-originated RESIZE notification loops via guard flag + dimension deduplication.
 * - Captures all IPC errors cleanly and maintains session lifecycle invariants.
 */
export class TerminalSessionManager {
  readonly client: UitermBackendClient;
  readonly runtimeMode: "tauri" | "mock";
  private readonly adapterFactory: AdapterFactory;

  private sessions = new Map<string, ManagedSession>();
  private eventSubscriptionId: number | null = null;
  private pendingBytesDelta = 0;
  private pendingRepliesDelta = 0;
  private statsFlushScheduled = false;

  constructor(
    client?: UitermBackendClient,
    adapterFactory: AdapterFactory = (backendId, callbacks) =>
      createTerminalAdapter(backendId, callbacks),
  ) {
    if (client) {
      this.client = client;
      this.runtimeMode = "mock";
    } else if (isTauriRuntime()) {
      this.client = new TauriIpcClient();
      this.runtimeMode = "tauri";
    } else {
      this.client = new MockIpcClient();
      this.runtimeMode = "mock";
    }
    this.adapterFactory = adapterFactory;
  }

  private reportIpcError(context: string, err: unknown): void {
    pocStore.getState().pushLog("system", `IPC error (${context}): ${formatIpcError(err)}`);
  }

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

  async startEventStream(): Promise<void> {
    if (this.eventSubscriptionId !== null) return;
    try {
      const sub = await this.client.eventsSubscribe((ev) => this.handleCoreEvent(ev));
      this.eventSubscriptionId = sub.subscriptionId;
      pocStore
        .getState()
        .pushLog(
          "system",
          `events subscribed (sub=${sub.subscriptionId}, seq=${sub.seq}, live=${sub.terminals.length})`,
        );
    } catch (err) {
      this.reportIpcError("eventsSubscribe", err);
    }
  }

  private handleCoreEvent(ev: CoreEvent): void {
    if (ev.type === "status") {
      const state = ev.data.status?.state ?? "clear";
      const app = ev.data.status?.app ? ` app=${ev.data.status.app}` : "";
      pocStore.getState().pushLog("system", `[${ev.terminal}] OSC 7501 status=${state}${app}`);
    } else if (ev.type === "turn") {
      pocStore
        .getState()
        .pushLog("system", `[${ev.terminal}] turn=${ev.data.turn} (${ev.data.state})`);
    } else if (ev.type === "finished") {
      pocStore
        .getState()
        .pushLog(
          "system",
          `[${ev.terminal}] finished (reason=${ev.data.reason ?? "-"}, exit=${ev.data.exitCode ?? "-"})`,
        );
    } else if (ev.type === "disconnected") {
      this.eventSubscriptionId = null;
      pocStore.getState().pushLog("system", `herdr-core disconnected: ${ev.reason}`);
    } else if (ev.type === "missed") {
      pocStore
        .getState()
        .pushLog("system", `events missed (seq=${ev.seq}, snapshot=${ev.terminals.length})`);
    }
  }

  private flushPostAttach(session: ManagedSession): void {
    if (session.disposed || session.attachId <= 0) return;

    if (session.unackedBytes >= ACK_BATCH_THRESHOLD) {
      const ack = session.unackedBytes;
      session.unackedBytes = 0;
      void this.client
        .terminalAck(session.attachId, ack)
        .catch((err) => this.reportIpcError(`terminalAck:${session.id}`, err));
    }

    if (session.pendingResize) {
      const { cols, rows } = session.pendingResize;
      session.pendingResize = null;
      if (cols !== session.lastSentCols || rows !== session.lastSentRows) {
        session.lastSentCols = cols;
        session.lastSentRows = rows;
        void this.client
          .terminalResize(session.attachId, cols, rows)
          .catch((err) => this.reportIpcError(`terminalResize:${session.id}`, err));
      }
    }

    if (session.pendingWrites.length > 0) {
      const queued = session.pendingWrites.splice(0, session.pendingWrites.length);
      for (const item of queued) {
        void this.client
          .terminalWrite(session.attachId, item.bytes, item.kind)
          .catch((err) => this.reportIpcError(`terminalWrite:${session.id}`, err));
      }
    }
  }

  private handleOutputFrame(session: ManagedSession, frame: OutputFrame): void {
    if (session.disposed) return;

    if (frame.kind === "resize") {
      // Guard against echoing backend-originated RESIZE notifications back to herdr-core.
      session.applyingBackendResize = true;
      session.lastSentCols = frame.cols;
      session.lastSentRows = frame.rows;
      try {
        session.adapter.resize(frame.cols, frame.rows);
      } finally {
        session.applyingBackendResize = false;
      }
      pocStore.getState().updateTabSize(session.id, frame.cols, frame.rows);
      return;
    }

    if (frame.kind === "close") {
      session.attachId = 0;
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
    if (session.attachId > 0 && session.unackedBytes >= ACK_BATCH_THRESHOLD) {
      const ack = session.unackedBytes;
      session.unackedBytes = 0;
      void this.client
        .terminalAck(session.attachId, ack)
        .catch((err) => this.reportIpcError(`terminalAck:${session.id}`, err));
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

    let termInfo: { address: SessionId } | null = null;
    let adapter: TerminalAdapter | null = null;
    let sessionRef: ManagedSession | null = null;

    try {
      termInfo = await this.client.terminalCreate({ name: id, cols: 80, rows: 24 });

      adapter = this.adapterFactory(backendId, {
        onUserInput: (bytes) => {
          pocStore
            .getState()
            .pushLog("input", `[${id}] user (${bytes.byteLength}B): ${formatControlBytes(bytes)}`);
          if (!sessionRef || sessionRef.disposed) return;
          if (sessionRef.attachId > 0) {
            void this.client
              .terminalWrite(sessionRef.attachId, bytes, "user")
              .catch((err) => this.reportIpcError(`terminalWrite:${id}`, err));
          } else {
            sessionRef.pendingWrites.push({ bytes: new Uint8Array(bytes), kind: "user" });
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
            return;
          }
          if (!sessionRef || sessionRef.disposed) return;
          if (sessionRef.attachId > 0) {
            void this.client
              .terminalWrite(sessionRef.attachId, bytes, "automatic")
              .catch((err) => this.reportIpcError(`terminalWriteAuto:${id}`, err));
          } else {
            sessionRef.pendingWrites.push({ bytes: new Uint8Array(bytes), kind: "automatic" });
          }
        },
        onResize: (cols, rows) => {
          pocStore.getState().updateTabSize(id, cols, rows);
          if (!sessionRef || sessionRef.disposed) return;
          // Suppress feedback loop when resize originated from backend or dimensions are unchanged.
          if (
            sessionRef.applyingBackendResize ||
            (cols === sessionRef.lastSentCols && rows === sessionRef.lastSentRows)
          ) {
            return;
          }
          if (sessionRef.attachId > 0) {
            sessionRef.lastSentCols = cols;
            sessionRef.lastSentRows = rows;
            void this.client
              .terminalResize(sessionRef.attachId, cols, rows)
              .catch((err) => this.reportIpcError(`terminalResize:${id}`, err));
          } else {
            sessionRef.pendingResize = { cols, rows };
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
        disposed: false,
        applyingBackendResize: false,
        lastSentCols: adapter.cols,
        lastSentRows: adapter.rows,
        pendingWrites: [],
        pendingResize: null,
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

      if (session.disposed) {
        void this.client.terminalDetach(attachResult.attachId).catch(() => {});
        return session;
      }

      session.attachId = attachResult.attachId;
      this.flushPostAttach(session);
      pocStore.getState().updateTabSize(id, adapter.cols, adapter.rows);
      return session;
    } catch (err) {
      this.reportIpcError(`createSession:${id}`, err);
      this.sessions.delete(id);
      if (sessionRef) {
        sessionRef.disposed = true;
      }
      adapter?.dispose();
      container.remove();
      if (termInfo) {
        void this.client.terminalKill(termInfo.address).catch(() => {});
      }
      throw err;
    }
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
        void this.client
          .terminalFocus(session.attachId, isActive)
          .catch((err) => this.reportIpcError(`terminalFocus:${id}`, err));
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
    if (!session || session.disposed) return;
    try {
      if (session.attachId > 0) {
        const oldAttachId = session.attachId;
        session.attachId = 0;
        session.unackedBytes = 0;
        await this.client.terminalDetach(oldAttachId);
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
      if (session.disposed) {
        void this.client.terminalDetach(attached.attachId).catch(() => {});
        return;
      }
      session.attachId = attached.attachId;
      this.flushPostAttach(session);
      pocStore.getState().pushLog("system", `[${id}] resynced (attachId=${attached.attachId})`);
    } catch (err) {
      this.reportIpcError(`resyncSession:${id}`, err);
    }
  }

  /** Direct renderer test probes (bypassing PTY echo to test VT/IME/rendering directly). */
  injectJapaneseTest(id: string): void {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return;
    const bytes = encoder.encode(JAPANESE_TEST_PAYLOAD);
    this.pendingBytesDelta += bytes.byteLength;
    this.scheduleStatsFlush();
    session.adapter.write(bytes);
  }

  injectVtTest(id: string): void {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return;
    const bytes = encoder.encode(VT_ANSI_TEST_PAYLOAD);
    this.pendingBytesDelta += bytes.byteLength;
    this.scheduleStatsFlush();
    session.adapter.write(bytes);
  }

  injectQueryProbe(id: string): void {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return;
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
    if (!session || session.disposed) return { totalBytes: 0, durationMs: 0 };
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
    session.disposed = true;
    this.sessions.delete(id);
    if (session.attachId > 0) {
      const aid = session.attachId;
      session.attachId = 0;
      void this.client
        .terminalDetach(aid)
        .catch((err) => this.reportIpcError(`terminalDetach:${id}`, err));
    }
    void this.client
      .terminalKill(session.sessionId)
      .catch((err) => this.reportIpcError(`terminalKill:${id}`, err));
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
