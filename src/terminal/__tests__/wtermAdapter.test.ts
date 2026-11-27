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
  const optionB = () =>
    Object.assign(new Event("keydown", { cancelable: true }), {
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
    const ctrlC = Object.assign(new Event("keydown", { cancelable: true }), {
      key: "c",
      ctrlKey: true,
    });
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
