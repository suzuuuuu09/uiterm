/**
 * uiterm Phase 0 IPC type definitions.
 * Shared contract between Rust backend (Tauri 2) and TypeScript frontend.
 * See docs/poc/interface.md for the full protocol specification.
 */

/** Immutable herdr-core terminal identifier (e.g. "t1", "box/t7"). */
export type SessionId = string;

/** Framed-mode stream channel ID returned by terminal_attach. */
export type AttachId = number;

/**
 * Input classification for terminal_write.
 * - "user": human keyboard / paste input -> herdr-core DATA (kind 0)
 * - "automatic": VT/terminal auto-reply (DA1/DA2/OSC color/mode 2048/XTGETTCAP/etc.)
 *   -> herdr-core AUTOMATIC_INPUT (kind 6) when negotiated, else DATA (kind 0).
 */
export type InputKind = "user" | "automatic";

/** Binary frame type prefix byte on the terminal_attach Tauri Channel. */
export const OUTPUT_FRAME_DATA = 0x00;
export const OUTPUT_FRAME_RESIZE = 0x01;
export const OUTPUT_FRAME_CLOSE = 0x02;

export type OutputFrame =
  | { kind: "data"; bytes: Uint8Array }
  | { kind: "resize"; cols: number; rows: number }
  | {
      kind: "close";
      reason: string;
      exitCode: number | null;
      signal: string | null;
    };

export interface DisplayFacts {
  fg?: string | null;
  bg?: string | null;
  cursor?: string | null;
  cell?: [number, number] | null;
}

export interface CoreStatus {
  version: string;
  pid: number;
  uptimeMs: number;
  live: number;
  finished: number;
  session: string;
  socketPath: string;
  automaticInputSupported: boolean;
  resizeNotificationsSupported: boolean;
}

export interface ForegroundProcess {
  pid: number;
  name: string;
  argv: string[];
  cwd: string;
}

export interface StatusRecord {
  state: string;
  kind?: string | null;
  progress?: number | null;
  app?: string;
  title?: string;
  msg?: string;
}

export interface TerminalInfo {
  address: SessionId;
  name: string | null;
  argv: string[];
  cwd: string;
  state: "live" | "finished" | string;
  cols: number;
  rows: number;
  title: string;
  pid: number | null;
  createdMs: number;
  finishedMs: number | null;
  exitCode: number | null;
  signal: string | null;
  reason: string | null;
  status: StatusRecord | null;
  statusMs: number | null;
  turn: number;
  labels: Record<string, string>;
  foreground: ForegroundProcess | null;
}

export interface TerminalCreateParams {
  argv?: string[];
  line?: string;
  cwd?: string;
  env?: Record<string, string>;
  name?: string;
  labels?: Record<string, string>;
  cols?: number;
  rows?: number;
  display?: DisplayFacts;
  keep?: boolean;
}

export interface TerminalAttachParams {
  sessionId: SessionId;
  cols: number;
  rows: number;
  history?: number;
  observe?: boolean;
  resizeNotifications?: boolean;
  display?: DisplayFacts;
}

export interface TerminalAttachResult {
  attachId: AttachId;
  terminal: TerminalInfo;
  automaticInput: boolean;
  resizeNotifications: boolean;
  initialCreditBytes: number;
}

export interface EventsSubscribeResult {
  subscriptionId: number;
  seq: number;
  terminals: TerminalInfo[];
}

