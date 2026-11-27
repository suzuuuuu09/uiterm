import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { expect, it } from "vite-plus/test";

const resolve = createRequire(import.meta.url).resolve;
const { Renderer } = await import(
  new URL("./renderer.js", pathToFileURL(resolve("@wterm/dom"))).href
);

function renderRow(cells) {
  const row = { style: {}, innerHTML: "" };
  const renderer = Object.assign(Object.create(Renderer.prototype), {
    cols: cells.length,
    rowParts: new WeakMap(),
    rowText: new WeakMap(),
    rowBackground: new WeakMap(),
  });

  renderer._buildRowContent(row, (col) => cells[col], cells.length, -1, -1);
  return row;
}

const background = "rgb(64,96,144)";
const cell = { char: 65, fg: 256, bg: 256, bgRgb: 0x406090, width: 1, flags: 0 };

it("keeps backgrounds opaque across normal, dim and concealed cells", () => {
  const row = renderRow([cell, { ...cell, flags: 2 }, { ...cell, flags: 64, char: 32 }]);

  expect(row.innerHTML).toBe(
    `<span style="background:${background};">A</span>` +
      `<span style="background:${background};"><span style="opacity:0.5;">A</span></span>` +
      `<span style="background:${background};"><span style="visibility:hidden;"> </span></span>`,
  );
  expect(row.style.background).toBe(background);
});

it("does not extend a mixed background beyond its last colored cell", () => {
  const row = renderRow([
    { ...cell, bgRgb: undefined },
    { ...cell, flags: 2, char: 32 },
  ]);

  expect(row.style.background).toBe("");
});

it("conceals a block glyph without replacing or hiding the reversed background", () => {
  const row = renderRow([{ ...cell, char: 0x2588, fgRgb: 0xe0e0e0, flags: 0x20 | 0x40 }]);

  expect(row.style.background).toBe("rgb(224,224,224)");
  expect(row.innerHTML).toBe(
    `<span style="color:${background};background:rgb(224,224,224);"><span style="visibility:hidden;">█</span></span>`,
  );
});
