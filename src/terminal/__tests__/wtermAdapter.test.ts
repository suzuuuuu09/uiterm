import { expect, it, vi } from "vite-plus/test";
import { WTermGhosttyAdapter } from "../wtermAdapter.ts";

const { core, clearSelection } = vi.hoisted(() => ({
  core: {
    dispose: vi.fn(),
    focusEvents: () => true,
    getCursor: () => ({ col: 0, row: 0, shape: "underline" }),
    bracketedPaste: () => true,
    kittyKeyboardFlags: () => 0,
  },
  clearSelection: vi.fn(),
}));

vi.mock("@wterm/ghostty", () => ({ GhosttyCore: { load: async () => core } }));
vi.mock("@wterm/dom", () => ({
  WTerm: class {
    async init() {}
    clearSelection = clearSelection;
    setThemeColors() {}
    cols = 80;
    rows = 24;
    write() {}
    resize(cols: number, rows: number) {
      this.cols = cols;
      this.rows = rows;
    }
    fit() {}
    destroy() {}
  },
}));

it("guards the mounted Option handler during IME and consumes a regular Option key once", async () => {
  const host = Object.assign(new EventTarget(), {
    style: { setProperty() {} },
    classList: { toggle() {} },
    remove() {},
  });
  const addListener = vi.spyOn(host, "addEventListener");
  const removeListener = vi.spyOn(host, "removeEventListener");
  vi.stubGlobal("navigator", { platform: "MacIntel" });
  vi.stubGlobal("document", { createElement: () => host });
  let composing = true;
  const input = vi.fn();
  const adapter = new WTermGhosttyAdapter("dom", {
    onUserInput: (bytes) => input(new TextDecoder().decode(bytes)),
    onAutoResponse() {},
    onResize() {},
    isComposing: () => composing,
  });
  const optionB = () => Object.assign(new Event("keydown", { cancelable: true }), {
    key: "∫",
    code: "KeyB",
    altKey: true,
    isComposing: false,
  });

  try {
    await adapter.mount({ appendChild() {} } as unknown as HTMLElement);
    expect(core.focusEvents()).toBe(false);
    expect(core.getCursor().shape).toBe("underline");
    adapter.applyAppearance({ cursorStyle: "bar" });
    expect(core.getCursor().shape).toBe("bar");
    adapter.applyAppearance({ cursorStyle: "auto" });
    expect(core.getCursor().shape).toBe("underline");

    expect(host.dispatchEvent(optionB())).toBe(true);
    expect(input).not.toHaveBeenCalled();

    composing = false;
    expect(host.dispatchEvent(optionB())).toBe(false);
    expect(input.mock.calls).toEqual([["\x1bb"]]);

    clearSelection.mockClear();
    const ctrlC = Object.assign(new Event("keydown", { cancelable: true }), { key: "c", ctrlKey: true });
    expect(host.dispatchEvent(ctrlC)).toBe(true);
    expect(clearSelection).toHaveBeenCalledOnce();
    expect(input).toHaveBeenCalledOnce();

    adapter.paste("一行目\n二行目");
    expect(input.mock.calls[1]).toEqual(["\x1b[200~一行目\n二行目\x1b[201~"]);
  } finally {
    adapter.dispose();
    vi.unstubAllGlobals();
  }

  expect(removeListener.mock.calls).toEqual(addListener.mock.calls);
});


it("reports the grid to applications that enable in-band resize", async () => {
  const host = Object.assign(new EventTarget(), {
    style: { setProperty() {} },
    classList: { toggle() {} },
    remove() {},
  });
  vi.stubGlobal("document", { createElement: () => host });
  vi.stubGlobal("getComputedStyle", () => ({
    getPropertyValue: (name: string) => name === "--term-cell-width" ? "8px" : "16px",
  }));
  const responses: string[] = [];
  const adapter = new WTermGhosttyAdapter("dom", {
    onUserInput() {},
    onAutoResponse: (bytes) => responses.push(new TextDecoder().decode(bytes)),
    onResize() {},
  });
  const write = (data: string) => adapter.write(new TextEncoder().encode(data));

  try {
    await adapter.mount({ appendChild() {} } as unknown as HTMLElement);
    write("\x1b[?2048$p");
    expect(responses).toEqual([]);
    write("\x1b[?2026;20");
    write("48h");
    write("\x1b[?2048h");
    expect(responses).toEqual(["\x1b[48;24;80;384;640t", "\x1b[48;24;80;384;640t"]);

    adapter.resize(100, 30);
    expect(adapter.getResizeReport()).toEqual(new TextEncoder().encode("\x1b[48;30;100;480;800t"));

    write("\x1b[?2048;2026l");
    expect(adapter.getResizeReport()).toBeNull();
    write("\x1b[?20480h\x1b[?2048 h\x1b[?2048\x18h");
    expect(responses).toHaveLength(2);
    expect(adapter.getResizeReport()).toBeNull();

    write("\x1b[?2048h\x1bc");
    expect(adapter.getResizeReport()).toBeNull();
  } finally {
    adapter.dispose();
    vi.unstubAllGlobals();
  }
});