export type CoreEvent =
  | { type: "created"; seq: number; terminal: SessionId; data: TerminalInfo }
  | {
      type: "lost";
      seq: number;
      terminal: SessionId;
      data: {
        reason: string | null;
        exitCode: number | null;
        signal: string | null;
      };
    }
  | { type: "restarted"; seq: number; terminal: SessionId; data: TerminalInfo }
  | {
      type: "finished";
      seq: number;
      terminal: SessionId;
      data: {
        reason: string | null;
        exitCode: number | null;
        signal: string | null;
        finishedMs: number | null;
      };
    }
  | { type: "removed"; seq: number; terminal: SessionId }
  | {
      type: "status";
      seq: number;
      terminal: SessionId;
      data: {
        status: StatusRecord | null;
        records: Record<string, StatusRecord>;
        reported: boolean;
      };
    }
  | {
      type: "turn";
      seq: number;
      terminal: SessionId;
      data: { turn: number; state: string };
    }
  | {
      type: "title";
      seq: number;
      terminal: SessionId;
      data: { title: string };
    }
  | {
      type: "size";
      seq: number;
      terminal: SessionId;
      data: { cols: number; rows: number };
    }
  | {
      type: "foreground";
      seq: number;
      terminal: SessionId;
      data: ForegroundProcess | null;
    }
  | {
      type: "labels";
      seq: number;
      terminal: SessionId;
      data: { labels: Record<string, string> };
    }
  | { type: "heartbeat"; seq: number }
  | { type: "missed"; seq: number; terminals: TerminalInfo[] }
  | { type: "disconnected"; reason: string };

export interface IpcError {
  code: string;
  message: string;
  hint?: string | null;
}

export interface ConfigEvalParams {
  configPath: string;
  actionName?: string;
  actionArgs?: unknown;
  eventName?: string;
  eventPayload?: unknown;
}

export interface ConfigEvalResult {
  config: unknown;
  actionResult: unknown;
  hookResult: unknown;
  startupMs: number;
  rpcMs: number;
  workerRssBytes: number | null;
}

/** Decode a raw binary frame received on the terminal_attach Tauri Channel. */
export function decodeOutputFrame(buffer: ArrayBuffer): OutputFrame {
  const view = new Uint8Array(buffer);
  if (view.byteLength === 0) {
    return { kind: "data", bytes: new Uint8Array(0) };
  }
  const tag = view[0];
  if (tag === OUTPUT_FRAME_DATA) {
    return { kind: "data", bytes: view.subarray(1) };
  }
  if (tag === OUTPUT_FRAME_RESIZE && view.byteLength >= 5) {
    const dv = new DataView(buffer, 1, 4);
    return {
      kind: "resize",
      cols: dv.getUint16(0, true),
      rows: dv.getUint16(2, true),
    };
  }
  if (tag === OUTPUT_FRAME_CLOSE) {
    const text = new TextDecoder().decode(view.subarray(1)).trim();
    if (!text) {
      return { kind: "close", reason: "detached", exitCode: null, signal: null };
    }
    try {
      const parsed = JSON.parse(text) as {
        reason?: string;
        exit_code?: number | null;
        signal?: string | null;
      };
      return {
        kind: "close",
        reason: parsed.reason ?? "closed",
        exitCode: parsed.exit_code ?? null,
        signal: parsed.signal ?? null,
      };
    } catch {
      return { kind: "close", reason: text, exitCode: null, signal: null };
    }
  }
  throw new Error(`Unknown output frame tag: 0x${tag.toString(16)}`);
}

/** Common backend interface implemented by both TauriIpcClient and MockIpcClient. */
export interface UitermBackendClient {
  coreStatus(): Promise<CoreStatus>;
  coreReconnect(): Promise<CoreStatus>;
  terminalCreate(params?: TerminalCreateParams): Promise<TerminalInfo>;
  terminalList(all?: boolean): Promise<TerminalInfo[]>;
  terminalAttach(
    params: TerminalAttachParams,
    onFrame: (frame: OutputFrame) => void,
  ): Promise<TerminalAttachResult>;
  terminalDetach(attachId: AttachId): Promise<void>;
  terminalWrite(
    attachId: AttachId,
    data: Uint8Array | string,
    kind?: InputKind,
  ): Promise<void>;
  terminalResize(attachId: AttachId, cols: number, rows: number): Promise<void>;
  terminalFocus(attachId: AttachId, focused: boolean): Promise<void>;
  terminalFacts(attachId: AttachId, display: DisplayFacts): Promise<void>;
  terminalAck(attachId: AttachId, bytes: number): Promise<void>;
  terminalKill(sessionId: SessionId): Promise<void>;
  eventsSubscribe(
    onEvent: (event: CoreEvent) => void,
  ): Promise<EventsSubscribeResult>;
  eventsUnsubscribe(subscriptionId: number): Promise<void>;
  configEvalPoc(params: ConfigEvalParams): Promise<ConfigEvalResult>;
}
