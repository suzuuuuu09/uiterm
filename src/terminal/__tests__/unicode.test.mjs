import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { GhosttyCore } from "@wterm/ghostty";
import { expect, it, vi } from "vite-plus/test";

it("keeps UTF-8 fragments and graphemes in the expected terminal columns", async () => {
  const resolve = createRequire(import.meta.url).resolve;
  const wasm = readFileSync(resolve("@wterm/ghostty/ghostty-vt.wasm"));
  vi.stubGlobal("fetch", async () => new Response(new Uint8Array(wasm), {
    headers: { "content-type": "application/wasm" },
  }));
  const core = await GhosttyCore.load({ wasmPath: "file:///uiterm-unicode-test.wasm" });
  try {
    core.init(80, 24);
    const bytes = new TextEncoder().encode("AＡｶ日αe\u0301👩‍💻!");
    for (const byte of bytes) core.writeRaw(Uint8Array.of(byte));

    expect(core.getCursor().col).toBe(11);
    expect(core.getCell(0, 1).width).toBe(2);
    expect(core.getCell(0, 2).width).toBe(0);
    expect(core.getCell(0, 6).char).toBe("α".codePointAt(0));
    expect(core.getCell(0, 7).chars).toBe("e\u0301");
    expect(core.getCell(0, 8).chars).toBe("👩‍💻");
    expect(core.getCell(0, 10).char).toBe("!".codePointAt(0));

    // Verify Nerd Font Private Use Area (BMP PUA + Supplementary PUA-A) cell alignment
    core.writeRaw(new TextEncoder().encode("\r\n\uE0B0\uE718\uF015\u{F024B}|"));
    expect(core.getCursor().row).toBe(1);
    expect(core.getCursor().col).toBe(5);
    expect(core.getCell(1, 0).char).toBe(0xe0b0);
    expect(core.getCell(1, 1).char).toBe(0xe718);
    expect(core.getCell(1, 2).char).toBe(0xf015);
    expect(core.getCell(1, 3).char).toBe(0xf024b);
    expect(core.getCell(1, 4).char).toBe("|".codePointAt(0));
  } finally {
    core.dispose();
    vi.unstubAllGlobals();
  }
});

it("commits once and leaves conversion keys out of the terminal input stream", async () => {
  const resolve = createRequire(import.meta.url).resolve;
  const { InputHandler } = await import(new URL("./input.js",
    pathToFileURL(resolve("@wterm/dom"))).href);

  class Element extends EventTarget {
    style = {};
    value = "";
    attrs = new Map();
    classList = { remove() {} };
    getAttribute = (key) => this.attrs.get(key) ?? null;
    setAttribute = (key, value) => this.attrs.set(key, value);
    removeAttribute = (key) => this.attrs.delete(key);
    hasAttribute = (key) => this.attrs.has(key);
    getRootNode = () => this;
    querySelector = () => null;
    append() {}
    appendChild() {}
    remove() {}
  }

  const doc = { createElement: () => new Element() };
  vi.stubGlobal("document", doc);
  vi.stubGlobal("MutationObserver", class {
    disconnect() {}
    observe() {}
    takeRecords() { return []; }
  });
  const host = new Element();
  host.ownerDocument = doc;
  const input = vi.fn();
  const handler = new InputHandler(host, input, () => null);
  const textarea = handler.textarea;
  const emit = (type, fields = {}) => {
    const event = Object.assign(new Event(type, { cancelable: true }), { getModifierState: () => false }, fields);
    textarea.dispatchEvent(event);
    return event;
  };
  try {
    emit("compositionstart");
    expect(emit("keydown", { key: "Backspace", code: "Backspace" }).defaultPrevented).toBe(false);
    emit("keydown", { key: "c", code: "KeyC", ctrlKey: true });
    emit("keydown", { key: "ArrowDown", code: "ArrowDown" });
    emit("compositionend", { data: "日本語" });
    emit("keydown", { key: "Enter", code: "Enter", keyCode: 229, isComposing: false });
    textarea.value = "日本語";
    emit("input", { inputType: "insertText" });
    expect(input.mock.calls).toEqual([["日本語"]]);

    emit("keydown", { key: "Enter", code: "Enter", keyCode: 13, isComposing: false });
    expect(input.mock.calls).toEqual([["日本語"], ["\r"]]);
  } finally {
    handler.destroy();
    vi.unstubAllGlobals();
  }
});
