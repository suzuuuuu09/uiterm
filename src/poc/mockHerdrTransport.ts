import { validateResizePayload } from "../validation/ipcSchemas.ts";
import { JAPANESE_TEST_PAYLOAD, VT_ANSI_TEST_PAYLOAD, VT_QUERY_TEST_PAYLOAD } from "./probes.ts";

export interface TerminalOutputChunk {
  readonly kind: "initial_screen" | "stream" | "resync";
  readonly bytes: Uint8Array;
}

export interface TerminalTransport {
  readonly sessionId: string;
  subscribeOutput(listener: (chunk: TerminalOutputChunk) => void): () => void;
  attach(cols: number, rows: number): void;
  sendInput(bytes: Uint8Array): void;
  resize(cols: number, rows: number): void;
  requestResync(): void;
  dispose(): void;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * Phase 0 in-memory mock transport retained for regression testing and standalone benchmarks.
 */
export class MockHerdrTransport implements TerminalTransport {
  readonly sessionId: string;
  private listeners = new Set<(chunk: TerminalOutputChunk) => void>();
  private cols = 80;
  private rows = 24;
  private disposed = false;
  private lineBuffer = "";
  private totalInputBytes = 0;

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  subscribeOutput(listener: (chunk: TerminalOutputChunk) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private emit(kind: TerminalOutputChunk["kind"], textOrBytes: string | Uint8Array): void {
    if (this.disposed) return;
    const bytes = typeof textOrBytes === "string" ? encoder.encode(textOrBytes) : textOrBytes;
    const chunk: TerminalOutputChunk = { kind, bytes };
    for (const listener of this.listeners) {
      listener(chunk);
    }
  }

  attach(cols: number, rows: number): void {
    this.cols = cols;
    this.rows = rows;
    const snapshot = [
      `\x1b[2J\x1b[H`,
      `\x1b[1;34m[herdr-core mock]\x1b[0m session=\x1b[33m${this.sessionId}\x1b[0m grid=${cols}x${rows}\r\n`,
      `Initial screen snapshot delivered via binary Uint8Array stream.\r\n`,
      `日本語入力テスト / Type here (Enter echoes line; Ctrl+C resets).\r\n`,
      `\x1b[32muiterm-mock\x1b[0m$ `,
    ].join("");
    this.emit("initial_screen", snapshot);
  }

  sendInput(bytes: Uint8Array): void {
    if (this.disposed || bytes.byteLength === 0) return;
    this.totalInputBytes += bytes.byteLength;
    const text = decoder.decode(bytes);

    let out = "";
    for (const ch of text) {
      if (ch === "\r" || ch === "\n") {
        out += `\r\n\x1b[90m[echo ${this.lineBuffer.length} chars]\x1b[0m ${this.lineBuffer}\r\n\x1b[32muiterm-mock\x1b[0m$ `;
        this.lineBuffer = "";
      } else if (ch === "\x7f" || ch === "\b") {
        if (this.lineBuffer.length > 0) {
          const chars = Array.from(this.lineBuffer);
          const removed = chars.pop() ?? "";
          this.lineBuffer = chars.join("");
          const cp = removed.codePointAt(0) ?? 0;
          const isWide = cp > 0x1100;
          out += isWide ? "\b \b\b \b" : "\b \b";
        }
      } else if (ch === "\x03") {
        this.lineBuffer = "";
        out += "^C\r\n\x1b[32muiterm-mock\x1b[0m$ ";
      } else if (ch === "\x1b") {
        out += "^[";
      } else {
        this.lineBuffer += ch;
        out += ch;
      }
    }
    if (out.length > 0) {
      this.emit("stream", out);
    }
  }

  resize(cols: number, rows: number): void {
    const validated = validateResizePayload({ sessionId: this.sessionId, cols, rows });
    this.cols = validated.cols;
    this.rows = validated.rows;
  }

  requestResync(): void {
    const resyncSnapshot = [
      `\x1b[2J\x1b[H`,
      `\x1b[1;33m[herdr-core resync]\x1b[0m session=${this.sessionId} grid=${this.cols}x${this.rows} totalInputBytes=${this.totalInputBytes}\r\n`,
      `Full viewport reconstructed from backend VT state.\r\n`,
      `\x1b[32muiterm-mock\x1b[0m$ ${this.lineBuffer}`,
    ].join("");
    this.emit("resync", resyncSnapshot);
  }

  emitJapaneseTest(): void {
    this.emit("stream", JAPANESE_TEST_PAYLOAD);
  }

  emitVtTest(): void {
    this.emit("stream", VT_ANSI_TEST_PAYLOAD);
  }

  emitQueryProbe(): void {
    this.emit("stream", VT_QUERY_TEST_PAYLOAD);
  }

  emitBurst(lines = 10_000, chunkLines = 200): { totalBytes: number; durationMs: number } {
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
      this.emit("stream", bytes);
    }
    return { totalBytes, durationMs: performance.now() - start };
  }

  dispose(): void {
    this.disposed = true;
    this.listeners.clear();
  }
}
