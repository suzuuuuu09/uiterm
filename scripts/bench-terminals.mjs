/**
 * Reproducible Phase 0 terminal VT core benchmark & bundle size measurement script.
 *
 * Run with:
 *   node --expose-gc scripts/bench-terminals.mjs
 *
 * Measures:
 * 1. Standalone package JS / WASM raw & gzipped sizes
 * 2. Real VT parsing throughput (10,000 lines of ANSI + Japanese UTF-8, ~1.05 MB)
 * 3. Multi-instance scaling (1, 5, 10 terminal instances): creation time, idle heap/RSS,
 *    scrollback growth, active/hidden throughput, and memory release after disposal
 * 4. Terminal auto-response (DA1 / DSR / OSC 11) behavior
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { gzipSync } from "node:zlib";

const encoder = new TextEncoder();

// Build a 10,000-line realistic payload (ANSI SGR + Japanese UTF-8)
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
  ["@wterm/core wasm", "node_modules/@wterm/core/wasm/wterm.wasm"],
  ["@wterm/ghostty wasm", "node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm"],
  ["@wterm/dom wterm.js", "node_modules/@wterm/dom/dist/wterm.js"],
  ["@wterm/dom input.js", "node_modules/@wterm/dom/dist/input.js"],
  ["@wterm/dom renderer.js", "node_modules/@wterm/dom/dist/renderer.js"],
  ["@wterm/dom gpu-renderer.js", "node_modules/@wterm/dom/dist/gpu-renderer.js"],
  ["@gespenst/core wasm", "node_modules/@gespenst/core/dist/ghostty-vt.wasm"],
  ["@gespenst/core index.js", "node_modules/@gespenst/core/dist/index.js"],
  ["@gespenst/core worker.js", "node_modules/@gespenst/core/dist/assets/terminal-worker-CfcHI8XP.js"],
  ["ghostty-web wasm", "node_modules/ghostty-web/dist/ghostty-vt.wasm"],
  ["ghostty-web js", "node_modules/ghostty-web/dist/ghostty-web.js"],
  ["@xterm/xterm mjs", "node_modules/@xterm/xterm/lib/xterm.mjs"],
  ["@xterm/addon-webgl mjs", "node_modules/@xterm/addon-webgl/lib/addon-webgl.mjs"],
  ["@xterm/addon-fit mjs", "node_modules/@xterm/addon-fit/lib/addon-fit.mjs"],
];
for (const [label, path] of artifacts) {
  try {
    const s = fileSizeKB(path);
    console.log(`${label.padEnd(28)} raw=${String(s.rawKB).padStart(7)} KB  gzip=${String(s.gzipKB).padStart(6)} KB`);
  } catch (e) {
    console.log(`${label.padEnd(28)} ERROR: ${e.message}`);
  }
}

const payload = makePayload(10_000);
console.log(`\n=== 2. Headless VT Core Benchmarks (10,000 lines, ${(payload.totalBytes / 1024).toFixed(1)} KB, scrollback=5000) ===`);

// Benchmark @wterm/ghostty (libghostty WASM)
async function benchWtermGhostty() {
  const { GhosttyCore } = await import("@wterm/ghostty");
  const wasmPath = resolve(process.cwd(), "node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm");
  // Patch fetch for local file path in Node
  const wasmBuf = readFileSync(wasmPath);
  const origFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (String(url).includes("ghostty-vt.wasm")) {
      return new Response(wasmBuf, { headers: { "content-type": "application/wasm" } });
    }
    return origFetch(url);
  };

  try {
    const results = [];
    for (const count of [1, 5, 10]) {
      forceGc();
      const m0 = memMB();
      const tCreate0 = performance.now();
      const cores = [];
      for (let i = 0; i < count; i++) {
        const core = await GhosttyCore.load({ wasmPath: "file:///ghostty-vt.wasm", scrollbackLines: 5000 });
        core.init(120, 40);
        cores.push(core);
      }
      const createMs = performance.now() - tCreate0;
      const mIdle = memMB();

      // Write 10k lines to first instance
      const tWrite0 = performance.now();
      for (const chunk of payload.chunks) {
        cores[0].writeRaw(chunk);
      }
      const writeMs = performance.now() - tWrite0;

      // Write 10k lines to all instances (scrollback growth)
      for (let i = 1; i < count; i++) {
        for (const chunk of payload.chunks) {
          cores[i].writeRaw(chunk);
        }
      }
      const mLoaded = memMB();

      // Test auto-response query on cores[0]
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
    console.log("\n[@wterm/ghostty (libghostty WASM)]");
    console.table(results);
  } finally {
    globalThis.fetch = origFetch;
  }
}

// Benchmark ghostty-web WASM
async function benchGhosttyWeb() {
  const { Ghostty } = await import("ghostty-web");
  const wasmPath = resolve(process.cwd(), "node_modules/ghostty-web/dist/ghostty-vt.wasm");
  const wasmBuf = readFileSync(wasmPath);
  const origFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (String(url).includes("ghostty-vt.wasm")) {
      return new Response(wasmBuf, { headers: { "content-type": "application/wasm" } });
    }
    return origFetch(url);
  };

  try {
    const ghostty = await Ghostty.load("file:///ghostty-vt.wasm");
    const results = [];
    for (const count of [1, 5, 10]) {
      forceGc();
      const m0 = memMB();
      const tCreate0 = performance.now();
      const terms = [];
      for (let i = 0; i < count; i++) {
        terms.push(ghostty.createTerminal(120, 40, { scrollbackLimit: 5000 }));
      }
      const createMs = performance.now() - tCreate0;
      const mIdle = memMB();

      const tWrite0 = performance.now();
      for (const chunk of payload.chunks) {
        terms[0].write(chunk);
      }
      const writeMs = performance.now() - tWrite0;

      for (let i = 1; i < count; i++) {
        for (const chunk of payload.chunks) {
          terms[i].write(chunk);
        }
      }
      const mLoaded = memMB();

      const tDisp0 = performance.now();
      for (const term of terms) term.free();
      terms.length = 0;
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
      });
    }
    console.log("\n[ghostty-web (libghostty WASM, shared WASM instance)]");
    console.table(results);
  } finally {
    globalThis.fetch = origFetch;
  }
}

// Benchmark @gespenst/core headless
async function benchGespenstHeadless() {
  const headless = await import("@gespenst/core/headless");
  const wasmPath = resolve(process.cwd(), "node_modules/@gespenst/core/dist/ghostty-vt.wasm");
  const callbacksPath = resolve(process.cwd(), "node_modules/@gespenst/core/dist/ghostty-callbacks.wasm");
  const runtime = await headless.createCoreRuntime({
    wasmUrl: wasmPath,
    callbacksUrl: callbacksPath,
  });
  const results = [];
  for (const count of [1, 5, 10]) {
    forceGc();
    const m0 = memMB();
    const tCreate0 = performance.now();
    const terms = [];
    for (let i = 0; i < count; i++) {
      terms.push(
        runtime.createTerminal({
          cols: 120,
          rows: 40,
          scrollbackLines: 5000,
        }),
      );
    }
    const createMs = performance.now() - tCreate0;
    const mIdle = memMB();

    const tWrite0 = performance.now();
    for (const chunk of payload.chunks) {
      terms[0].write(chunk);
    }
    const writeMs = performance.now() - tWrite0;

    for (let i = 1; i < count; i++) {
      for (const chunk of payload.chunks) {
        terms[i].write(chunk);
      }
    }
    const mLoaded = memMB();

    let replyEvents = 0;
    terms[0].on("input", (ev) => {
      if (ev.source === "reply") replyEvents++;
    });
    terms[0].write(encoder.encode("\x1b[c\x1b[6n"));

    const tDisp0 = performance.now();
    for (const term of terms) term.dispose();
    terms.length = 0;
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
      replyEvents,
    });
  }
  console.log("\n[@gespenst/core/headless (libghostty WASM, shared runtime)]");
  console.table(results);
}

await benchWtermGhostty();
await benchGhosttyWeb();
await benchGespenstHeadless();
