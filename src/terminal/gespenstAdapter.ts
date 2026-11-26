import { createTerminal, type GespenstTerminal } from "@gespenst/core";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

export class GespenstAdapter implements TerminalAdapter {
  readonly backendId = "gespenst" as const;
  private readonly callbacks: TerminalAdapterCallbacks;
  private readonly options: TerminalAdapterOptions;
  private term: GespenstTerminal | null = null;
  private hostEl: HTMLDivElement | null = null;
  private disposed = false;

  constructor(callbacks: TerminalAdapterCallbacks, options: TerminalAdapterOptions = {}) {
    this.callbacks = callbacks;
    this.options = options;
  }

  get cols(): number {
    return this.term?.geometry.cols ?? 80;
  }

  get rows(): number {
    return this.term?.geometry.rows ?? 24;
  }

  async mount(container: HTMLElement): Promise<void> {
    const host = document.createElement("div");
    host.className = "h-full w-full overflow-hidden";
    container.appendChild(host);
    this.hostEl = host;

    const term = await createTerminal({
      container: host,
      worker: "dedicated",
      renderer: "auto",
      scrollbackLines: this.options.scrollbackLines ?? 5000,
      fontSizePx: this.options.fontSize ?? 14,
      fontFamily:
        this.options.fontFamily ??
        '"JetBrains Mono", "SF Mono", "Menlo", "Hiragino Sans", "Noto Sans Mono CJK JP", monospace',
    });

    if (this.disposed) {
      term.dispose();
      host.remove();
      return;
    }

    // Gespenst natively distinguishes source === 'reply' from user input!
    term.on("input", ({ data, source }) => {
      if (source === "reply") {
        this.callbacks.onAutoResponse(data);
      } else {
        this.callbacks.onUserInput(data);
      }
    });

    term.on("resize", ({ cols, rows }) => {
      this.callbacks.onResize(cols, rows);
    });

    this.term = term;
    term.fit();
  }

  write(bytes: Uint8Array): void {
    if (!this.term || this.disposed) return;
    this.term.write(bytes);
  }

  resize(cols: number, rows: number): void {
    this.term?.resize(cols, rows);
  }

  fit(): void {
    this.term?.fit();
  }

  focus(): void {
    this.term?.focus();
  }

  setHidden(_hidden: boolean): void {
    // Gespenst does not expose an explicit renderingPaused API in v0.2.0
  }

  async getSelection(): Promise<string> {
    if (!this.term) return "";
    return this.term.getSelection();
  }

  dispose(): void {
    this.disposed = true;
    this.term?.dispose();
    this.term = null;
    this.hostEl?.remove();
    this.hostEl = null;
  }
}
