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
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const ACK_BATCH_THRESHOLD = 16 * 1024;
const STATS_FLUSH_INTERVAL_MS = 100;
const MAX_LOG_BYTES_PREVIEW = 128;
const MAX_PENDING_WRITE_BYTES = 1024 * 1024;

function formatControlBytes(bytes: Uint8Array): string {
  const truncated = bytes.byteLength > MAX_LOG_BYTES_PREVIEW;
  const slice = truncated ? bytes.subarray(0, MAX_LOG_BYTES_PREVIEW) : bytes;
  const raw = decoder.decode(slice);
  const formatted = raw
    .replaceAll("\x1b", "ESC")
    .replaceAll("\r", "\\r")
    .replaceAll("\n", "\\n")
    .replaceAll("\x07", "BEL");
  return truncated ? `${formatted}...` : formatted;
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
  focused: boolean;
  applyingBackendResize: boolean;
  lastSentCols: number;
  lastSentRows: number;
  stream: { closed: boolean; writing: boolean; resizing: boolean } | null;
  resyncing: boolean;
  pendingWrites: PendingWrite[];
  pendingWriteBytes: number;
  pendingResize: { cols: number; rows: number } | null;
}

export type AdapterFactory = (
  backendId: TerminalBackendId,
  callbacks: TerminalAdapterCallbacks,
  options?: TerminalAdapterOptions,
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
  private eventStream: { subscriptionId: number | null } | null = null;
  private activeId: string | null = null;
  private creating = new Map<string, AbortController>();
  private pendingBytesDelta = 0;
  private pendingRepliesDelta = 0;
  private statsFlushTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    client?: UitermBackendClient,
    adapterFactory: AdapterFactory = (backendId, callbacks, options) =>
      createTerminalAdapter(backendId, callbacks, options),
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

  private flushStatsNow(): void {
    if (this.statsFlushTimer !== null) {
      clearTimeout(this.statsFlushTimer);
      this.statsFlushTimer = null;
    }
    if (this.pendingBytesDelta > 0 || this.pendingRepliesDelta > 0) {
      const b = this.pendingBytesDelta;
      const r = this.pendingRepliesDelta;
      this.pendingBytesDelta = 0;
      this.pendingRepliesDelta = 0;
      pocStore.getState().addStats(b, r);
    }
  }

  private scheduleStatsFlush(): void {
    if (this.statsFlushTimer !== null) return;
    this.statsFlushTimer = setTimeout(() => {
      this.statsFlushTimer = null;
      this.flushStatsNow();
    }, STATS_FLUSH_INTERVAL_MS);
  }

  async startEventStream(): Promise<void> {
    if (this.eventStream !== null) return;
    const stream: { subscriptionId: number | null } = { subscriptionId: null };
    this.eventStream = stream;
    try {
      const sub = await this.client.eventsSubscribe((ev) => {
        if (this.eventStream !== stream) return;
        if (ev.type === "disconnected") this.eventStream = null;
        this.handleCoreEvent(ev);
      });
      if (this.eventStream !== stream) {
        void this.client
          .eventsUnsubscribe(sub.subscriptionId)
          .catch((err) => this.reportIpcError("eventsUnsubscribe", err));
        return;
      }
      stream.subscriptionId = sub.subscriptionId;
      pocStore
        .getState()
        .pushLog(
          "system",
          `events subscribed (sub=${sub.subscriptionId}, seq=${sub.seq}, live=${sub.terminals.length})`,
        );
    } catch (err) {
      if (this.eventStream === stream) this.eventStream = null;
      this.reportIpcError("eventsSubscribe", err);
    }
  }

  stopEventStream(): void {
    const stream = this.eventStream;
    if (!stream) return;
    this.eventStream = null;
    if (stream.subscriptionId !== null) {
      const subId = stream.subscriptionId;
      stream.subscriptionId = null;
      void this.client
        .eventsUnsubscribe(subId)
        .catch((err) => this.reportIpcError("eventsUnsubscribe", err));
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

    void this.flushResize(session);
    void this.flushInput(session);
  }

  private async flushResize(session: ManagedSession): Promise<void> {
    const { stream, attachId } = session;
    if (!stream || stream.resizing || stream.closed || attachId <= 0) return;
    stream.resizing = true;
    try {
      while (!stream.closed && session.pendingResize) {
        const { cols, rows } = session.pendingResize;
        session.pendingResize = null;
        if (cols === session.lastSentCols && rows === session.lastSentRows) continue;
        try {
          await this.client.terminalResize(attachId, cols, rows);
          if (!stream.closed) {
            session.lastSentCols = cols;
            session.lastSentRows = rows;
          }
        } catch (err) {
          this.reportIpcError(`terminalResize:${session.id}`, err);
        }
      }
    } finally {
      stream.resizing = false;
    }
  }

  private async flushInput(session: ManagedSession): Promise<void> {
    const { stream, attachId } = session;
    if (!stream || stream.writing || stream.closed || attachId <= 0) return;
    stream.writing = true;
    try {
      while (!stream.closed) {
        const item = session.pendingWrites.shift();
        if (!item) break;
        try {
          await this.client.terminalWrite(attachId, item.bytes, item.kind);
        } catch (err) {
          this.reportIpcError(`terminalWrite:${session.id}`, err);
        } finally {
          if (!stream.closed) session.pendingWriteBytes -= item.bytes.byteLength;
        }
      }
    } finally {
      stream.writing = false;
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
      this.closeStream(session);
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

  private closeStream(session: ManagedSession): void {
    if (session.stream) session.stream.closed = true;
    session.attachId = 0;
    session.unackedBytes = 0;
    session.pendingWrites = [];
    session.pendingWriteBytes = 0;
    session.pendingResize = null;
  }

  private sendInput(session: ManagedSession | null, bytes: Uint8Array, kind: InputKind): void {
    if (!session?.stream || session.disposed || session.stream.closed || bytes.byteLength === 0) return;
    if (
      bytes.byteLength > MAX_PENDING_WRITE_BYTES - session.pendingWriteBytes ||
      session.pendingWrites.length >= 256
    ) {
      this.reportIpcError(`terminalWrite:${session.id}`, new Error("pending input queue full"));
      return;
    }
    session.pendingWriteBytes += bytes.byteLength;
    session.pendingWrites.push({ bytes: new Uint8Array(bytes), kind });
    void this.flushInput(session);
  }

  private async attach(session: ManagedSession): Promise<boolean> {
    const stream = { closed: false, failed: false, writing: false, resizing: false };
    session.stream = stream;
    try {
      const attached = await this.client.terminalAttach(
        {
          sessionId: session.sessionId,
          cols: session.adapter.cols,
          rows: session.adapter.rows,
          resizeNotifications: true,
        },
        (frame) => {
          if (session.disposed || session.stream !== stream || stream.closed) return;
          try {
            this.handleOutputFrame(session, frame);
          } catch (err) {
            stream.failed = true;
            const attachId = session.attachId;
            this.closeStream(session);
            this.reportIpcError(`render:${session.id}`, err);
            if (attachId > 0) {
              void this.client.terminalDetach(attachId)
                .catch((detachError) => this.reportIpcError(`terminalDetach:${session.id}`, detachError));
            }
          }
        },
      );
      if (session.disposed || stream.closed) {
        // CLOSE can arrive before the invoke reply, and the core may already have closed it.
        if (session.disposed || stream.failed) {
          await this.client.terminalDetach(attached.attachId)
            .catch((err) => this.reportIpcError(`terminalDetach:${session.id}`, err));
        }
        return false;
      }
      session.attachId = attached.attachId;
      this.flushPostAttach(session);
      return true;
    } catch (err) {
      this.closeStream(session);
      throw err;
    }
  }

  async createSession(
    id: string,
    backendId: TerminalBackendId,
    mountRoot: HTMLElement,
    hidden: boolean,
  ): Promise<ManagedSession> {
    if (this.creating.has(id) || this.sessions.has(id)) {
      throw new Error(`session already exists: ${id}`);
    }
    const creation = new AbortController();
    this.creating.set(id, creation);
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
      creation.signal.throwIfAborted();

      const initialPaused = hidden && pocStore.getState().pauseHiddenRendering;
      adapter = this.adapterFactory(
        backendId,
        {
          onUserInput: (bytes) => {
            const state = pocStore.getState();
            if (state.diagnosticsEnabled) {
              state.pushLog("input", `[${id}] user (${bytes.byteLength}B)`);
            }
            this.sendInput(sessionRef, bytes, "user");
          },
          onAutoResponse: (bytes) => {
            const state = pocStore.getState();
            const suppress = state.suppressAutoReplies;
            if (state.diagnosticsEnabled) {
              state.pushLog(
                "auto-reply",
                `[${id}] ${suppress ? "BLOCKED" : "AUTO(kind=6)"} (${bytes.byteLength}B): ${formatControlBytes(bytes)}`,
              );
            }
            if (suppress) {
              this.pendingRepliesDelta += 1;
              this.scheduleStatsFlush();
              return;
            }
            this.sendInput(sessionRef, bytes, "automatic");
          },
          onResize: (cols, rows) => {
            pocStore.getState().updateTabSize(id, cols, rows);
            if (!sessionRef || sessionRef.disposed || sessionRef.stream?.closed) return;
            if (sessionRef.applyingBackendResize) return;
            sessionRef.pendingResize = { cols, rows };
            void this.flushResize(sessionRef);
          },
        },
        { renderingPaused: initialPaused },
      );

      await adapter.mount(container);
      creation.signal.throwIfAborted();

      const session: ManagedSession = {
        id,
        sessionId: termInfo.address,
        attachId: 0,
        adapter,
        container,
        unackedBytes: 0,
        mounted: true,
        disposed: false,
        focused: !hidden,
        applyingBackendResize: false,
        lastSentCols: adapter.cols,
        lastSentRows: adapter.rows,
        stream: null,
        resyncing: false,
        pendingWrites: [],
        pendingWriteBytes: 0,
        pendingResize: null,
      };
      sessionRef = session;
      this.sessions.set(id, session);

      if (initialPaused) {
        adapter.setHidden(true);
      }

      await this.attach(session);
      creation.signal.throwIfAborted();
      pocStore.getState().updateTabSize(id, adapter.cols, adapter.rows);
      return session;
    } catch (err) {
      this.reportIpcError(`createSession:${id}`, err);
      if (!sessionRef?.disposed) {
        this.sessions.delete(id);
        if (sessionRef) sessionRef.disposed = true;
        try {
          adapter?.dispose();
        } catch (disposeError) {
          this.reportIpcError(`disposeSession:${id}`, disposeError);
        }
        if (termInfo) {
          await this.client.terminalKill(termInfo.address)
            .catch((killError) => this.reportIpcError(`terminalKill:${id}`, killError));
        }
      }
      container.remove();
      throw err;
    } finally {
      this.creating.delete(id);
    }
  }

  get(id: string): ManagedSession | undefined {
    return this.sessions.get(id);
  }

  setActive(activeId: string | null): void {
    const pauseHidden = pocStore.getState().pauseHiddenRendering;
    const prevActiveId = this.activeId;
    this.activeId = activeId;

    for (const [id, session] of this.sessions) {
      const isActive = id === activeId;
      const nextDisplay = isActive ? "block" : "none";
      if (session.container.style.display !== nextDisplay) {
        session.container.style.display = nextDisplay;
      }
      session.adapter.setHidden(pauseHidden && !isActive);
      if (session.focused !== isActive && session.attachId > 0) {
        session.focused = isActive;
        void this.client
          .terminalFocus(session.attachId, isActive)
          .catch((err) => this.reportIpcError(`terminalFocus:${id}`, err));
      }
      if (isActive && prevActiveId !== activeId) {
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
    if (!session || session.disposed || session.resyncing || this.creating.has(id)) return;
    session.resyncing = true;
    try {
      const oldAttachId = session.attachId;
      this.closeStream(session);
      if (oldAttachId > 0) {
        try {
          await this.client.terminalDetach(oldAttachId);
        } catch (err) {
          const code = (err as IpcError | undefined)?.code;
          if (code !== "invalid_attach" && code !== "core_disconnected") throw err;
        }
      }
      await this.client.coreReconnect();
      await this.startEventStream();
      if (!session.disposed && await this.attach(session)) {
        pocStore.getState().pushLog("system", `[${id}] resynced (attachId=${session.attachId})`);
      }
    } catch (err) {
      this.reportIpcError(`resyncSession:${id}`, err);
    } finally {
      session.resyncing = false;
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
    this.creating.get(id)?.abort();
    const session = this.sessions.get(id);
    if (!session) return;
    session.disposed = true;
    this.sessions.delete(id);
    if (this.activeId === id) {
      this.activeId = null;
    }
    const aid = session.attachId;
    this.closeStream(session);
    if (aid > 0) {
      void this.client
        .terminalDetach(aid)
        .catch((err) => this.reportIpcError(`terminalDetach:${id}`, err));
    }
    void this.client
      .terminalKill(session.sessionId)
      .catch((err) => this.reportIpcError(`terminalKill:${id}`, err));
    try {
      session.adapter.dispose();
    } finally {
      session.container.remove();
    }
  }

  disposeAll(): void {
    this.flushStatsNow();
    this.activeId = null;
    for (const id of new Set([...this.creating.keys(), ...this.sessions.keys()])) {
      this.disposeSession(id);
    }
  }
}

export const sessionManager = new TerminalSessionManager();
