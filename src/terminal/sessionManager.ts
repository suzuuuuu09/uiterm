import type { StoreApi } from "zustand/vanilla";
import { AppError, normalizeError } from "../errors/appError.ts";
import { isTauriRuntime, TauriIpcClient } from "../ipc/client.ts";
import { MockIpcClient } from "../ipc/mock.ts";
import type {
  AttachId,
  CoreEvent,
  InputKind,
  IpcError,
  OutputFrame,
  SessionId,
  TerminalCreateParams,
  UitermBackendClient,
} from "../ipc/types.ts";
import { appLogger, type Logger } from "../logging/logger.ts";
import {
  JAPANESE_TEST_PAYLOAD,
  VT_ANSI_TEST_PAYLOAD,
  VT_QUERY_TEST_PAYLOAD,
} from "../poc/probes.ts";
import { appStore, type AppState } from "../state/appStore.ts";
import {
  parseStatusEventData,
  parseTerminationEventData,
  parseTurnEventData,
} from "../validation/ipcSchemas.ts";
import type { TerminalBackendId, UserSettings } from "../validation/settingsSchemas.ts";
import { createTerminalAdapter } from "./createAdapter.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

const encoder = new TextEncoder();
const ACK_BATCH_THRESHOLD = 16 * 1024;
const STATS_FLUSH_INTERVAL_MS = 100;
const MAX_PENDING_WRITE_BYTES = 1024 * 1024;
const MAX_PENDING_WRITES = 256;

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

export interface SessionDiagnosticsSink {
  isEnabled: () => boolean;
  addStats: (bytesDelta: number, repliesDelta: number) => void;
  pushLog: (kind: "ime" | "input" | "auto-reply" | "system", detail: string) => void;
}

export interface TerminalSessionManagerOptions {
  store?: StoreApi<AppState>;
  logger?: Logger;
  diagnostics?: SessionDiagnosticsSink;
}

/**
 * Imperative terminal session manager wired to `UitermBackendClient`.
 *
 * Architectural invariants:
 * - Binary `OutputFrame` bytes flow directly to `TerminalAdapter.write(bytes)` and NEVER enter Zustand.
 * - Authoritative session lifecycle & OSC 7501 metadata from `herdr-core` sync into `AppState`.
 * - Prevents `attachId = 0` IPC calls before `terminalAttach()` resolves by queuing ACK/writes/resizes.
 * - Prevents backend-originated RESIZE loops via guard flag + dimension deduplication.
 * - Normal logs never record raw terminal input bytes or IME text.
 */
export class TerminalSessionManager {
  readonly client: UitermBackendClient;
  readonly runtimeMode: "tauri" | "mock";
  private readonly adapterFactory: AdapterFactory;
  private readonly store: StoreApi<AppState>;
  private readonly logger: Logger;
  private readonly diagnostics?: SessionDiagnosticsSink;

