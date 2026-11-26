/**
 * Reproducible Phase 0 terminal VT core benchmark & bundle size measurement script
 * for the selected @wterm/dom + @wterm/ghostty stack.
 *
 * Run with:
 *   node --expose-gc scripts/bench-terminals.mjs
 *
 * Historical multi-renderer comparison results (@gespenst/core, ghostty-web, @xterm/xterm)
 * are preserved in docs/poc/frontend.md.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { gzipSync } from "node:zlib";

const encoder = new TextEncoder();

function makePayload(lines = 10_000) {
  const chunks = [];
  let totalBytes = 0;
  for (let i = 0; i < lines; i += 200) {
    let block = "";
    const end = Math.min(i + 200, lines);
    for (let l = i; l < end; l++) {
      block += `\x1b[36m[${String(l).padStart(5, "0")}]\x1b[0m \x1b[32mINFO\x1b[0m uiterm-bench: 日本語ログ出力テスト payload=0123456789abcdef\r\n`;
    }
    const bytes = encoder.encode(block);
    chunks.push(bytes);
    totalBytes += bytes.byteLength;
  }
  return { chunks, totalBytes };
}

function forceGc() {
  if (typeof globalThis.gc === "function") {
    globalThis.gc();
    globalThis.gc();
  }
}

function memMB() {
  const m = process.memoryUsage();
  return {
    rss: m.rss / (1024 * 1024),
    heapUsed: m.heapUsed / (1024 * 1024),
    external: m.external / (1024 * 1024),
    arrayBuffers: m.arrayBuffers / (1024 * 1024),
  };
}

function fileSizeKB(relPath) {
  const buf = readFileSync(resolve(process.cwd(), relPath));
  return {
    rawKB: Number((buf.byteLength / 1024).toFixed(1)),
    gzipKB: Number((gzipSync(buf).byteLength / 1024).toFixed(1)),
  };
}

console.log("=== 1. Package Artifact Sizes (Raw / Gzip KB) ===");
const artifacts = [
  ["@wterm/ghostty wasm", "node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm"],
  ["@wterm/dom wterm.js", "node_modules/@wterm/dom/dist/wterm.js"],
  ["@wterm/dom input.js", "node_modules/@wterm/dom/dist/input.js"],
  ["@wterm/dom renderer.js", "node_modules/@wterm/dom/dist/renderer.js"],
  ["@wterm/dom gpu-renderer.js", "node_modules/@wterm/dom/dist/gpu-renderer.js"],
];
for (const [label, path] of artifacts) {
  try {
    const s = fileSizeKB(path);
    console.log(
      `${label.padEnd(28)} raw=${String(s.rawKB).padStart(7)} KB  gzip=${String(s.gzipKB).padStart(6)} KB`,
    );
  } catch (e) {
    console.log(`${label.padEnd(28)} ERROR: ${e.message}`);
  }
}

const payload = makePayload(10_000);
console.log(
  `\n=== 2. Headless @wterm/ghostty VT Core Benchmarks (10,000 lines, ${(payload.totalBytes / 1024).toFixed(1)} KB) ===`,
);

async function benchWtermGhostty(scrollbackLines) {
  const { GhosttyCore } = await import("@wterm/ghostty");
  const wasmPath = resolve(process.cwd(), "node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm");
  const wasmBuf = readFileSync(wasmPath);
  const origFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (String(url).includes("ghostty-vt.wasm")) {
      return new Response(wasmBuf, { headers: { "content-type": "application/wasm" } });
    }
    return origFetch(url);
  };

  try {
    // Warm up one-time WASM module load/compile so the first scrollback pass isn't biased.
    const warmup = await GhosttyCore.load({
      wasmPath: "file:///ghostty-vt.wasm",
      scrollbackLines,
    });
    warmup.init(120, 40);
    warmup.dispose();
    forceGc();

    const results = [];
    for (const count of [1, 5, 10]) {
      forceGc();
      const m0 = memMB();
      const tCreate0 = performance.now();
      const cores = [];
      for (let i = 0; i < count; i++) {
        const core = await GhosttyCore.load({
          wasmPath: "file:///ghostty-vt.wasm",
          scrollbackLines,
        });
        core.init(120, 40);
        cores.push(core);
      }
      const createMs = performance.now() - tCreate0;
      const mIdle = memMB();

      const tWrite0 = performance.now();
      for (const chunk of payload.chunks) {
        cores[0].writeRaw(chunk);
      }
      const writeMs = performance.now() - tWrite0;

      for (let i = 1; i < count; i++) {
        for (const chunk of payload.chunks) {
          cores[i].writeRaw(chunk);
        }
      }
      const mLoaded = memMB();

      cores[0].writeRaw(encoder.encode("\x1b[c\x1b[6n"));
      const replies = [];
      let r;
      while ((r = cores[0].getResponse()) !== null) replies.push(r);

      const tDisp0 = performance.now();
      for (const core of cores) core.dispose();
      cores.length = 0;
      const disposeMs = performance.now() - tDisp0;
      forceGc();
      const mAfter = memMB();

      const mbps = payload.totalBytes / (1024 * 1024) / (writeMs / 1000);
      results.push({
        count,
        createMs: createMs.toFixed(1),
        write10kMs: writeMs.toFixed(1),
        mbps: mbps.toFixed(1),
        idleRssDeltaMB: (mIdle.rss - m0.rss).toFixed(2),
        loadedRssDeltaMB: (mLoaded.rss - m0.rss).toFixed(2),
        afterDisposeRssDeltaMB: (mAfter.rss - m0.rss).toFixed(2),
        disposeMs: disposeMs.toFixed(2),
        repliesCount: replies.length,
      });
    }
    console.log(`\n[@wterm/ghostty (scrollbackLines=${scrollbackLines})]`);
    console.table(results);
  } finally {
    globalThis.fetch = origFetch;
  }
}

await benchWtermGhostty(5000);
await benchWtermGhostty(2000);
