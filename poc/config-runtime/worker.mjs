import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

let loadedConfig = null;

const ctx = {
  async callHost(method, params = {}) {
    // In Phase 0 PoC, host calls are recorded and echoed back to prove
    // async helper invocation works cleanly over the boundary.
    return { hostMethod: method, params, ok: true };
  },
};

function send(obj) {
  process.stdout.write(`${JSON.stringify(obj)}\n`);
}

function serializeConfigSummary(cfg) {
  return {
    shell: cfg?.shell ?? null,
    font: cfg?.font ?? null,
    theme: cfg?.theme ?? null,
    actionNames: Object.keys(cfg?.actions ?? {}),
    hookNames: Object.keys(cfg?.hooks ?? {}),
  };
}

async function handleRequest(req) {
  const { id, method, params = {} } = req;
  try {
    if (method === "ping") {
      return send({ jsonrpc: "2.0", id, result: { pong: true, node: process.version } });
    }
    if (method === "config.load") {
      const fileUrl = pathToFileURL(params.path).href;
      const mod = await import(`${fileUrl}?t=${Date.now()}`);
      loadedConfig = mod.default ?? mod.config ?? null;
      if (!loadedConfig || typeof loadedConfig !== "object") {
        throw new Error("Config module must export a default object");
      }
      return send({
        jsonrpc: "2.0",
        id,
        result: serializeConfigSummary(loadedConfig),
      });
    }
    if (method === "action.invoke") {
      const fn = loadedConfig?.actions?.[params.name];
      if (typeof fn !== "function") {
        throw new Error(`Unknown action: ${params.name}`);
      }
      const result = await fn(params.args, ctx);
      return send({ jsonrpc: "2.0", id, result });
    }
    if (method === "hook.emit") {
      const fn = loadedConfig?.hooks?.[params.event];
      if (typeof fn !== "function") {
        return send({ jsonrpc: "2.0", id, result: null });
      }
      const result = await fn(params.payload, ctx);
      return send({ jsonrpc: "2.0", id, result });
    }
    if (method === "runtime.metrics") {
      const mem = process.memoryUsage();
      return send({
        jsonrpc: "2.0",
        id,
        result: {
          pid: process.pid,
          nodeVersion: process.version,
          rssBytes: mem.rss,
          heapUsedBytes: mem.heapUsed,
          heapTotalBytes: mem.heapTotal,
        },
      });
    }
    if (method === "shutdown") {
      send({ jsonrpc: "2.0", id, result: { bye: true } });
      process.exit(0);
    }
    return send({
      jsonrpc: "2.0",
      id,
      error: { code: -32601, message: `Method not found: ${method}` },
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return send({
      jsonrpc: "2.0",
      id,
      error: { code: -32000, message },
    });
  }
}

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
let requestQueue = Promise.resolve();
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  requestQueue = requestQueue.then(async () => {
    try {
      const req = JSON.parse(trimmed);
      await handleRequest(req);
    } catch (err) {
      send({
        jsonrpc: "2.0",
        id: null,
        error: { code: -32700, message: String(err) },
      });
    }
  });
});
