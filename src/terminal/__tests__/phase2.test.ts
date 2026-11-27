import { describe, expect, it, vi } from "vite-plus/test";
import { createAppRuntime } from "../../app/runtime.ts";
import { MockIpcClient } from "../../ipc/mock.ts";
import {
  applyAppearanceToHost,
  DEFAULT_TERMINAL_APPEARANCE,
  DEFAULT_TERMINAL_THEME,
  hexToRgbNumber,
  toWtermCursorBlink,
  toWtermThemeColors,
} from "../appearance.ts";
import {
  buildTerminalFontStack,
  clampFontSize,
  clampLineHeight,
  DEFAULT_FONT_SIZE,
  DEFAULT_LINE_HEIGHT,
  JAPANESE_FONT_FALLBACK_FAMILIES,
  NERD_FONT_FALLBACK_FAMILIES,
} from "../fonts.ts";
import {
  isImeComposingEvent,
  isMacPlatform,
  matchAppShortcut,
  translateMacOptionKey,
} from "../keyboard.ts";
import type { TerminalAdapter } from "../types.ts";

function createFakeHost() {
  const styles = new Map<string, string>();
  const classes = new Set<string>();
  const dataset: Record<string, string> = {};
  const styleObj = {
    fontFamily: "",
    fontSize: "",
    lineHeight: "",
    display: "",
    setProperty: (k: string, val: string) => {
      styles.set(k, val);
    },
    getPropertyValue: (k: string) => styles.get(k) ?? "",
  };
  return {
    el: {
      dataset,
      className: "",
      style: styleObj,
      classList: {
        toggle: (name: string, force?: boolean) => {
          if (force === undefined) {
            if (classes.has(name)) classes.delete(name);
            else classes.add(name);
          } else if (force) {
            classes.add(name);
          } else {
            classes.delete(name);
          }
        },
        contains: (name: string) => classes.has(name),
      },
      appendChild: (child: unknown) => child,
      remove: () => {},
    } as unknown as HTMLDivElement,
    styles,
    classes,
    dataset,
  };
}

describe("Phase 2 fonts & Nerd Font fallback", () => {
  it("builds a deduplicated font stack with Nerd Font PUA, Japanese, and Emoji fallbacks", () => {
    const stack = buildTerminalFontStack('"SF Mono", "Menlo"');
    expect(stack.startsWith('"SF Mono", "Menlo",')).toBe(true);
    for (const nf of NERD_FONT_FALLBACK_FAMILIES) {
      expect(stack).toContain(nf);
    }
    for (const jp of JAPANESE_FONT_FALLBACK_FAMILIES) {
      expect(stack).toContain(jp);
    }
    expect(stack.endsWith(", monospace")).toBe(true);
  });

  it("omits Nerd Font fallback when explicitly disabled and deduplicates user-supplied fonts", () => {
    const stack = buildTerminalFontStack('"UDEV Gothic 35NF", monospace', {
      nerdFontFallback: false,
    });
    expect(stack.startsWith('"UDEV Gothic 35NF",')).toBe(true);
    expect(stack).not.toContain('"Symbols Nerd Font Mono"');
    expect(stack.match(/monospace/g)?.length).toBe(1);

    const deduped = buildTerminalFontStack('"UDEV Gothic 35NF"', { nerdFontFallback: true });
    expect(deduped.match(/UDEV Gothic 35NF/g)?.length).toBe(1);

    // Unclosed trailing quote must not break CSS fallback chain
    const unclosed = buildTerminalFontStack('"UDEV Gothic 35NF', { nerdFontFallback: true });
    expect(unclosed.startsWith("UDEV Gothic 35NF,")).toBe(true);
    expect(unclosed).toContain('"Symbols Nerd Font Mono"');
    expect(unclosed.endsWith(", monospace")).toBe(true);
  });

  it("clamps fontSize and lineHeight within safe bounds", () => {
    expect(clampFontSize(4)).toBe(8);
    expect(clampFontSize(100)).toBe(48);
    expect(clampFontSize(Number.NaN)).toBe(DEFAULT_FONT_SIZE);
    expect(clampLineHeight(0.5)).toBe(1.0);
    expect(clampLineHeight(3.5)).toBe(2.0);
    expect(clampLineHeight(Number.NaN)).toBe(DEFAULT_LINE_HEIGHT);
  });
});

