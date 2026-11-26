import { WTerm } from "@wterm/dom";
import { GhosttyCore } from "@wterm/ghostty";
import ghosttyWasmUrl from "@wterm/ghostty/ghostty-vt.wasm?url";
import type { TerminalBackendId } from "../validation/settingsSchemas.ts";
import {
  applyAppearanceToHost,
  DEFAULT_TERMINAL_APPEARANCE,
  type TerminalAppearance,
  toWtermCursorBlink,
  toWtermThemeColors,
} from "./appearance.ts";
import type { CellMetrics } from "./fonts.ts";
import { isImeComposingEvent, isMacPlatform, translateMacOptionKey } from "./keyboard.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";

const encoder = new TextEncoder();

/**
 * Default frontend scrollback buffer size.
 * herdr-core maintains authoritative backend history; keeping frontend scrollback
 * at 2,000 lines reduces per-terminal WASM peak memory by ~44% vs 5,000 lines
 * (10-terminal RSS +89.6 MB -> +49.8 MB).
 */
export const DEFAULT_SCROLLBACK_LINES = 2000;

export class WTermGhosttyAdapter implements TerminalAdapter {
  readonly backendId: TerminalBackendId;
  private readonly rendererMode: "dom" | "webgl";
  private readonly callbacks: TerminalAdapterCallbacks;
  private readonly options: TerminalAdapterOptions;
  private appearance: TerminalAppearance;
  private macOptionAsMeta: boolean;
  private readonly isMac: boolean;
  private core: GhosttyCore | null = null;
  private term: WTerm | null = null;
  private hostEl: HTMLDivElement | null = null;
  private onKeyDown: ((e: KeyboardEvent) => void) | null = null;
  private isWriting = false;
  private isHidden = false;
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
    this.isMac = isMacPlatform();
    this.macOptionAsMeta = options.macOptionAsMeta ?? true;
    this.isHidden = options.renderingPaused ?? false;
    this.appearance = {
      fontFamily: options.fontFamily ?? DEFAULT_TERMINAL_APPEARANCE.fontFamily,
      fontSize: options.fontSize ?? DEFAULT_TERMINAL_APPEARANCE.fontSize,
      lineHeight: options.lineHeight ?? DEFAULT_TERMINAL_APPEARANCE.lineHeight,
      nerdFontFallback: options.nerdFontFallback ?? DEFAULT_TERMINAL_APPEARANCE.nerdFontFallback,
      cursorStyle: options.cursorStyle ?? DEFAULT_TERMINAL_APPEARANCE.cursorStyle,
      cursorBlink: options.cursorBlink ?? DEFAULT_TERMINAL_APPEARANCE.cursorBlink,
      theme: options.theme ?? DEFAULT_TERMINAL_APPEARANCE.theme,
    };
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
    applyAppearanceToHost(host, this.appearance);
    container.appendChild(host);
    this.hostEl = host;

    const scrollbackLines = this.options.scrollbackLines ?? DEFAULT_SCROLLBACK_LINES;
    const core = await GhosttyCore.load({
      wasmPath: ghosttyWasmUrl,
      scrollbackLines,
      foregroundColor: this.appearance.theme.foreground,
      backgroundColor: this.appearance.theme.background,
    });

    if (this.disposed) {
      core.dispose();
      host.remove();
      return;
    }

    // herdr-core sends PTY focus reports from FOCUS frames.
    core.focusEvents = () => false;
    const readCursor = core.getCursor.bind(core);
    core.getCursor = () => {
      const cursor = readCursor();
      if (this.appearance.cursorStyle !== "auto") cursor.shape = this.appearance.cursorStyle;
      return cursor;
    };
    this.core = core;

