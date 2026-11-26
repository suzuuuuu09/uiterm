import { Switch } from "@base-ui/react/switch";
import { Tabs } from "@base-ui/react/tabs";
import {
  Activity,
  Copy,
  Gauge,
  Globe,
  Languages,
  Plus,
  RefreshCw,
  ShieldAlert,
  Terminal as TerminalIcon,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import { useCallback, useRef } from "react";
import { type DictKey, t } from "./i18n/dict.ts";
import { type BenchmarkRow, pocStore, type TabMeta, usePocStore } from "./store/pocStore.ts";
import { sessionManager } from "./terminal/sessionManager.ts";
import { TerminalViewport } from "./components/TerminalPane.tsx";
import type { TerminalBackendId } from "./transport/schemas.ts";

const BACKENDS: readonly { id: TerminalBackendId; labelKey: DictKey }[] = [
  { id: "wterm-ghostty-dom", labelKey: "backendWtermDom" },
  { id: "wterm-ghostty-webgl", labelKey: "backendWtermWebgl" },
];

let nextTabCounter = 1;

function readHeapMB(): number | null {
  const perf = performance as Performance & { memory?: { usedJSHeapSize: number } };
  if (!perf.memory) return null;
  return perf.memory.usedJSHeapSize / (1024 * 1024);
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** Isolated status summary so high-frequency stats/IME updates never re-render App or Toolbar. */
function StatusSummary() {
  const locale = usePocStore((s) => s.locale);
  const tabs = usePocStore((s) => s.tabs);
  const activeTabId = usePocStore((s) => s.activeTabId);
  const isComposing = usePocStore((s) => s.isComposing);
  const totalBytesReceived = usePocStore((s) => s.totalBytesReceived);
  const totalRepliesBlocked = usePocStore((s) => s.totalRepliesBlocked);

  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null;

  return (
    <div className="grid grid-cols-2 gap-1 border-b border-zinc-800 p-2 font-mono text-[11px]">
      <div>
        {t(locale, "totalTabs")}: <span className="text-zinc-100">{tabs.length}</span>
      </div>
      <div>
        {t(locale, "colsRows")}:{" "}
        <span className="text-zinc-100">
          {activeTab ? `${activeTab.cols}x${activeTab.rows}` : "-"}
        </span>
      </div>
      <div>
        {t(locale, "bytesReceived")}:{" "}
        <span className="text-zinc-100">{(totalBytesReceived / 1024).toFixed(1)} KB</span>
      </div>
      <div>
        {t(locale, "repliesBlocked")}:{" "}
        <span className="text-amber-300">{totalRepliesBlocked}</span>
      </div>
      <div className="col-span-2">
        <span
          className={
            isComposing
              ? "rounded bg-sky-900/80 px-1.5 py-0.5 text-sky-200"
              : "rounded bg-zinc-800 px-1.5 py-0.5 text-zinc-400"
          }
        >
          {isComposing ? t(locale, "imeComposing") : t(locale, "imeIdle")}
        </span>
      </div>
    </div>
  );
}

/** Isolated benchmark results table. */
function BenchmarkPanel() {
  const locale = usePocStore((s) => s.locale);
  const benchResults = usePocStore((s) => s.benchResults);

  return (
    <div className="border-b border-zinc-800 p-2">
      <div className="mb-1 font-semibold text-zinc-300">{t(locale, "runBench")}</div>
      {benchResults.length === 0 ? (
        <p className="text-[11px] text-zinc-500">{t(locale, "noBenchYet")}</p>
      ) : (
        <table className="w-full font-mono text-[10px]">
          <thead>
            <tr className="border-b border-zinc-800 text-left text-zinc-400">
              <th>N</th>
              <th>Mount</th>
              <th>10k</th>
              <th>Hid</th>
              <th>Sw</th>
              <th>Disp</th>
            </tr>
          </thead>
          <tbody>
            {benchResults.map((r) => (
              <tr key={`${r.backendId}-${r.count}`} className="border-b border-zinc-800/50">
                <td>{r.count}</td>
                <td>{r.mountMs}ms</td>
                <td>{r.burst10kMs}ms</td>
                <td>{r.hiddenBurstMs}ms</td>
                <td>{r.switchAvgMs}ms</td>
                <td>{r.disposeMs}ms</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

/** Isolated diagnostic log panel so per-keystroke IME/input logs never re-render App. */
function DiagnosticLogPanel() {
  const locale = usePocStore((s) => s.locale);
  const logs = usePocStore((s) => s.logs);
  const diagnosticsEnabled = usePocStore((s) => s.diagnosticsEnabled);

  return (
    <>
      <div className="flex items-center justify-between border-b border-zinc-800 px-2 py-1">
        <span className="font-semibold text-zinc-300">{t(locale, "imePanel")}</span>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-1 text-[10px] text-zinc-400">
            <Switch.Root
              checked={diagnosticsEnabled}
              onCheckedChange={(v) => pocStore.getState().setDiagnosticsEnabled(v)}
              className="relative inline-flex h-3.5 w-6 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-2.5 w-2.5 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3" />
            </Switch.Root>
            <span>{t(locale, "diagnosticsLog")}</span>
          </label>
          <button
            type="button"
            onClick={() => pocStore.getState().clearLogs()}
            className="text-[11px] text-zinc-400 hover:text-zinc-200"
          >
            {t(locale, "clearLog")}
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-2 font-mono text-[10px] leading-relaxed">
        {logs.map((entry) => (
          <div key={entry.id} className="truncate">
            <span className="text-zinc-500">{entry.time}</span>{" "}
            <span
              className={
                entry.kind === "auto-reply"
                  ? "text-amber-300"
                  : entry.kind === "ime"
                    ? "text-sky-300"
                    : entry.kind === "input"
                      ? "text-emerald-300"
                      : "text-zinc-400"
              }
            >
              [{entry.kind}]
            </span>{" "}
            <span className="text-zinc-200">{entry.detail}</span>
          </div>
        ))}
      </div>
    </>
  );
}

/** Isolated tab strip so tab/size updates do not re-render toolbar or log panel. */
function TabBar({ onCloseTab }: { onCloseTab: (id: string) => void }) {
  const tabs = usePocStore((s) => s.tabs);
  const activeTabId = usePocStore((s) => s.activeTabId);
  const benchRunning = usePocStore((s) => s.benchRunning);

  return (
    <Tabs.Root
      value={activeTabId ?? ""}
      onValueChange={(value) => {
        if (typeof value === "string" && value && !benchRunning) {
          pocStore.getState().setActiveTabId(value);
        }
      }}
      className="border-b border-zinc-800 bg-zinc-900/60"
    >
      <Tabs.List className="flex items-center gap-1 overflow-x-auto px-2 py-1">
        {tabs.map((tab) => (
          <div key={tab.id} className="flex items-center">
            <Tabs.Tab
              value={tab.id}
              disabled={benchRunning}
              className="rounded-l px-2.5 py-1 text-zinc-400 hover:bg-zinc-800 disabled:opacity-50 data-[selected]:bg-zinc-800 data-[selected]:text-emerald-300"
            >
              {tab.title} ({tab.cols}x{tab.rows})
            </Tabs.Tab>
            <button
              type="button"
              disabled={benchRunning}
              aria-label={`Close ${tab.title}`}
              onClick={() => onCloseTab(tab.id)}
              className="rounded-r px-1 py-1 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        ))}
      </Tabs.List>
    </Tabs.Root>
  );
}

export function App() {
  const locale = usePocStore((s) => s.locale);
  const backendId = usePocStore((s) => s.backendId);
  const suppressAutoReplies = usePocStore((s) => s.suppressAutoReplies);
  const pauseHiddenRendering = usePocStore((s) => s.pauseHiddenRendering);
  const benchRunning = usePocStore((s) => s.benchRunning);

  const hostRef = useRef<HTMLDivElement | null>(null);
  const initializedRef = useRef(false);
  const tabGenerationRef = useRef(0);

  const spawnTabs = useCallback(async (count: number, replace = false) => {
    const host = hostRef.current;
    if (!host) return;
    const state = pocStore.getState();

    if (replace) {
      tabGenerationRef.current++;
      sessionManager.disposeAll();
      state.setTabs([], null);
      state.resetStats();
    }

    const generation = tabGenerationRef.current;
    const created: TabMeta[] = [];

    for (let i = 0; i < count; i++) {
      const id = `term-${nextTabCounter++}`;
      const tab: TabMeta = { id, title: id, cols: 80, rows: 24 };
      const isFirst = pocStore.getState().tabs.length === 0 && i === 0;
      try {
        await sessionManager.createSession(id, pocStore.getState().backendId, host, !isFirst);
        if (generation !== tabGenerationRef.current) return;
        created.push(tab);
      } catch {
        // Error is already recorded in diagnostic log by sessionManager.
        break;
      }
    }

    if (generation !== tabGenerationRef.current) return;
    const current = pocStore.getState();
    const allTabs = [...current.tabs, ...created];
    const nextActive = replace
      ? (created[0]?.id ?? null)
      : (created[created.length - 1]?.id ?? current.activeTabId);
    current.setTabs(allTabs, nextActive);
    sessionManager.setActive(nextActive);
  }, []);

  const handleHostReady = useCallback(
    (el: HTMLDivElement | null) => {
      hostRef.current = el;
      if (!el) {
        // React StrictMode reattaches the same host before the microtask runs.
        queueMicrotask(() => {
          if (!hostRef.current) {
            tabGenerationRef.current++;
            initializedRef.current = false;
            sessionManager.stopEventStream();
            sessionManager.disposeAll();
          }
        });
      } else if (!initializedRef.current) {
        initializedRef.current = true;
        void sessionManager.startEventStream();
        void spawnTabs(1, true);
      }
    },
    [spawnTabs],
  );

  const handleBackendChange = async (nextBackend: TerminalBackendId) => {
    pocStore.getState().setBackendId(nextBackend);
    const count = Math.max(1, pocStore.getState().tabs.length);
    await spawnTabs(count, true);
  };

  const handleCloseTab = (id: string) => {
    sessionManager.disposeSession(id);
    const current = pocStore.getState();
    const remaining = current.tabs.filter((tab) => tab.id !== id);
    const nextActive =
      current.activeTabId === id ? (remaining[remaining.length - 1]?.id ?? null) : current.activeTabId;
    current.setTabs(remaining, nextActive);
    sessionManager.setActive(nextActive);
  };

  const handleCloseAll = () => {
    tabGenerationRef.current++;
    sessionManager.disposeAll();
    pocStore.getState().setTabs([], null);
    pocStore.getState().resetStats();
  };

  const getActiveSession = () => {
    const id = pocStore.getState().activeTabId;
    return id ? sessionManager.get(id) : undefined;
  };

  const handleRunBenchmark = async () => {
    const host = hostRef.current;
    if (!host || pocStore.getState().benchRunning) return;
    tabGenerationRef.current++;
    pocStore.getState().setBenchRunning(true);
    pocStore.getState().clearBenchResults();
    const currentBackend = pocStore.getState().backendId;

    try {
      for (const count of [1, 5, 10]) {
        sessionManager.disposeAll();
        pocStore.getState().setTabs([], null);
        await nextFrame();
        const heapBefore = readHeapMB();

        const tMount0 = performance.now();
        const ids: string[] = [];
        for (let i = 0; i < count; i++) {
          const id = `bench-${count}-${i}`;
          ids.push(id);
          await sessionManager.createSession(id, currentBackend, host, i !== 0);
        }
        await nextFrame();
        const mountMs = performance.now() - tMount0;

        // Active terminal 10,000-line burst
        const burst = sessionManager.injectBurst(ids[0]!, 10_000, 200);
        await nextFrame();
        const burstMBps =
          burst.durationMs > 0 ? burst.totalBytes / (1024 * 1024) / (burst.durationMs / 1000) : 0;

        // Hidden terminal 10,000-line burst (when count > 1)
        let hiddenBurstMs = 0;
        if (count > 1) {
          const hb = sessionManager.injectBurst(ids[1]!, 10_000, 200);
          await nextFrame();
          hiddenBurstMs = hb.durationMs;
        }

        // Tab switch latency (10 switches)
        const switchTimes: number[] = [];
        if (count > 1) {
          for (let i = 0; i < 10; i++) {
            const targetId = ids[i % count]!;
            const tSw0 = performance.now();
            sessionManager.setActive(targetId);
            await nextFrame();
            switchTimes.push(performance.now() - tSw0);
          }
        }
        const switchAvgMs =
          switchTimes.length > 0
            ? switchTimes.reduce((a, b) => a + b, 0) / switchTimes.length
            : 0;

        const heapAfter = readHeapMB();

        const tDisp0 = performance.now();
        sessionManager.disposeAll();
        await nextFrame();
        const disposeMs = performance.now() - tDisp0;

        const row: BenchmarkRow = {
          backendId: currentBackend,
          count,
          mountMs: Number(mountMs.toFixed(1)),
          burst10kMs: Number(burst.durationMs.toFixed(1)),
          burstMBps: Number(burstMBps.toFixed(1)),
          hiddenBurstMs: Number(hiddenBurstMs.toFixed(1)),
          switchAvgMs: Number(switchAvgMs.toFixed(2)),
          disposeMs: Number(disposeMs.toFixed(1)),
          heapDeltaMB:
            heapBefore !== null && heapAfter !== null
              ? Number((heapAfter - heapBefore).toFixed(2))
              : null,
        };
        pocStore.getState().addBenchResult(row);
      }
      // Restore 1 active terminal after benchmark
      await spawnTabs(1, true);
    } catch (err) {
      for (const count of [1, 5, 10]) {
        for (let i = 0; i < count; i++) sessionManager.disposeSession(`bench-${count}-${i}`);
      }
      pocStore.getState().pushLog("system", `Benchmark aborted: ${String(err)}`);
    } finally {
      pocStore.getState().setBenchRunning(false);
    }
  };

  return (
    <div className="flex h-screen w-screen flex-col bg-zinc-950 text-xs text-zinc-100">
      {/* Top Toolbar */}
      <header className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900 px-3 py-1.5">
        <div className="flex items-center gap-1.5 font-semibold text-zinc-100">
          <TerminalIcon className="h-4 w-4 text-emerald-400" />
          <span>{t(locale, "appTitle")}</span>
          <span
            className={
              sessionManager.runtimeMode === "tauri"
                ? "rounded bg-emerald-900/80 px-1.5 py-0.5 font-mono text-[10px] text-emerald-200"
                : "rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400"
            }
          >
            {sessionManager.runtimeMode === "tauri" ? "tauri+herdr-core" : "mock"}
          </span>
        </div>

        <label className="flex items-center gap-1.5">
          <span className="text-zinc-400">{t(locale, "backend")}:</span>
          <select
            data-testid="backend-select"
            disabled={benchRunning}
            value={backendId}
            onChange={(e) => void handleBackendChange(e.target.value as TerminalBackendId)}
            className="rounded border border-zinc-700 bg-zinc-800 px-2 py-1 text-zinc-100 disabled:opacity-50"
          >
            {BACKENDS.map((b) => (
              <option key={b.id} value={b.id}>
                {t(locale, b.labelKey)}
              </option>
            ))}
          </select>
        </label>

        <div className="flex items-center gap-1 border-l border-zinc-800 pl-2">
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(1, false)}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            <Plus className="h-3.5 w-3.5" />
            {t(locale, "addTab")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(1, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "spawn1")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(5, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "spawn5")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(10, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "spawn10")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={handleCloseAll}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 text-red-300 hover:bg-zinc-700 disabled:opacity-50"
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t(locale, "closeAll")}
          </button>
        </div>

        <div className="flex items-center gap-1 border-l border-zinc-800 pl-2">
          <button
            type="button"
            onClick={() => {
              const id = pocStore.getState().activeTabId;
              if (id) sessionManager.injectJapaneseTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Languages className="h-3.5 w-3.5 text-sky-400" />
            {t(locale, "testJapanese")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = pocStore.getState().activeTabId;
              if (id) sessionManager.injectVtTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Activity className="h-3.5 w-3.5 text-violet-400" />
            {t(locale, "testVt")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = pocStore.getState().activeTabId;
              if (id) sessionManager.injectQueryProbe(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <ShieldAlert className="h-3.5 w-3.5 text-amber-400" />
            {t(locale, "testQuery")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = pocStore.getState().activeTabId;
              if (id) void sessionManager.resyncSession(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <RefreshCw className="h-3.5 w-3.5 text-emerald-400" />
            {t(locale, "testResync")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = pocStore.getState().activeTabId;
              if (!id) return;
              const res = sessionManager.injectBurst(id, 10_000, 200);
              pocStore
                .getState()
                .pushLog(
                  "system",
                  `burst 10k lines: ${(res.totalBytes / 1024).toFixed(1)} KB in ${res.durationMs.toFixed(1)} ms`,
                );
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Zap className="h-3.5 w-3.5 text-yellow-400" />
            {t(locale, "testBurst")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void handleRunBenchmark()}
            className="flex items-center gap-1 rounded bg-emerald-700 px-2 py-1 font-medium text-white hover:bg-emerald-600 disabled:opacity-50"
          >
            <Gauge className="h-3.5 w-3.5" />
            {benchRunning ? t(locale, "benchRunning") : t(locale, "runBench")}
          </button>
        </div>

        <div className="ml-auto flex items-center gap-3">
          <label className="flex items-center gap-1.5 text-zinc-300">
            <Switch.Root
              checked={suppressAutoReplies}
              onCheckedChange={(v) => pocStore.getState().setSuppressAutoReplies(v)}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
            <span>{t(locale, "suppressReplies")}</span>
          </label>

          <label className="flex items-center gap-1.5 text-zinc-300">
            <Switch.Root
              checked={pauseHiddenRendering}
              onCheckedChange={(v) => {
                pocStore.getState().setPauseHiddenRendering(v);
                sessionManager.applyHiddenPausePolicy(v, pocStore.getState().activeTabId);
              }}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
            <span>{t(locale, "pauseHidden")}</span>
          </label>

          <button
            type="button"
            onClick={async () => {
              const sel = await getActiveSession()?.adapter.getSelection();
              if (sel) {
                await navigator.clipboard?.writeText(sel);
                pocStore.getState().pushLog("system", `copied ${sel.length} chars`);
              }
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Copy className="h-3.5 w-3.5" />
            {t(locale, "copySelection")}
          </button>

          <button
            type="button"
            onClick={() => pocStore.getState().setLocale(locale === "ja" ? "en" : "ja")}
            className="flex items-center gap-1 rounded border border-zinc-700 px-2 py-1 hover:bg-zinc-800"
          >
            <Globe className="h-3.5 w-3.5" />
            {t(locale, "localeToggle")}
          </button>
        </div>
      </header>

      {/* Base UI Tabs for Terminal Switching */}
      <TabBar onCloseTab={handleCloseTab} />

      {/* Main Split: Terminal Viewport + Diagnostic / Benchmark Sidebar */}
      <div className="flex min-h-0 flex-1">
        <TerminalViewport onHostReady={handleHostReady} />

        <aside className="flex w-80 flex-col border-l border-zinc-800 bg-zinc-900/70">
          <StatusSummary />
          <BenchmarkPanel />
          <DiagnosticLogPanel />
        </aside>
      </div>
    </div>
  );
}
