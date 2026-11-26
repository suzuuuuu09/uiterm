import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

const encoder = new TextEncoder();

export class XtermAdapter implements TerminalAdapter {
  readonly backendId = "xterm" as const;
  private readonly callbacks: TerminalAdapterCallbacks;
  private readonly options: TerminalAdapterOptions;
  private term: Terminal | null = null;
  private fitAddon: FitAddon | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private hostEl: HTMLDivElement | null = null;
  private isWriting = false;
  private disposed = false;

  constructor(callbacks: TerminalAdapterCallbacks, options: TerminalAdapterOptions = {}) {
    this.callbacks = callbacks;
    this.options = options;
  }

  get cols(): number {
    return this.term?.cols ?? 80;
  }

  get rows(): number {
    return this.term?.rows ?? 24;
  }

  async mount(container: HTMLElement): Promise<void> {
    const host = document.createElement("div");
    host.className = "h-full w-full overflow-hidden";
    container.appendChild(host);
    this.hostEl = host;

    const term = new Terminal({
      scrollback: this.options.scrollbackLines ?? 5000,
      fontSize: this.options.fontSize ?? 14,
      fontFamily:
        this.options.fontFamily ??
        '"JetBrains Mono", "SF Mono", "Menlo", "Hiragino Sans", "Noto Sans Mono CJK JP", monospace',
      theme: {
        background: "#09090b",
        foreground: "#f4f4f5",
      },
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(host);

    term.onData((data: string) => {
      const bytes = encoder.encode(data);
      if (this.isWriting) {
        this.callbacks.onAutoResponse(bytes);
      } else {
        this.callbacks.onUserInput(bytes);
      }
    });

    term.onResize(({ cols, rows }) => {
      this.callbacks.onResize(cols, rows);
    });

    this.resizeObserver = new ResizeObserver(() => {
      if (!this.disposed && host.clientWidth > 0 && host.clientHeight > 0) {
        fitAddon.fit();
      }
    });
    this.resizeObserver.observe(host);

    this.term = term;
    this.fitAddon = fitAddon;
    fitAddon.fit();
  }

  write(bytes: Uint8Array): void {
    if (!this.term || this.disposed) return;
    this.isWriting = true;
    try {
      this.term.write(bytes);
    } finally {
      this.isWriting = false;
    }
  }

  resize(cols: number, rows: number): void {
    this.term?.resize(cols, rows);
  }

  fit(): void {
    if (this.hostEl && this.hostEl.clientWidth > 0 && this.hostEl.clientHeight > 0) {
      this.fitAddon?.fit();
    }
  }

  focus(): void {
    this.term?.focus();
  }

  setHidden(_hidden: boolean): void {
    // xterm.js relies on requestAnimationFrame; no explicit pause API
  }

  async getSelection(): Promise<string> {
    return this.term?.getSelection() ?? "";
  }

  dispose(): void {
    this.disposed = true;
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    this.fitAddon?.dispose();
    this.fitAddon = null;
    this.term?.dispose();
    this.term = null;
    this.hostEl?.remove();
    this.hostEl = null;
  }
}
