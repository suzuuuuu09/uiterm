import { validateResizePayload } from "./schemas.ts";
import type { TerminalOutputChunk, TerminalTransport } from "./types.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export const JAPANESE_TEST_PAYLOAD = [
  "\r\n\x1b[1;36m=== 日本語・文字幅・フォントフォールバック検証 ===\x1b[0m\r\n",
  "ASCII 40 cols : 1234567890123456789012345678901234567890|\r\n",
  "全角 20文字   : あいうえおかきくけこさしすせそたちつてと|\r\n",
  "混在テキスト  : Hello日本語World1234漢字ABCあいうえお  |\r\n",
  "曖昧幅(East_A): ○●△▲□■☆★※→←↑↓─│┌┐└┘┼|\r\n",
  "半角カナ      : ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄ12345678901234567890|\r\n",
  "絵文字/ZWJ    : 🚀 🎉 👨‍💻 🇯🇵 👨‍👩‍👧‍👦 👍🏽 ✨\r\n",
  "結合文字(NFD) : か\u3099 き\u3099 く\u3099 e\u0301 a\u0308 (濁点結合・アクセント)\r\n",
  "罫線ボックス  : ┌────────────────────────┐\r\n",
  "              : │ 日本語テキスト整列確認 │\r\n",
  "              : └────────────────────────┘\r\n",
  "日本語IME入力テスト → ここに直接入力してください: ",
].join("");

export const VT_ANSI_TEST_PAYLOAD = [
  "\r\n\x1b[1;35m=== VT / ANSI 互換性テスト ===\x1b[0m\r\n",
  "SGR: \x1b[1mBold\x1b[0m \x1b[2mDim\x1b[0m \x1b[3mItalic\x1b[0m \x1b[4mUnderline\x1b[0m \x1b[9mStrike\x1b[0m \x1b[7mInverse\x1b[0m\r\n",
  "TrueColor: \x1b[38;2;255;100;100mR\x1b[38;2;100;255;100mG\x1b[38;2;100;160;255mB\x1b[0m ",
  "\x1b[48;2;30;60;120;38;2;255;255;255m 24-bit BG+FG \x1b[0m\r\n",
  "256-color: ",
  Array.from({ length: 16 }, (_, i) => `\x1b[48;5;${16 + i * 12}m  \x1b[0m`).join(""),
  "\r\n",
  "OSC 8 Link: \x1b]8;;https://ghostty.org\x07Ghostty\x1b]8;;\x07\r\n",
  "OSC 7501 (herdr): \x1b]7501;state=working:app=codex\x1b\\(status emitted)\r\n",
  "\x1b[32muiterm-mock\x1b[0m$ ",
].join("");

/**
 * VT query sequences that a PTY app (or initial screen / resync) may emit.
 * In a dual-VT architecture (herdr-core + frontend libghostty), frontend auto-responses
 * to these queries MUST NOT blindly be forwarded to PTY stdin as user input.
 */
export const VT_QUERY_TEST_PAYLOAD =
  "\r\n\x1b[33m[Auto-Reply Probe]\x1b[0m Sending DA1 (CSI c), DA2 (CSI > c), DSR (CSI 6 n), OSC 11 ? ...\r\n" +
  "\x1b[c\x1b[>c\x1b[6n\x1b]11;?\x07" +
  "\x1b[32muiterm-mock\x1b[0m$ ";

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
          // Simple width heuristic for mock echo
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

  /**
   * High-throughput binary burst generator for performance benchmarking.
   * Returns total bytes emitted and synchronous emission duration in ms.
   */
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