describe("Phase 2 appearance, ANSI palette & cursor", () => {
  it("provides distinct normal (0-7) and bright (8-15) ANSI colors and converts to 24-bit RGB", () => {
    expect(DEFAULT_TERMINAL_THEME.palette).toHaveLength(16);
    for (let i = 0; i < 8; i++) {
      expect(DEFAULT_TERMINAL_THEME.palette[i]).not.toBe(DEFAULT_TERMINAL_THEME.palette[i + 8]);
    }
    expect(hexToRgbNumber("#09090b")).toBe(0x09090b);
    expect(hexToRgbNumber("#fff")).toBe(0xffffff);
    expect(hexToRgbNumber("invalid")).toBeNull();

    const wtermColors = toWtermThemeColors(DEFAULT_TERMINAL_THEME);
    expect(wtermColors).not.toBeNull();
    expect(wtermColors?.background).toBe(0x09090b);
    expect(wtermColors?.foreground).toBe(0xf4f4f5);
    expect(wtermColors?.palette).toHaveLength(16);
  });

  it("maps cursor blink modes and applies CSS custom properties onto host element", () => {
    expect(toWtermCursorBlink("auto")).toBeUndefined();
    expect(toWtermCursorBlink("on")).toBe(true);
    expect(toWtermCursorBlink("off")).toBe(false);

    const { el, styles, classes } = createFakeHost();
    applyAppearanceToHost(el, {
      ...DEFAULT_TERMINAL_APPEARANCE,
      fontSize: 16,
      lineHeight: 1.5,
      cursorStyle: "bar",
      cursorBlink: "off",
    });

    expect(styles.get("--term-font-size")).toBe("16px");
    expect(styles.get("--term-line-height")).toBe("1.5");
    expect(styles.get("--term-row-height")).toBe("24px");
    expect(styles.get("--term-bg")).toBe("#09090b");
    expect(styles.get("--term-color-1")).toBe(DEFAULT_TERMINAL_THEME.palette[1]);
    expect(styles.get("--term-color-15")).toBe(DEFAULT_TERMINAL_THEME.palette[15]);
    expect(classes.has("cursor-steady")).toBe(true);
    expect(classes.has("cursor-blink")).toBe(false);
  });
});

