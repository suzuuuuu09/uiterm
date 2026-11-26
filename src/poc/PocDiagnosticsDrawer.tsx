import { Switch } from "@base-ui/react/switch";
import {
  Activity,
  Copy,
  Gauge,
  Languages,
  RefreshCw,
  ShieldAlert,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { appRuntime } from "../app/runtime.ts";
import { t, type TranslationKey } from "../i18n/index.ts";
import { appStore, useAppStore } from "../state/appStore.ts";
import { DEFAULT_PRIMARY_FONT_FAMILY } from "../terminal/fonts.ts";
import type { TerminalBackendId } from "../validation/settingsSchemas.ts";
import { BenchmarkPanel, DiagnosticLogPanel, StatusSummary } from "./PocPanels.tsx";
import { type BenchmarkRow, pocStore, usePocStore } from "./pocStore.ts";

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

export interface PocDiagnosticsDrawerProps {
  readonly onSpawnTabs: (count: number, replace?: boolean) => Promise<void>;
  readonly onCloseAll: () => void;
}

/**
 * Developer-only Phase 0 diagnostic & benchmark drawer.
 * Hidden by default in normal product mode; toggled via `app.toggleDiagnostics`
 * (Cmd+Shift+D / Ctrl+Shift+D) or `?poc=1`.
 */
export function PocDiagnosticsDrawer({ onSpawnTabs, onCloseAll }: PocDiagnosticsDrawerProps) {
  const locale = useAppStore((s) => s.settings.locale);
  const renderer = useAppStore((s) => s.settings.renderer);
  const fontFamily = useAppStore((s) => s.settings.fontFamily);
  const fontSize = useAppStore((s) => s.settings.fontSize);
  const lineHeight = useAppStore((s) => s.settings.lineHeight);
  const nerdFontFallback = useAppStore((s) => s.settings.nerdFontFallback);
  const activePaneId = useAppStore((s) => s.selection.activePaneId);
  const suppressAutoReplies = useAppStore((s) => s.settings.suppressAutoReplies);
  const pauseHiddenRendering = useAppStore((s) => s.settings.pauseHiddenRendering);
  const benchRunning = usePocStore((s) => s.benchRunning);

  const [fontInput, setFontInput] = useState(fontFamily);
  const benchInFlightRef = useRef(false);

  useEffect(() => {
    setFontInput(fontFamily);
  }, [fontFamily]);

  const cellMetrics = activePaneId ? appRuntime.sessionManager.getCellMetrics(activePaneId) : null;

  const applyFont = (patch: {
    fontFamily?: string;
    fontSize?: number;
    lineHeight?: number;
    nerdFontFallback?: boolean;
  }) => {
    void appRuntime.actions.execute("app.setFont", patch, { caller: "gui" });
  };

  const handleBackendChange = async (nextBackend: TerminalBackendId) => {
    await appRuntime.actions.execute(
      "app.setRenderer",
      { renderer: nextBackend },
      { caller: "gui" },
    );
    const count = Math.max(1, Object.keys(appStore.getState().tabs).length);
    await onSpawnTabs(count, true);
  };

  const handleRunBenchmark = async () => {
    const host = appRuntime.getMountRoot();
    if (!host || benchInFlightRef.current || pocStore.getState().benchRunning) return;
    benchInFlightRef.current = true;
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
        await onSpawnTabs(1, true);
      }
    } finally {
      benchInFlightRef.current = false;
    }
  };

  return (
    <aside
      data-testid="poc-diagnostics-drawer"
      className="flex w-80 flex-col border-l border-zinc-800 bg-zinc-900/95 text-xs text-zinc-100"
    >
      <div className="flex items-center justify-between border-b border-zinc-800 px-2.5 py-1.5">
        <div className="flex items-center gap-1.5">
          <span className="font-semibold text-zinc-200">{t(locale, "poc.imePanel")}</span>
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
        <button
          type="button"
          aria-label={t(locale, "action.app.toggleDiagnostics.title")}
          onClick={() => {
            void appRuntime.actions.execute("app.toggleDiagnostics", {}, { caller: "gui" });
          }}
          className="rounded p-0.5 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Developer controls */}
      <div className="flex flex-col gap-2 border-b border-zinc-800 p-2">
        <label className="flex items-center justify-between gap-2">
          <span className="text-zinc-400">{t(locale, "poc.backend")}</span>
          <select
            data-testid="backend-select"
            disabled={benchRunning}
            value={renderer}
            onChange={(e) => void handleBackendChange(e.target.value as TerminalBackendId)}
            className="rounded border border-zinc-700 bg-zinc-800 px-1.5 py-0.5 text-[11px] text-zinc-100 disabled:opacity-50"
          >
            {BACKENDS.map((b) => (
              <option key={b.id} value={b.id}>
                {t(locale, b.labelKey)}
              </option>
            ))}
          </select>
        </label>

        <div className="flex flex-wrap items-center gap-1">
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void onSpawnTabs(1, true)}
            className="rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn1")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void onSpawnTabs(5, true)}
            className="rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn5")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void onSpawnTabs(10, true)}
            className="rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700 disabled:opacity-50"
          >
            {t(locale, "poc.spawn10")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={onCloseAll}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] text-red-300 hover:bg-zinc-700 disabled:opacity-50"
          >
            <Trash2 className="h-3 w-3" />
            {t(locale, "poc.closeAll")}
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1">
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectJapaneseTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <Languages className="h-3 w-3 text-sky-400" />
            {t(locale, "poc.testJapanese")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectVtTest(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <Activity className="h-3 w-3 text-violet-400" />
            {t(locale, "poc.testVt")}
          </button>
          <button
            type="button"
            onClick={() => {
              const id = appStore.getState().selection.activePaneId;
              if (id) appRuntime.sessionManager.injectQueryProbe(id);
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <ShieldAlert className="h-3 w-3 text-amber-400" />
            {t(locale, "poc.testQuery")}
          </button>
          <button
            type="button"
            onClick={() => {
              void appRuntime.actions.execute("terminal.resync", {}, { caller: "gui" });
            }}
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <RefreshCw className="h-3 w-3 text-emerald-400" />
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
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <Zap className="h-3 w-3 text-yellow-400" />
            {t(locale, "poc.testBurst")}
          </button>
          <button
            type="button"
            disabled={benchRunning}
            onClick={() => void handleRunBenchmark()}
            className="flex items-center gap-1 rounded bg-emerald-700 px-2 py-0.5 text-[11px] font-medium text-white hover:bg-emerald-600 disabled:opacity-50"
          >
            <Gauge className="h-3 w-3" />
            {benchRunning ? t(locale, "poc.benchRunning") : t(locale, "poc.runBench")}
          </button>
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
            className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] hover:bg-zinc-700"
          >
            <Copy className="h-3 w-3" />
            {t(locale, "poc.copySelection")}
          </button>
        </div>

        <div className="flex flex-col gap-1.5 text-[11px]">
          <label className="flex items-center justify-between text-zinc-300">
            <span>{t(locale, "poc.suppressReplies")}</span>
            <Switch.Root
              checked={suppressAutoReplies}
              onCheckedChange={(v) => appStore.getState().updateSettings({ suppressAutoReplies: v })}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
          </label>

          <label className="flex items-center justify-between text-zinc-300">
            <span>{t(locale, "poc.pauseHidden")}</span>
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
          </label>
        </div>

        {/* Developer Font Verification (Phase 2 internal API check; full user settings UI is Phase 4) */}
        <div className="flex flex-col gap-1.5 border-t border-zinc-800 pt-2 text-[11px]">
          <div className="flex items-center justify-between">
            <span className="font-semibold text-zinc-300">{t(locale, "poc.fontVerify")}</span>
            <span className="font-mono text-[10px] text-zinc-400">
              {t(locale, "poc.cellMetrics")}:{" "}
              {cellMetrics
                ? `${cellMetrics.charWidth.toFixed(1)}x${cellMetrics.rowHeight.toFixed(1)}px`
                : "-"}
            </span>
          </div>

          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={() => applyFont({ fontFamily: DEFAULT_PRIMARY_FONT_FAMILY })}
              className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] hover:bg-zinc-700"
            >
              {t(locale, "poc.fontPresetDefault")}
            </button>
            <button
              type="button"
              onClick={() => applyFont({ fontFamily: '"Menlo"' })}
              className="rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] hover:bg-zinc-700"
            >
              {t(locale, "poc.fontPresetMenlo")}
            </button>
          </div>

          <form
            className="flex items-center gap-1"
            onSubmit={(e) => {
              e.preventDefault();
              const trimmed = fontInput.trim();
              if (trimmed) applyFont({ fontFamily: trimmed });
            }}
          >
            <input
              type="text"
              aria-label={t(locale, "poc.fontFamily")}
              value={fontInput}
              onChange={(e) => setFontInput(e.target.value)}
              placeholder='"UDEV Gothic 35NF", "Menlo"'
              className="min-w-0 flex-1 rounded border border-zinc-700 bg-zinc-950 px-1.5 py-0.5 font-mono text-[10px] text-zinc-100"
            />
            <button
              type="submit"
              className="rounded bg-zinc-800 px-2 py-0.5 text-[10px] hover:bg-zinc-700"
            >
              {t(locale, "poc.fontApply")}
            </button>
          </form>

          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1 text-zinc-400">
              <span>{t(locale, "poc.fontSize")}</span>
              <input
                type="number"
                min={8}
                max={48}
                step={0.5}
                value={fontSize}
                onChange={(e) => {
                  const val = Number(e.target.value);
                  if (Number.isFinite(val) && val >= 8 && val <= 48) {
                    applyFont({ fontSize: val });
                  }
                }}
                className="w-14 rounded border border-zinc-700 bg-zinc-950 px-1 py-0.5 font-mono text-[10px] text-zinc-100"
              />
            </label>
            <label className="flex items-center gap-1 text-zinc-400">
              <span>{t(locale, "poc.lineHeight")}</span>
              <input
                type="number"
                min={1.0}
                max={2.0}
                step={0.05}
                value={lineHeight}
                onChange={(e) => {
                  const val = Number(e.target.value);
                  if (Number.isFinite(val) && val >= 1.0 && val <= 2.0) {
                    applyFont({ lineHeight: val });
                  }
                }}
                className="w-14 rounded border border-zinc-700 bg-zinc-950 px-1 py-0.5 font-mono text-[10px] text-zinc-100"
              />
            </label>
          </div>

          <label className="flex items-center justify-between text-zinc-300">
            <span>{t(locale, "poc.nerdFontFallback")}</span>
            <Switch.Root
              checked={nerdFontFallback}
              onCheckedChange={(v) => applyFont({ nerdFontFallback: v })}
              className="relative inline-flex h-4 w-7 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-3 w-3 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3.5" />
            </Switch.Root>
          </label>
        </div>
      </div>

      <StatusSummary />
      <BenchmarkPanel />
      <DiagnosticLogPanel />
    </aside>
  );
}
