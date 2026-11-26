import type {
  AttachId,
  ConfigEvalParams,
  ConfigEvalResult,
  CoreEvent,
  CoreStatus,
  DisplayFacts,
  EventsSubscribeResult,
  InputKind,
  OutputFrame,
  SessionId,
  TerminalAttachParams,
  TerminalAttachResult,
  TerminalCreateParams,
  TerminalInfo,
  UitermBackendClient,
} from "./types";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * In-memory mock implementation of UitermBackendClient.
 * Allows frontend PoC work (wterm/libghostty, IME, resize, multi-terminal)
 * to run in a standard browser via `pnpm dev` without Tauri or herdr-core.
 */
export class MockIpcClient implements UitermBackendClient {
  private nextSession = 1;
  private nextAttach = 1;
  private nextSub = 1;
  private seq = 0;
  private sessions = new Map<SessionId, TerminalInfo>();
  private attachments = new Map<
    AttachId,
    { sessionId: SessionId; onFrame: (frame: OutputFrame) => void }
  >();
  private subscribers = new Map<number, (event: CoreEvent) => void>();

  /** Recorded inputs for automated testing of user vs automatic input separation. */
  public readonly recordedInputs: Array<{
    attachId: AttachId;
    kind: InputKind;
    bytes: Uint8Array;
  }> = [];

  async coreStatus(): Promise<CoreStatus> {
    return {
      version: "0.1.22-mock",
      pid: 0,
      uptimeMs: Math.floor(performance.now()),
      live: [...this.sessions.values()].filter((s) => s.state === "live").length,
      finished: [...this.sessions.values()].filter((s) => s.state !== "live")
        .length,
      session: "mock",
      socketPath: "/tmp/uiterm-mock.sock",
      automaticInputSupported: true,
      resizeNotificationsSupported: true,
    };
  }

  async coreReconnect(): Promise<CoreStatus> {
    return this.coreStatus();
  }

  async terminalCreate(params: TerminalCreateParams = {}): Promise<TerminalInfo> {
    const address = `t${this.nextSession++}`;
    const info: TerminalInfo = {
      address,
      name: params.name ?? null,
      argv: params.argv ?? ["/bin/sh"],
      cwd: params.cwd ?? "/tmp",
      state: "live",
      cols: params.cols ?? 80,
      rows: params.rows ?? 24,
      title: params.name ?? address,
      pid: 10000 + this.nextSession,
      createdMs: Date.now(),
      finishedMs: null,
      exitCode: null,
      signal: null,
      reason: null,
      status: null,
      statusMs: null,
      turn: 0,
      labels: params.labels ?? {},
      foreground: null,
    };
    this.sessions.set(address, info);
    this.emit({ type: "created", seq: ++this.seq, terminal: address, data: info });
    return info;
  }

  async terminalList(all = false): Promise<TerminalInfo[]> {
    const items = [...this.sessions.values()];
    return all ? items : items.filter((t) => t.state === "live");
  }

  async terminalAttach(
    params: TerminalAttachParams,
    onFrame: (frame: OutputFrame) => void,
  ): Promise<TerminalAttachResult> {
    const info = this.sessions.get(params.sessionId);
    if (!info) {
      throw new Error(`Session not found: ${params.sessionId}`);
    }
    info.cols = params.cols;
    info.rows = params.rows;
    const attachId = this.nextAttach++;
    this.attachments.set(attachId, { sessionId: params.sessionId, onFrame });

    queueMicrotask(() => {
      if (params.resizeNotifications) {
        onFrame({ kind: "resize", cols: info.cols, rows: info.rows });
      }
      onFrame({
        kind: "data",
        bytes: encoder.encode(
          `\x1b[2J\x1b[H[mock:${info.address}] uiterm Phase 0 Mock Terminal (${info.cols}x${info.rows})\r\n日本語テスト: あいうえお 漢字 🍣\r\n$ `,
        ),
      });
    });

    return {
      attachId,
      terminal: { ...info },
      automaticInput: true,
      resizeNotifications: Boolean(params.resizeNotifications),
      initialCreditBytes: 256 * 1024,
    };
  }