describe("Phase 2 keyboard shortcuts, Option-as-Meta, and IME protection", () => {
  const baseKey = {
    key: "",
    code: "",
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    shiftKey: false,
    isComposing: false,
    keyCode: 0,
  };

  it("detects macOS platform strings", () => {
    expect(isMacPlatform("MacIntel")).toBe(true);
    expect(isMacPlatform("macOS")).toBe(true);
    expect(isMacPlatform("Linux x86_64")).toBe(false);
  });

  it("never triggers shortcuts or Option-as-Meta during IME composition", () => {
    expect(isImeComposingEvent({ key: "Enter", keyCode: 229, isComposing: false }, false)).toBe(
      true,
    );
    expect(isImeComposingEvent({ key: "Process", keyCode: 0, isComposing: false }, false)).toBe(
      true,
    );
    expect(isImeComposingEvent({ key: "a", keyCode: 65, isComposing: false }, true)).toBe(true);

    expect(
      matchAppShortcut(
        { ...baseKey, key: "t", code: "KeyT", metaKey: true },
        { isMac: true, isComposing: true, hasSelection: false },
      ),
    ).toBeNull();

    expect(
      matchAppShortcut(
        { ...baseKey, key: "t", code: "KeyT", metaKey: true, keyCode: 229 },
        { isMac: true, isComposing: false, hasSelection: false },
      ),
    ).toBeNull();

    expect(
      translateMacOptionKey(
        { ...baseKey, key: "∫", code: "KeyB", altKey: true, isComposing: true },
        { isMac: true, macOptionAsMeta: true, isComposing: false },
      ),
    ).toBeNull();
  });

  it("separates macOS app shortcuts from terminal control keys", () => {
    const macOpts = { isMac: true, isComposing: false, hasSelection: true };

    expect(
      matchAppShortcut({ ...baseKey, key: "c", code: "KeyC", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.copySelection");
    expect(
      matchAppShortcut(
        { ...baseKey, key: "c", code: "KeyC", metaKey: true },
        { ...macOpts, hasSelection: false },
      ),
    ).toBeNull();
    expect(
      matchAppShortcut({ ...baseKey, key: "a", code: "KeyA", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.selectAll");
    expect(
      matchAppShortcut({ ...baseKey, key: "t", code: "KeyT", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.create");
    expect(
      matchAppShortcut({ ...baseKey, key: "w", code: "KeyW", metaKey: true }, macOpts),
    ).toBeNull();
    expect(
      matchAppShortcut(
        { ...baseKey, key: "W", code: "KeyW", metaKey: true, shiftKey: true },
        macOpts,
      )?.actionId,
    ).toBe("terminal.close");
    expect(
      matchAppShortcut({ ...baseKey, key: "=", code: "Equal", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.zoomIn");
    // JIS keyboard Cmd+= is Shift+Minus (shiftKey: true, key: "=", code: "Minus")
    expect(
      matchAppShortcut(
        { ...baseKey, key: "=", code: "Minus", metaKey: true, shiftKey: true },
        macOpts,
      )?.actionId,
    ).toBe("terminal.zoomIn");
    expect(
      matchAppShortcut({ ...baseKey, key: "-", code: "Minus", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.zoomOut");
    expect(
      matchAppShortcut({ ...baseKey, key: "0", code: "Digit0", metaKey: true }, macOpts)?.actionId,
    ).toBe("terminal.resetFontSize");
    expect(
      matchAppShortcut(
        { ...baseKey, key: "d", code: "KeyD", metaKey: true, shiftKey: true },
        macOpts,
      )?.actionId,
    ).toBe("app.toggleDiagnostics");

    // Must never hijack PTY control sequences or native Cmd+V paste
    for (const ptyEvent of [
      { ...baseKey, key: "c", code: "KeyC", ctrlKey: true },
      { ...baseKey, key: "d", code: "KeyD", ctrlKey: true },
      { ...baseKey, key: "z", code: "KeyZ", ctrlKey: true },
      { ...baseKey, key: "a", code: "KeyA", ctrlKey: true },
      { ...baseKey, key: "[", code: "BracketLeft", ctrlKey: true },
      { ...baseKey, key: "Escape", code: "Escape" },
      { ...baseKey, key: "v", code: "KeyV", metaKey: true },
    ]) {
      expect(matchAppShortcut(ptyEvent, macOpts)).toBeNull();
    }

    // Non-Latin layout on Linux falls back to physical Key* code when event.key is non-ASCII
    const linuxOpts = { isMac: false, isComposing: false, hasSelection: true };
    expect(
      matchAppShortcut(
        { ...baseKey, key: "Е", code: "KeyT", ctrlKey: true, shiftKey: true },
        linuxOpts,
      )?.actionId,
    ).toBe("terminal.create");
    expect(
      matchAppShortcut(
        { ...baseKey, key: "С", code: "KeyC", ctrlKey: true, shiftKey: true },
        linuxOpts,
      )?.actionId,
    ).toBe("terminal.copySelection");
  });

  it("translates macOS Option+<key> to ESC+<ascii> Meta sequences for fish/Neovim/Codex", () => {
    const opts = { isMac: true, macOptionAsMeta: true, isComposing: false, kittyKeyboardFlags: 0 };

    expect(translateMacOptionKey({ ...baseKey, key: "∫", code: "KeyB", altKey: true }, opts)).toBe(
      "\x1bb",
    );
    expect(translateMacOptionKey({ ...baseKey, key: "ƒ", code: "KeyF", altKey: true }, opts)).toBe(
      "\x1bf",
    );
    expect(translateMacOptionKey({ ...baseKey, key: "∂", code: "KeyD", altKey: true }, opts)).toBe(
      "\x1bd",
    );
    expect(
      translateMacOptionKey({ ...baseKey, key: "≥", code: "Period", altKey: true }, opts),
    ).toBe("\x1b.");
    expect(
      translateMacOptionKey(
        { ...baseKey, key: "ı", code: "KeyB", altKey: true, shiftKey: true },
        opts,
      ),
    ).toBe("\x1bB");
    expect(
      translateMacOptionKey(
        { ...baseKey, key: "Backspace", code: "Backspace", altKey: true },
        opts,
      ),
    ).toBe("\x1b\x7f");

    // Disabled when Kitty keyboard protocol owns keys or macOptionAsMeta is false
    expect(
      translateMacOptionKey(
        { ...baseKey, key: "∫", code: "KeyB", altKey: true },
        { ...opts, kittyKeyboardFlags: 1 },
      ),
    ).toBeNull();
    expect(
      translateMacOptionKey(
        { ...baseKey, key: "∫", code: "KeyB", altKey: true },
        { ...opts, macOptionAsMeta: false },
      ),
    ).toBeNull();
  });
});

describe("Phase 2 built-in terminal & appearance actions", () => {
  it("executes paste, selectAll, clearSelection, restart, zoom, setFont, setCursor, and toggleDiagnostics", async () => {
    const pasted: string[] = [];
    const appliedAppearances: unknown[] = [];
    let selectedAll = false;
    let cleared = false;
    let runtime: ReturnType<typeof createAppRuntime> | null = null;

    try {
      const fakeDoc = { createElement: () => createFakeHost().el };
      vi.stubGlobal("document", fakeDoc);

      const client = new MockIpcClient();
      let readClipboard = async () => "貼り付けテスト";
      runtime = createAppRuntime({
        client,
        readClipboard: () => readClipboard(),
        adapterFactory: () =>
          ({
            backendId: "wterm-ghostty-dom",
            cols: 100,
            rows: 30,
            async mount() {},
            write() {},
            resize() {},
            fit() {},
            focus() {},
            setHidden() {},
            async getSelection() {
              return "";
            },
            hasSelection() {
              return selectedAll;
            },
            async selectAll() {
              selectedAll = true;
              return true;
            },
            clearSelection() {
              cleared = true;
              selectedAll = false;
            },
            paste(text: string) {
              pasted.push(text);
            },
            applyAppearance(patch) {
              appliedAppearances.push(patch);
            },
            dispose() {},
          }) satisfies TerminalAdapter,
      });

      runtime.setMountRoot(createFakeHost().el);
      const created = await runtime.actions.execute("terminal.create", { activate: true });
      expect(created.ok).toBe(true);
      if (!created.ok) return;
      const { tabId, sessionId: firstSessionId } = created.value;

      // Paste from clipboard
      const pasteRes = await runtime.actions.execute("terminal.paste", {}, { caller: "keybind" });
      expect(pasteRes).toEqual({ ok: true, value: { pastedLength: 7 } });
      expect(pasted).toEqual(["貼り付けテスト"]);

      // Select all & clear selection
      expect(
        await runtime.actions.execute("terminal.selectAll", {}, { caller: "keybind" }),
      ).toEqual({
        ok: true,
        value: { selected: true },
      });
      expect(
        await runtime.actions.execute("terminal.clearSelection", {}, { caller: "keybind" }),
      ).toEqual({
        ok: true,
        value: { cleared: true },
      });
      expect(cleared).toBe(true);

      // Zoom & font/cursor live propagation
      const zoomIn = await runtime.actions.execute("terminal.zoomIn", {}, { caller: "keybind" });
      expect(zoomIn).toEqual({ ok: true, value: { fontSize: DEFAULT_FONT_SIZE + 1 } });
      const zoomOut = await runtime.actions.execute("terminal.zoomOut", {}, { caller: "keybind" });
      expect(zoomOut).toEqual({ ok: true, value: { fontSize: DEFAULT_FONT_SIZE } });
      const setFont = await runtime.actions.execute(
        "app.setFont",
        { fontFamily: '"UDEV Gothic 35NF"', fontSize: 16, lineHeight: 1.3 },
        { caller: "config" },
      );
      expect(setFont.ok).toBe(true);
      const setCursor = await runtime.actions.execute(
        "app.setCursor",
        { cursorStyle: "bar", cursorBlink: "off" },
        { caller: "config" },
      );
      expect(setCursor).toEqual({ ok: true, value: { cursorStyle: "bar", cursorBlink: "off" } });
      expect(appliedAppearances).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ fontSize: DEFAULT_FONT_SIZE + 1 }),
          expect.objectContaining({ fontSize: DEFAULT_FONT_SIZE }),
          expect.objectContaining({
            fontFamily: '"UDEV Gothic 35NF"',
            fontSize: 16,
            lineHeight: 1.3,
          }),
          expect.objectContaining({ cursorStyle: "bar", cursorBlink: "off" }),
        ]),
      );

      const resetZoom = await runtime.actions.execute(
        "terminal.resetFontSize",
        {},
        { caller: "keybind" },
      );
      expect(resetZoom).toEqual({ ok: true, value: { fontSize: DEFAULT_FONT_SIZE } });

      // A delayed clipboard read belongs to the original terminal instance.
      let finishClipboardRead!: (text: string) => void;
      readClipboard = () =>
        new Promise((resolve) => {
          finishClipboardRead = resolve;
        });
      const delayedPaste = runtime.actions.execute("terminal.paste", {}, { caller: "keybind" });

      // Restart session in same tab
      client.emitTestEvent({ type: "disconnected", reason: "test" });
      const restartedPromise = runtime.actions.execute(
        "terminal.restart",
        { tabId },
        { caller: "gui" },
      );
      const duplicate = await runtime.actions.execute(
        "terminal.restart",
        { tabId },
        { caller: "gui" },
      );
      expect(duplicate.ok).toBe(false);
      const restarted = await restartedPromise;
      expect(runtime.store.getState().coreConnection.status).toBe("connected");
      expect(restarted.ok).toBe(true);
      if (restarted.ok) {
        expect(restarted.value.tabId).toBe(tabId);
        expect(restarted.value.sessionId).not.toBe(firstSessionId);
        expect(runtime.store.getState().sessions[restarted.value.sessionId]?.state).toBe("live");
      }

      finishClipboardRead("古いセッションの入力");
      expect(await delayedPaste).toEqual({ ok: true, value: { pastedLength: 0 } });
      expect(pasted).toEqual(["貼り付けテスト"]);

      // Diagnostics drawer defaults to hidden and toggles cleanly
      expect(runtime.store.getState().ui.diagnosticsPanelVisible).toBe(false);
      const toggled = await runtime.actions.execute(
        "app.toggleDiagnostics",
        {},
        { caller: "keybind" },
      );
      expect(toggled).toEqual({ ok: true, value: { diagnosticsPanelVisible: true } });
    } finally {
      runtime?.dispose();
      vi.unstubAllGlobals();
    }
  });

  it("verifies all 3 Phase 2 font patterns (default, Nerd Font, Menlo fallback) via app.setFont", async () => {
    let runtime: ReturnType<typeof createAppRuntime> | null = null;

    try {
      const fakeDoc = { createElement: () => createFakeHost().el };
      vi.stubGlobal("document", fakeDoc);

      const { el: hostEl, styles } = createFakeHost();
      runtime = createAppRuntime({
        client: new MockIpcClient(),
        adapterFactory: () =>
          ({
            backendId: "wterm-ghostty-dom",
            cols: 100,
            rows: 30,
            async mount() {
              applyAppearanceToHost(hostEl, DEFAULT_TERMINAL_APPEARANCE);
            },
            write() {},
            resize() {},
            fit() {},
            focus() {},
            setHidden() {},
            async getSelection() {
              return "";
            },
            paste() {},
            applyAppearance(patch) {
              applyAppearanceToHost(hostEl, { ...DEFAULT_TERMINAL_APPEARANCE, ...patch });
            },
            dispose() {},
          }) satisfies TerminalAdapter,
      });

      runtime.setMountRoot(createFakeHost().el);
      await runtime.actions.execute("terminal.create", { activate: true });

      // 1. Default font pattern
      expect(styles.get("--term-font-family")).toContain('ui-monospace, "SF Mono", "Menlo"');
      expect(styles.get("--term-font-family")).toContain('"Symbols Nerd Font Mono"');
      expect(styles.get("--term-font-family")).toContain('"Hiragino Sans"');

      // 2. Nerd Font pattern (e.g. UDEV Gothic 35NF, size 12.5)
      const nfRes = await runtime.actions.execute(
        "app.setFont",
        { fontFamily: '"UDEV Gothic 35NF"', fontSize: 12.5, nerdFontFallback: true },
        { caller: "gui" },
      );
      expect(nfRes.ok).toBe(true);
      expect(styles.get("--term-font-family")?.startsWith('"UDEV Gothic 35NF",')).toBe(true);
      expect(styles.get("--term-font-family")?.match(/UDEV Gothic 35NF/g)?.length).toBe(1);
      expect(styles.get("--term-font-size")).toBe("12.5px");

      // 3a. Menlo with Nerd Font fallback ON (PUA falls back to installed Nerd Font, CJK to Hiragino Sans)
      const menloFallbackOn = await runtime.actions.execute(
        "app.setFont",
        { fontFamily: '"Menlo"', nerdFontFallback: true },
        { caller: "gui" },
      );
      expect(menloFallbackOn.ok).toBe(true);
      expect(styles.get("--term-font-family")?.startsWith('"Menlo",')).toBe(true);
      expect(styles.get("--term-font-family")).toContain('"Symbols Nerd Font Mono"');
      expect(styles.get("--term-font-family")).toContain('"Hiragino Sans"');

      // 3b. Menlo with Nerd Font fallback OFF (pure Menlo -> Japanese fallback)
      const menloFallbackOff = await runtime.actions.execute(
        "app.setFont",
        { fontFamily: '"Menlo"', nerdFontFallback: false },
        { caller: "gui" },
      );
      expect(menloFallbackOff.ok).toBe(true);
      expect(styles.get("--term-font-family")?.startsWith('"Menlo",')).toBe(true);
      expect(styles.get("--term-font-family")).not.toContain('"Symbols Nerd Font Mono"');
      expect(styles.get("--term-font-family")).toContain('"Hiragino Sans"');
    } finally {
      runtime?.dispose();
      vi.unstubAllGlobals();
    }
  });

  it("reconciles settings changed during mount, hides restarted pane after tab switch, and preserves focus on non-focusing setActive", async () => {
    let runtime: ReturnType<typeof createAppRuntime> | null = null;
    try {
      const fakeDoc = { createElement: () => createFakeHost().el };
      vi.stubGlobal("document", fakeDoc);

      const appliedByAdapter = new Map<number, Array<Record<string, unknown>>>();
      let focusCount = 0;
      let releaseSecondMount!: () => void;
      let mountIndex = 0;

      runtime = createAppRuntime({
        client: new MockIpcClient(),
        adapterFactory: () => {
          const idx = mountIndex++;
          return {
            backendId: "wterm-ghostty-dom",
            cols: 80,
            rows: 24,
            async mount() {
              if (idx === 1) {
                await new Promise<void>((resolve) => {
                  releaseSecondMount = resolve;
                });
              }
            },
            write() {},
            resize() {},
            fit() {},
            focus() {
              focusCount++;
            },
            setHidden() {},
            async getSelection() {
              return "";
            },
            paste() {},
            applyAppearance(patch) {
              const list = appliedByAdapter.get(idx) ?? [];
              list.push(patch as Record<string, unknown>);
              appliedByAdapter.set(idx, list);
            },
            dispose() {},
          } satisfies TerminalAdapter;
        },
      });

      runtime.setMountRoot(createFakeHost().el);
      const tab1 = await runtime.actions.execute("terminal.create", { activate: true });
      expect(tab1.ok).toBe(true);
      if (!tab1.ok) return;

      // Start creating tab2 (paused in mount), update font size while mount is in flight
      const tab2Promise = runtime.actions.execute("terminal.create", { activate: true });
      await new Promise((resolve) => setTimeout(resolve, 0));
      await runtime.actions.execute("app.setFont", { fontSize: 18 }, { caller: "gui" });
      releaseSecondMount();
      const tab2 = await tab2Promise;
      expect(tab2.ok).toBe(true);
      if (!tab2.ok) return;
      expect(appliedByAdapter.get(1)?.some((patch) => patch.fontSize === 18)).toBe(true);

      // Non-focusing setActive (used by benchmarks) must not steal focus
      const focusBefore = focusCount;
      runtime.sessionManager.setActive(tab1.value.paneId, { focus: false });
      expect(focusCount).toBe(focusBefore);

      // Restart tab1 while switching selection to tab2 -> tab1 container must end up hidden
      await runtime.actions.execute("tab.select", { tabId: tab1.value.tabId });
      const restartPromise = runtime.actions.execute("terminal.restart", {
        tabId: tab1.value.tabId,
      });
      await runtime.actions.execute("tab.select", { tabId: tab2.value.tabId });
      const restarted = await restartPromise;
      expect(restarted.ok).toBe(true);
      expect(runtime.sessionManager.get(tab1.value.paneId)?.container.style.display).toBe("none");
      expect(runtime.sessionManager.get(tab2.value.paneId)?.container.style.display).toBe("block");
    } finally {
      runtime?.dispose();
      vi.unstubAllGlobals();
    }
  });
});