    const term = new WTerm(host, {
      core,
      renderer: this.rendererMode,
      autoResize: true,
      renderingPaused: this.options.renderingPaused ?? false,
      scrollbackLines,
      cursorBlink: toWtermCursorBlink(this.appearance.cursorBlink),
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

    try {
      await term.init();
    } catch (err) {
      core.dispose();
      this.core = null;
      host.remove();
      this.hostEl = null;
      throw err;
    }

    if (this.disposed) {
      term.destroy();
      core.dispose();
      this.core = null;
      host.remove();
      return;
    }

    this.term = term;

    // Sync explicit theme colors so OSC 10/11/12/4 queries and WebGL match CSS custom properties.
    this.syncTheme();

    // Translate macOS Option+<key> into ESC+<ascii> Meta sequences before wterm sees
    // macOS's composed characters (e.g. Option+b -> "∫" instead of "\x1bb").
    this.onKeyDown = (event: KeyboardEvent) => {
      const isComposing = this.callbacks.isComposing?.() ?? false;
      if (isImeComposingEvent(event, isComposing)) return;
      if (event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey &&
          event.key.toLowerCase() === "c") {
        // wterm leaves Ctrl+C to the browser when a selection is active.
        term.clearSelection();
      }
      const seq = translateMacOptionKey(event, {
        isMac: this.isMac,
        macOptionAsMeta: this.macOptionAsMeta,
        isComposing,
        kittyKeyboardFlags: this.core?.kittyKeyboardFlags?.() ?? 0,
      });
      if (seq === null) return;
      event.preventDefault();
      event.stopPropagation();
      this.term?.clearSelection();
      this.callbacks.onUserInput(encoder.encode(seq));
    };
    host.addEventListener("keydown", this.onKeyDown, true);

    if (!this.options.renderingPaused) {
      term.fit();
    }

    // Refit once when local/system fonts finish loading so cell metrics settle accurately.
    if (typeof document !== "undefined" && "fonts" in document && document.fonts?.ready) {
      void document.fonts.ready.then(() => {
        if (!this.disposed && !this.isHidden && this.term) {
          this.term.fit();
        }
      });
    }
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
    this.isHidden = hidden;
    // Native WTerm API: suspends DOM/WebGL painting while continuing VT parsing
    this.term?.setRenderingPaused(hidden);
  }

  async getSelection(): Promise<string> {
    return this.term?.getSelectionText() ?? "";
  }

  hasSelection(): boolean {
    return Boolean(this.term?.getSelectionText());
  }

  async selectAll(): Promise<boolean> {
    if (!this.term || this.disposed) return false;
    return this.term.selectAll();
  }

  clearSelection(): void {
    this.term?.clearSelection();
  }

  paste(text: string): void {
    if (this.disposed || text.length === 0) return;
    this.term?.clearSelection();
    if (this.core?.bracketedPaste()) {
      // Strip ESC bytes to prevent bracketed paste escape breakout (\x1b[201~).
      const safe = text.replaceAll("\x1b", "");
      this.callbacks.onUserInput(encoder.encode(`\x1b[200~${safe}\x1b[201~`));
    } else {
      this.callbacks.onUserInput(encoder.encode(text));
    }
  }

  applyAppearance(patch: Partial<TerminalAppearance> & { macOptionAsMeta?: boolean }): void {
    const previousCursorStyle = this.appearance.cursorStyle;
    const prevFontKey = `${this.appearance.fontFamily}|${this.appearance.fontSize}|${this.appearance.lineHeight}|${this.appearance.nerdFontFallback}`;
    if (patch.macOptionAsMeta !== undefined) {
      this.macOptionAsMeta = patch.macOptionAsMeta;
    }
    this.appearance = {
      fontFamily: patch.fontFamily ?? this.appearance.fontFamily,
      fontSize: patch.fontSize ?? this.appearance.fontSize,
      lineHeight: patch.lineHeight ?? this.appearance.lineHeight,
      nerdFontFallback: patch.nerdFontFallback ?? this.appearance.nerdFontFallback,
      cursorStyle: patch.cursorStyle ?? this.appearance.cursorStyle,
      cursorBlink: patch.cursorBlink ?? this.appearance.cursorBlink,
      theme: patch.theme ?? this.appearance.theme,
    };

    if (this.hostEl) {
      applyAppearanceToHost(this.hostEl, this.appearance);
    }
    // setThemeColors also invalidates the painted frame when only the cursor shape changes.
    if (this.term && (patch.theme || previousCursorStyle !== this.appearance.cursorStyle)) {
      this.syncTheme();
    }
    const nextFontKey = `${this.appearance.fontFamily}|${this.appearance.fontSize}|${this.appearance.lineHeight}|${this.appearance.nerdFontFallback}`;
    if (nextFontKey !== prevFontKey && !this.isHidden && this.term) {
      this.term.fit();
    }
  }

  getCellMetrics(): CellMetrics | null {
    if (!this.hostEl) return null;
    const style = getComputedStyle(this.hostEl);
    const charWidth = Number.parseFloat(style.getPropertyValue("--term-cell-width"));
    const rowHeight = Number.parseFloat(style.getPropertyValue("--term-row-height"));
    if (
      !Number.isFinite(charWidth) ||
      !Number.isFinite(rowHeight) ||
      charWidth <= 0 ||
      rowHeight <= 0
    ) {
      return null;
    }
    return { charWidth, rowHeight };
  }

  private syncTheme(): void {
    const colors = toWtermThemeColors(this.appearance.theme);
    if (!colors) return;
    this.isWriting = true;
    try {
      this.term?.setThemeColors(colors);
    } finally {
      this.isWriting = false;
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.hostEl && this.onKeyDown) {
      this.hostEl.removeEventListener("keydown", this.onKeyDown, true);
      this.onKeyDown = null;
    }
    this.term?.destroy();
    this.term = null;
    // WTerm.destroy() does not dispose an externally supplied core;
    // explicitly deinit the Zig/WASM terminal and free transfer buffers.
    this.core?.dispose();
    this.core = null;
    this.hostEl?.remove();
    this.hostEl = null;
  }
}