  async terminalDetach(attachId: AttachId): Promise<void> {
    const entry = this.attachments.get(attachId);
    if (!entry) return;
    this.attachments.delete(attachId);
    entry.onFrame({
      kind: "close",
      reason: "detached",
      exitCode: null,
      signal: null,
    });
  }

  async terminalWrite(
    attachId: AttachId,
    data: Uint8Array | string,
    kind: InputKind = "user",
  ): Promise<void> {
    const entry = this.attachments.get(attachId);
    if (!entry) {
      throw new Error(`Attachment not found: ${attachId}`);
    }
    const bytes = typeof data === "string" ? encoder.encode(data) : data;
    this.recordedInputs.push({ attachId, kind, bytes: new Uint8Array(bytes) });

    // Only echo user input in mock mode; automatic VT replies are silently accepted.
    if (kind === "user") {
      const text = decoder.decode(bytes);
      const echoed = text.replace(/\r/g, "\r\n$ ");
      entry.onFrame({ kind: "data", bytes: encoder.encode(echoed) });
    }
  }

  async terminalResize(
    attachId: AttachId,
    cols: number,
    rows: number,
  ): Promise<void> {
    const entry = this.attachments.get(attachId);
    if (!entry) return;
    const info = this.sessions.get(entry.sessionId);
    if (info) {
      info.cols = cols;
      info.rows = rows;
      this.emit({
        type: "size",
        seq: ++this.seq,
        terminal: info.address,
        data: { cols, rows },
      });
    }
  }

  async terminalFocus(_attachId: AttachId, _focused: boolean): Promise<void> {}

  async terminalFacts(
    _attachId: AttachId,
    _display: DisplayFacts,
  ): Promise<void> {}

  async terminalAck(_attachId: AttachId, _bytes: number): Promise<void> {}

  async terminalKill(sessionId: SessionId): Promise<void> {
    const info = this.sessions.get(sessionId);
    if (!info) return;
    info.state = "finished";
    info.reason = "killed";
    info.finishedMs = Date.now();
    for (const [attachId, entry] of this.attachments.entries()) {
      if (entry.sessionId === sessionId) {
        this.attachments.delete(attachId);
        entry.onFrame({
          kind: "close",
          reason: "killed",
          exitCode: null,
          signal: "SIGKILL",
        });
      }
    }
    this.emit({
      type: "finished",
      seq: ++this.seq,
      terminal: sessionId,
      data: {
        reason: "killed",
        exitCode: null,
        signal: "SIGKILL",
        finishedMs: info.finishedMs,
      },
    });
  }

  async eventsSubscribe(
    onEvent: (event: CoreEvent) => void,
  ): Promise<EventsSubscribeResult> {
    const subscriptionId = this.nextSub++;
    this.subscribers.set(subscriptionId, onEvent);
    return {
      subscriptionId,
      seq: this.seq,
      terminals: [...this.sessions.values()],
    };
  }

  async eventsUnsubscribe(subscriptionId: number): Promise<void> {
    this.subscribers.delete(subscriptionId);
  }

  async configEvalPoc(params: ConfigEvalParams): Promise<ConfigEvalResult> {
    return {
      config: { mock: true, configPath: params.configPath },
      actionResult: params.actionName ? { invoked: params.actionName } : null,
      hookResult: params.eventName ? { handled: params.eventName } : null,
      startupMs: 0,
      rpcMs: 0,
      workerRssBytes: null,
    };
  }

  /** Injects a synthetic CoreEvent to all active subscribers (for integration testing). */
  emitTestEvent(event: CoreEvent): void {
    if ("seq" in event && event.seq > this.seq) {
      this.seq = event.seq;
    }
    this.emit(event);
  }

  private emit(event: CoreEvent): void {
    for (const sub of this.subscribers.values()) {
      sub(event);
    }
  }
}
