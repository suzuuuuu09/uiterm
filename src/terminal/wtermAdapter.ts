import { WTerm } from "@wterm/dom";
import { GhosttyCore } from "@wterm/ghostty";
import ghosttyWasmUrl from "@wterm/ghostty/ghostty-vt.wasm?url";
import type { TerminalBackendId } from "../transport/schemas.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

const encoder = new TextEncoder();

export class WTermGhosttyAdapter implements TerminalAdapter {
  readonly backendId: TerminalBackendId;
  private readonly rendererMode: "dom" | "webgl";
  private readonly callbacks: TerminalAdapterCallbacks;
  private readonly options: TerminalAdapterOptions;
  private term: WTerm | null = null;
  private hostEl: HTMLDivElement | null = null;
  private isWriting = false;
  private disposed = false;

  constructor(
    rendererMode: "dom" | "webgl",
    callbacks: TerminalAdapterCallbacks,
    options: TerminalAdapterOptions = {},
  ) {
    this.rendererMode = rendererMode;
    this.backendId = rendererMode === "webgl" ? "wterm-ghostty-webgl" : "wterm-ghostty-dom";
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
    host.className = "wterm-host h-full w-full overflow-hidden";
    if (this.options.fontSize) {
      host.style.fontSize = `${this.options.fontSize}px`;
    }
    if (this.options.fontFamily) {
      host.style.fontFamily = this.options.fontFamily;
    }
    container.appendChild(host);
    this.hostEl = host;

    const core = await GhosttyCore.load({
      wasmPath: ghosttyWasmUrl,
      scrollbackLines: this.options.scrollbackLines ?? 5000,
    });

    if (this.disposed) {
      core.dispose();
      host.remove();
      return;
    }

    const term = new WTerm(host, {
      core,
      renderer: this.rendererMode,
      autoResize: true,
      scrollbackLines: this.options.scrollbackLines ?? 5000,
      onData: (data: string) => {
        const bytes = encoder.encode(data);
        // WTerm drains core.getResponse() synchronously inside write().
        // Separate terminal auto-responses from user input for dual-VT safety.
        if (this.isWriting) {
          this.callbacks.onAutoResponse(bytes);
        } else {
          this.callbacks.onUserInput(bytes);
        }
      },
      onBinary: (bytes: Uint8Array) => {
        this.callbacks.onUserInput(bytes);
      },
      onResize: (cols: number, rows: number) => {
        this.callbacks.onResize(cols, rows);
      },
    });

    await term.init();
    if (this.disposed) {
      term.destroy();
      host.remove();
      return;
    }

    this.term = term;
    term.fit();
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
    this.term?.fit();
  }

  focus(): void {
    this.term?.focus();
  }

  setHidden(hidden: boolean): void {
    // Native WTerm API: suspends DOM/WebGL painting while continuing VT parsing
    this.term?.setRenderingPaused(hidden);
  }

  async getSelection(): Promise<string> {
    return this.term?.getSelectionText() ?? window.getSelection()?.toString() ?? "";
  }

  dispose(): void {
    this.disposed = true;
    this.term?.destroy();
    this.term = null;
    this.hostEl?.remove();
    this.hostEl = null;
  }
}
