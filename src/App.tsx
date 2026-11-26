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
import { appRuntime } from "./app/runtime.ts";
import { t, type TranslationKey } from "./i18n/index.ts";
import { BenchmarkPanel, DiagnosticLogPanel, StatusSummary } from "./poc/PocPanels.tsx";
import { type BenchmarkRow, pocStore, usePocStore } from "./poc/pocStore.ts";
import { appStore, useAppStore } from "./state/appStore.ts";
import { TerminalViewport } from "./ui/TerminalViewport.tsx";
import type { TerminalBackendId } from "./validation/settingsSchemas.ts";

const BACKENDS: readonly { id: TerminalBackendId; labelKey: TranslationKey }[] = [
  { id: "wterm-ghostty-dom", labelKey: "poc.backendWtermDom" },
  { id: "wterm-ghostty-webgl", labelKey: "poc.backendWtermWebgl" },
];

function readHeapMB(): number | null {
  const perf = performance as Performance & { memory?: { usedJSHeapSize: number } };
  if (!perf.memory) return null;
  return perf.memory.usedJSHeapSize / (1024 * 1024);
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** Isolated tab strip wired to `appStore` and `appRuntime.actions`. */
function TabBar() {
  const tabs = useAppStore((s) => s.tabs);
  const panes = useAppStore((s) => s.panes);
  const activeWorkspace = useAppStore((s) =>
    s.selection.activeWorkspaceId ? s.workspaces[s.selection.activeWorkspaceId] ?? null : null,
  );
  const activeTabId = useAppStore((s) => s.selection.activeTabId);
  const benchRunning = usePocStore((s) => s.benchRunning);

  const tabList = (activeWorkspace?.tabIds ?? [])
    .map((id) => tabs[id])
    .filter((tab): tab is NonNullable<typeof tab> => Boolean(tab));

  return (
    <Tabs.Root
      value={activeTabId ?? ""}
      onValueChange={(value) => {
        if (typeof value === "string" && value && !benchRunning) {
          void appRuntime.actions.execute("tab.select", { tabId: value }, { caller: "gui" });
        }
      }}
      className="border-b border-zinc-800 bg-zinc-900/60"
    >
      <Tabs.List className="flex items-center gap-1 overflow-x-auto px-2 py-1">
        {tabList.map((tab) => {
          const pane = tab.activePaneId ? panes[tab.activePaneId] : undefined;
          const cols = pane?.cols ?? 80;
          const rows = pane?.rows ?? 24;
          return (
            <div key={tab.id} className="flex items-center">
              <Tabs.Tab
                value={tab.id}
                disabled={benchRunning}
                className="rounded-l px-2.5 py-1 text-zinc-400 hover:bg-zinc-800 disabled:opacity-50 data-[selected]:bg-zinc-800 data-[selected]:text-emerald-300"
              >
                {tab.title} ({cols}x{rows})
              </Tabs.Tab>
              <button
                type="button"
                disabled={benchRunning}
                aria-label={`Close ${tab.title}`}
                onClick={() => {
                  void appRuntime.actions.execute(
                    "terminal.close",
                    { tabId: tab.id },
                    { caller: "gui" },
                  );
                }}
                className="rounded-r px-1 py-1 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          );
        })}
      </Tabs.List>
    </Tabs.Root>
  );
}

export function App() {
  const locale = useAppStore((s) => s.settings.locale);
  const renderer = useAppStore((s) => s.settings.renderer);
  const suppressAutoReplies = useAppStore((s) => s.settings.suppressAutoReplies);
  const pauseHiddenRendering = useAppStore((s) => s.settings.pauseHiddenRendering);
  const sidebarVisible = useAppStore((s) => s.ui.sidebarVisible);
  const benchRunning = usePocStore((s) => s.benchRunning);

  const initializedRef = useRef(false);
  const tabGenerationRef = useRef(0);
  const benchInFlightRef = useRef(false);

  const spawnTabs = useCallback(async (count: number, replace = false) => {
    const host = appRuntime.getMountRoot();
    if (!host) return;

    if (replace) {
      tabGenerationRef.current++;
      appRuntime.sessionManager.disposeAll();
      appStore.getState().clearWorkspaceTabs();
      pocStore.getState().resetStats();
    }

    const generation = tabGenerationRef.current;

    for (let i = 0; i < count; i++) {
      if (generation !== tabGenerationRef.current) return;
      const activate = replace ? i === 0 : i === count - 1;
      const result = await appRuntime.actions.execute(
        "terminal.create",
        { activate },
        { caller: "gui" },
      );
      if (!result.ok) {
        break;
      }
    }
  }, []);

  const handleHostReady = useCallback(
    (el: HTMLDivElement | null) => {
      appRuntime.setMountRoot(el);
      if (!el) {
        // React StrictMode reattaches the same host before the microtask runs.
        queueMicrotask(() => {
          if (!appRuntime.getMountRoot()) {
            tabGenerationRef.current++;
            initializedRef.current = false;
            appRuntime.sessionManager.stopEventStream();
            appRuntime.sessionManager.disposeAll();
            appStore.getState().clearWorkspaceTabs();
          }
        });
      } else if (!initializedRef.current) {
        initializedRef.current = true;
        void appRuntime.sessionManager.startEventStream();
        void spawnTabs(1, true);
      }
    },
    [spawnTabs],
  );

  const handleBackendChange = async (nextBackend: TerminalBackendId) => {
    await appRuntime.actions.execute("app.setRenderer", { renderer: nextBackend }, { caller: "gui" });
    const count = Math.max(1, Object.keys(appStore.getState().tabs).length);
    await spawnTabs(count, true);
  };

  const handleCloseAll = () => {
    tabGenerationRef.current++;
    appRuntime.sessionManager.disposeAll();
    appStore.getState().clearWorkspaceTabs();
    pocStore.getState().resetStats();
  };

  const handleRunBenchmark = async () => {
    const host = appRuntime.getMountRoot();
    if (!host || benchInFlightRef.current || pocStore.getState().benchRunning) return;
    benchInFlightRef.current = true;
    tabGenerationRef.current++;
    pocStore.getState().setBenchRunning(true);
    pocStore.getState().clearBenchResults();
    const currentBackend = appStore.getState().settings.renderer;
    let completed = false;

    try {
      try {
        for (const count of [1, 5, 10]) {
          appRuntime.sessionManager.disposeAll();
          appStore.getState().clearWorkspaceTabs();
          await nextFrame();
          const heapBefore = readHeapMB();

          const tMount0 = performance.now();
          const ids: string[] = [];
          for (let i = 0; i < count; i++) {
            const id = `bench-${count}-${i}`;
            ids.push(id);
            await appRuntime.sessionManager.createSession(id, currentBackend, host, i !== 0);
          }
          await nextFrame();
          const mountMs = performance.now() - tMount0;

          const burst = appRuntime.sessionManager.injectBurst(ids[0]!, 10_000, 200);
          await nextFrame();
          const burstMBps =
            burst.durationMs > 0 ? burst.totalBytes / (1024 * 1024) / (burst.durationMs / 1000) : 0;

          let hiddenBurstMs = 0;
          if (count > 1) {
            const hb = appRuntime.sessionManager.injectBurst(ids[1]!, 10_000, 200);
            await nextFrame();
            hiddenBurstMs = hb.durationMs;
          }

          const switchTimes: number[] = [];
          if (count > 1) {
            for (let i = 0; i < 10; i++) {
              const targetId = ids[i % count]!;
              const tSw0 = performance.now();
              appRuntime.sessionManager.setActive(targetId);
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
          appRuntime.sessionManager.disposeAll();
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
        completed = true;
      } catch (err) {
        for (const count of [1, 5, 10]) {
          for (let i = 0; i < count; i++) {
            appRuntime.sessionManager.disposeSession(`bench-${count}-${i}`);
          }
        }
        pocStore.getState().pushLog("system", `Benchmark aborted: ${String(err)}`);
      } finally {
        pocStore.getState().setBenchRunning(false);
      }

      if (completed) {
        await spawnTabs(1, true);
      }
    } finally {
      benchInFlightRef.current = false;
    }
  };

  return (
    <div className="flex h-screen w-screen flex-col bg-zinc-950 text-xs text-zinc-100">
      {/* Top Toolbar (Phase 1 Core Actions + Retained Phase 0 Diagnostics) */}
      <header className="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900 px-3 py-1.5">
        <div className="flex items-center gap-1.5 font-semibold text-zinc-100">
          <TerminalIcon className="h-4 w-4 text-emerald-400" />
          <span>{t(locale, "app.title")}</span>
          <span className="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-300">
            {t(locale, "app.phaseBadge")}
          </span>
          <span
            className={
              appRuntime.sessionManager.runtimeMode === "tauri"
                ? "rounded bg-emerald-900/80 px-1.5 py-0.5 font-mono text-[10px] text-emerald-200"
                : "rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400"
            }
          >
            {appRuntime.sessionManager.runtimeMode === "tauri"
              ? t(locale, "app.runtimeTauri")
              : t(locale, "app.runtimeMock")}
          </span>
        </div>

        <label className="flex items-center gap-1.5">
          <span className="text-zinc-400">{t(locale, "poc.backend")}:</span>
          <select
            data-testid="backend-select"
            disabled={benchRunning}
            value={renderer}
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
            {t(locale, "poc.addTab")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(1, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn1")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(5, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn5")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void spawnTabs(10, true)}
            className="rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn10")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={handleCloseAll}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 text-red-300 hover:bg-zinc-700 disabled:opacity-50"
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t(locale, "poc.closeAll")}
          </button>
        </div>

        <div className="flex items-center gap-1 border-l border-zinc-800 pl-2">
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectJapaneseTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Languages className="h-3.5 w-3.5 text-sky-400" />
            {t(locale, "poc.testJapanese")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectVtTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Activity className="h-3.5 w-3.5 text-violet-400" />
            {t(locale, "poc.testVt")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectQueryProbe(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <ShieldAlert className="h-3.5 w-3.5 text-amber-400" />
            {t(locale, "poc.testQuery")}
          </button>
          <button
            type="button"
            onClick={() => {
              void appRuntime.actions.execute("terminal.resync", {}, { caller: "gui" });
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <RefreshCw className="h-3.5 w-3.5 text-emerald-400" />
            {t(locale, "poc.testResync")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (!id) return;
              const res = appRuntime.sessionManager.injectBurst(id, 10_000, 200);
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
            {t(locale, "poc.testBurst")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void handleRunBenchmark()}
            className="flex items-center gap-1 rounded bg-emerald-700 px-2 py-1 font-medium text-white hover:bg-emerald-600 disabled:opacity-50"
          >
            <Gauge className="h-3.5 w-3.5" />
            {benchRunning ? t(locale, "poc.benchRunning") : t(locale, "poc.runBench")}
          </button>
        </div>

        <div className="ml-auto flex items-center gap-3">
          <label className="flex items-center gap-1.5 text-zinc-300">
            <Switch.Root
              checked={suppressAutoReplies}
              onCheckedChange={(v) => appStore.getState().updateSettings({ suppressAutoReplies: v })}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
            <span>{t(locale, "poc.suppressReplies")}</span>
          </label>

          <label className="flex items-center gap-1.5 text-zinc-300">
            <Switch.Root
              checked={pauseHiddenRendering}
              disabled={benchRunning}
              onCheckedChange={(v) => {
                appStore.getState().updateSettings({ pauseHiddenRendering: v });
                appRuntime.sessionManager.applyHiddenPausePolicy(
                  v,
                  appStore.getState().selection.activePaneId,
                );
              }}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 disabled:opacity-50 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
            <span>{t(locale, "poc.pauseHidden")}</span>
          </label>

          <button
            type="button"
            onClick={async () => {
              const res = await appRuntime.actions.execute(
                "terminal.copySelection",
                {},
                { caller: "gui" },
              );
              if (res.ok && res.value.copiedLength > 0) {
                pocStore.getState().pushLog("system", `copied ${res.value.copiedLength} chars`);
              }
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-1 hover:bg-zinc-700"
          >
            <Copy className="h-3.5 w-3.5" />
            {t(locale, "poc.copySelection")}
          </button>

          <button
            type="button"
            onClick={() => {
              void appRuntime.actions.execute(
                "app.setLocale",
                { locale: locale === "ja" ? "en" : "ja" },
                { caller: "gui" },
              );
            }}
            className="flex items-center gap-1 rounded border border-zinc-700 px-2 py-1 hover:bg-zinc-800"
          >
            <Globe className="h-3.5 w-3.5" />
            {t(locale, "app.localeToggle")}
          </button>
        </div>
      </header>

      <TabBar />

      <div className="flex min-h-0 flex-1">
        <TerminalViewport onHostReady={handleHostReady} />

        {sidebarVisible ? (
          <aside className="flex w-80 flex-col border-l border-zinc-800 bg-zinc-900/70">
            <StatusSummary />
            <BenchmarkPanel />
            <DiagnosticLogPanel />
          </aside>
        ) : null}
      </div>
    </div>
  );
}