  private sessions = new Map<string, ManagedSession>();
  private eventStream: { subscriptionId: number | null } | null = null;
  private activeId: string | null = null;
  private windowFocused = true;
  private creating = new Map<string, AbortController>();
  private pendingBytesDelta = 0;
  private pendingRepliesDelta = 0;
  private statsFlushTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    client?: UitermBackendClient,
    adapterFactory: AdapterFactory = (backendId, callbacks, options) =>
      createTerminalAdapter(backendId, callbacks, options),
    options: TerminalSessionManagerOptions = {},
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
    this.store = options.store ?? appStore;
    this.logger = options.logger ?? appLogger;
    this.diagnostics = options.diagnostics;
  }

  private reportError(context: string, err: unknown): AppError {
    const appErr = normalizeError(err, { context });
    this.logger.logError("terminal", appErr, { context });
    this.diagnostics?.pushLog("system", `IPC error (${context}): ${appErr.formatDeveloperSummary()}`);
    return appErr;
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
      this.diagnostics?.addStats(b, r);
    }
  }

  private scheduleStatsFlush(): void {
    if (!this.diagnostics || this.statsFlushTimer !== null) return;
    this.statsFlushTimer = setTimeout(() => {
      this.statsFlushTimer = null;
      this.flushStatsNow();
    }, STATS_FLUSH_INTERVAL_MS);
  }

  async startEventStream(): Promise<void> {
    if (this.eventStream !== null) return;
    const stream: {
      subscriptionId: number | null;
      ready: boolean;
      pending: CoreEvent[];
    } = { subscriptionId: null, ready: false, pending: [] };
    this.eventStream = stream;
    try {
      const sub = await this.client.eventsSubscribe((ev) => {
        if (this.eventStream !== stream) return;
        if (!stream.ready) {
          stream.pending.push(ev);
          return;
        }
        if (ev.type === "disconnected") this.eventStream = null;
        this.handleCoreEvent(ev);
      });
      if (this.eventStream !== stream) {
        void this.client
          .eventsUnsubscribe(sub.subscriptionId)
          .catch((err) => this.reportError("eventsUnsubscribe", err));
        return;
      }
      stream.subscriptionId = sub.subscriptionId;
      this.store.getState().applyCoreSnapshot(sub.terminals, sub.seq);
      stream.ready = true;
      for (const ev of stream.pending.splice(0)) {
        if (this.eventStream !== stream) break;
        if (ev.type === "disconnected") this.eventStream = null;
        this.handleCoreEvent(ev);
      }
      this.logger.info("terminal", "Subscribed to herdr-core event stream", {
        subscriptionId: sub.subscriptionId,
        seq: sub.seq,
        liveCount: sub.terminals.length,
      });
      this.diagnostics?.pushLog(
        "system",
        `events subscribed (sub=${sub.subscriptionId}, seq=${sub.seq}, live=${sub.terminals.length})`,
      );
    } catch (err) {
      if (this.eventStream === stream) this.eventStream = null;
      const appErr = this.reportError("eventsSubscribe", err);
      this.store.getState().setCoreConnection({ status: "disconnected", lastError: appErr });
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
        .catch((err) => this.reportError("eventsUnsubscribe", err));
    }
  }

  private handleCoreEvent(ev: CoreEvent): void {
    this.store.getState().applyCoreEvent(ev);

    if (ev.type === "status") {
      const parsed = parseStatusEventData(ev.data);
      const state = parsed?.status?.state ?? "clear";
      const app = parsed?.status?.app ? ` app=${parsed.status.app}` : "";
      this.diagnostics?.pushLog("system", `[${ev.terminal}] OSC 7501 status=${state}${app}`);
    } else if (ev.type === "turn") {
      const parsed = parseTurnEventData(ev.data);
      if (parsed) {
        this.diagnostics?.pushLog("system", `[${ev.terminal}] turn=${parsed.turn} (${parsed.state})`);
      }
    } else if (ev.type === "finished") {
      const parsed = parseTerminationEventData(ev.data);
      this.logger.info("terminal", "Terminal session finished", {
        sessionId: ev.terminal,
        reason: parsed.reason,
        exitCode: parsed.exitCode,
      });
      this.diagnostics?.pushLog(
        "system",
        `[${ev.terminal}] finished (reason=${parsed.reason ?? "-"}, exit=${parsed.exitCode ?? "-"})`,
      );
    } else if (ev.type === "disconnected") {
      this.logger.warn("terminal", "Disconnected from herdr-core", { reason: ev.reason });
      this.diagnostics?.pushLog("system", `herdr-core disconnected: ${ev.reason}`);
    } else if (ev.type === "missed") {
      this.logger.warn("terminal", "Core event stream missed events; snapshot applied", {
        seq: ev.seq,
        snapshotCount: ev.terminals.length,
      });
      this.diagnostics?.pushLog(
        "system",
        `events missed (seq=${ev.seq}, snapshot=${ev.terminals.length})`,
      );
    }
  }

  private flushPostAttach(session: ManagedSession): void {
    if (session.disposed || session.attachId <= 0) return;

    if (session.unackedBytes >= ACK_BATCH_THRESHOLD) {
      const ack = session.unackedBytes;
      session.unackedBytes = 0;
      void this.client
        .terminalAck(session.attachId, ack)
        .catch((err) => this.reportError(`terminalAck:${session.id}`, err));
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
          this.reportError(`terminalResize:${session.id}`, err);
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
          this.reportError(`terminalWrite:${session.id}`, err);
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
      this.store.getState().updatePaneSize(session.id, frame.cols, frame.rows);
      return;
    }

    if (frame.kind === "close") {
      this.closeStream(session);
      this.logger.info("terminal", "Terminal attachment closed", {
        paneId: session.id,
        sessionId: session.sessionId,
        reason: frame.reason,
        exitCode: frame.exitCode,
      });
      this.diagnostics?.pushLog(
        "system",
        `[${session.id}] closed (${frame.reason}, exit=${frame.exitCode ?? "-"})`,
      );
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
        .catch((err) => this.reportError(`terminalAck:${session.id}`, err));
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
    if (!session?.stream || session.disposed || session.stream.closed || bytes.byteLength === 0) {
      return;
    }
    if (
      bytes.byteLength > MAX_PENDING_WRITE_BYTES - session.pendingWriteBytes ||
      session.pendingWrites.length >= MAX_PENDING_WRITES
    ) {
      this.reportError(
        `terminalWrite:${session.id}`,
        new AppError({
          code: "TERMINAL_QUEUE_FULL",
          severity: "warn",
          messageKey: "error.terminal.queueFull",
          devMessage: "pending input queue full",
          diagnostics: {
            paneId: session.id,
            pendingCount: session.pendingWrites.length,
            pendingBytes: session.pendingWriteBytes,
          },
        }),
      );
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
            this.reportError(`render:${session.id}`, err);
            if (attachId > 0) {
              void this.client
                .terminalDetach(attachId)
                .catch((detachError) => this.reportError(`terminalDetach:${session.id}`, detachError));
            }
          }
        },
      );
      if (session.disposed || stream.closed) {
        if (session.disposed || stream.failed) {
          await this.client
            .terminalDetach(attached.attachId)
            .catch((err) => this.reportError(`terminalDetach:${session.id}`, err));
        }
        return false;
      }
      session.attachId = attached.attachId;
      session.focused = false;
      this.syncFocus(session);
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
    createParams: TerminalCreateParams = {},
  ): Promise<ManagedSession> {
    if (this.creating.has(id) || this.sessions.has(id)) {
      throw new AppError({
        code: "TERMINAL_CREATE_FAILED",
        messageKey: "error.terminal.createFailed",
        devMessage: `session already exists: ${id}`,
        diagnostics: { paneId: id },
      });
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
      await this.startEventStream();
      creation.signal.throwIfAborted();
      termInfo = await this.client.terminalCreate({
        ...createParams,
        name: createParams.name ?? id,
        cols: createParams.cols ?? 80,
        rows: createParams.rows ?? 24,
      });
      creation.signal.throwIfAborted();

      const settings = this.store.getState().settings;
      const initialPaused = hidden && settings.pauseHiddenRendering;
      adapter = this.adapterFactory(
        backendId,
        {
          isComposing: () => this.store.getState().ui.isComposing,
          onUserInput: (bytes) => {
            // Never log raw user input text; diagnostics only record byte count when explicitly enabled.
            if (this.diagnostics?.isEnabled()) {
              this.diagnostics.pushLog("input", `[${id}] user (${bytes.byteLength}B)`);
            }
            this.sendInput(sessionRef, bytes, "user");
          },
          onAutoResponse: (bytes) => {
            const suppress = this.store.getState().settings.suppressAutoReplies;
            if (this.diagnostics?.isEnabled()) {
              this.diagnostics.pushLog(
                "auto-reply",
                `[${id}] ${suppress ? "BLOCKED" : "AUTO(kind=6)"} (${bytes.byteLength}B)`,
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
            this.store.getState().updatePaneSize(id, cols, rows);
            if (!sessionRef || sessionRef.disposed || sessionRef.stream?.closed) return;
            if (sessionRef.applyingBackendResize) return;
            sessionRef.pendingResize = { cols, rows };
            void this.flushResize(sessionRef);
          },
        },
        {
          renderingPaused: initialPaused,
          scrollbackLines: settings.scrollbackLines,
          fontSize: settings.fontSize,
          lineHeight: settings.lineHeight,
          fontFamily: settings.fontFamily,
          nerdFontFallback: settings.nerdFontFallback,
          cursorStyle: settings.cursorStyle,
          cursorBlink: settings.cursorBlink,
          macOptionAsMeta: settings.macOptionAsMeta,
        },
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
        focused: false,
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
      // Re-apply current settings immediately on registration so font/zoom/cursor
      // updates made while awaiting `adapter.mount()` are not missed.
      this.applySettingsToSession(session, this.store.getState().settings);

      if (initialPaused) {
        adapter.setHidden(true);
      }

      await this.attach(session);
      creation.signal.throwIfAborted();
      this.store.getState().bindPaneSession(id, termInfo.address);
      this.store.getState().updatePaneSize(id, adapter.cols, adapter.rows);
      return session;
    } catch (err) {
      this.reportError(`createSession:${id}`, err);
      if (!sessionRef?.disposed) {
        this.sessions.delete(id);
        if (sessionRef) sessionRef.disposed = true;
        try {
          adapter?.dispose();
        } catch (disposeError) {
          this.reportError(`disposeSession:${id}`, disposeError);
        }
        if (termInfo) {
          await this.client
            .terminalKill(termInfo.address)
            .catch((killError) => this.reportError(`terminalKill:${id}`, killError));
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

  isCreating(id: string): boolean {
    return this.creating.has(id);
  }

  private syncFocus(session: ManagedSession): void {
    const focused = this.windowFocused && session.id === this.activeId;
    if (session.attachId <= 0 || session.focused === focused) return;
    session.focused = focused;
    void this.client
      .terminalFocus(session.attachId, focused)
      .catch((err) => this.reportError(`terminalFocus:${session.id}`, err));
  }

  setActive(activeId: string | null, options: { focus?: boolean } = {}): void {
    const takeFocus = options.focus ?? true;
    const pauseHidden = this.store.getState().settings.pauseHiddenRendering;
    const prevActiveId = this.activeId;
    if (takeFocus) {
      this.activeId = activeId;
    }

    for (const [id, session] of this.sessions) {
      const isActive = id === activeId;
      const nextDisplay = isActive ? "block" : "none";
      if (session.container.style.display !== nextDisplay) {
        session.container.style.display = nextDisplay;
      }
      session.adapter.setHidden(pauseHidden && !isActive);
      if (takeFocus) {
        if (id === prevActiveId && !isActive) session.adapter.clearSelection?.();
        this.syncFocus(session);
        if (isActive && this.windowFocused && prevActiveId !== activeId) {
          session.adapter.fit();
          session.adapter.focus();
        }
      }
    }
  }

  focusSession(id: string): boolean {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return false;
    session.adapter.focus();
    return true;
  }

  pasteText(id: string, text: string): boolean {
    const session = this.sessions.get(id);
    if (!session?.stream || session.stream.closed || session.disposed || text.length === 0) return false;
    session.adapter.paste(text);
    return true;
  }

  async selectAll(id: string): Promise<boolean> {
    const session = this.sessions.get(id);
    if (!session || session.disposed || !session.adapter.selectAll) return false;
    return session.adapter.selectAll();
  }

  clearSelection(id: string): boolean {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return false;
    session.adapter.clearSelection?.();
    return true;
  }

  hasSelection(id: string): boolean {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return false;
    return session.adapter.hasSelection?.() ?? false;
  }

  getCellMetrics(id: string): { charWidth: number; rowHeight: number } | null {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return null;
    return session.adapter.getCellMetrics?.() ?? null;
  }

  private applySettingsToSession(session: ManagedSession, settings: UserSettings): void {
    if (session.disposed) return;
    session.adapter.applyAppearance?.({
      fontFamily: settings.fontFamily,
      fontSize: settings.fontSize,
      lineHeight: settings.lineHeight,
      nerdFontFallback: settings.nerdFontFallback,
      cursorStyle: settings.cursorStyle,
      cursorBlink: settings.cursorBlink,
      macOptionAsMeta: settings.macOptionAsMeta,
    });
  }

  applySettingsToAll(settings: UserSettings): void {
    for (const session of this.sessions.values()) {
      this.applySettingsToSession(session, settings);
    }
  }

  setWindowFocused(focused: boolean): void {
    this.windowFocused = focused;
    const session = this.activeId ? this.sessions.get(this.activeId) : undefined;
    if (!session || session.disposed) return;
    this.syncFocus(session);
    if (focused) session.adapter.focus();
  }

  hideSession(id: string): void {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return;
    const pauseHidden = this.store.getState().settings.pauseHiddenRendering;
    session.container.style.display = "none";
    session.adapter.setHidden(pauseHidden);
    if (session.focused && session.attachId > 0) {
      session.focused = false;
      void this.client
        .terminalFocus(session.attachId, false)
        .catch((err) => this.reportError(`terminalFocus:${id}`, err));
    } else {
      session.focused = false;
    }
    if (this.activeId === id) {
      this.activeId = null;
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
    this.store.getState().setCoreConnection({ status: "reconnecting" });
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
      const coreStatus = await this.client.coreReconnect();
      this.store.getState().setCoreConnection({ status: "connected", coreStatus, lastError: null });
      this.stopEventStream();
      await this.startEventStream();
      if (!session.disposed && (await this.attach(session))) {
        this.logger.info("terminal", "Terminal session resynced", {
          paneId: id,
          attachId: session.attachId,
        });
        this.diagnostics?.pushLog("system", `[${id}] resynced (attachId=${session.attachId})`);
      }
    } catch (err) {
      const appErr = this.reportError(`resyncSession:${id}`, err);
      this.store.getState().setCoreConnection({ status: "disconnected", lastError: appErr });
      throw appErr;
    } finally {
      session.resyncing = false;
    }
  }

  /** PoC diagnostic probe helpers (bypass PTY echo to test VT/IME/rendering directly). */
  injectJapaneseTest(id: string): void {
    this.injectRawPayload(id, JAPANESE_TEST_PAYLOAD);
  }

  injectVtTest(id: string): void {
    this.injectRawPayload(id, VT_ANSI_TEST_PAYLOAD);
  }

  injectQueryProbe(id: string): void {
    this.injectRawPayload(id, VT_QUERY_TEST_PAYLOAD);
  }

  private injectRawPayload(id: string, payload: string): void {
    const session = this.sessions.get(id);
    if (!session || session.disposed) return;
    const bytes = encoder.encode(payload);
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
        .catch((err) => this.reportError(`terminalDetach:${id}`, err));
    }
    void this.client
      .terminalKill(session.sessionId)
      .catch((err) => this.reportError(`terminalKill:${id}`, err));
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
