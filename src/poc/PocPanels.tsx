import { Switch } from "@base-ui/react/switch";
import { formatBytes, t } from "../i18n/index.ts";
import { useAppStore } from "../state/appStore.ts";
import { pocStore, usePocStore } from "./pocStore.ts";

/** Isolated status summary so high-frequency stats/IME updates never re-render App or Toolbar. */
export function StatusSummary() {
  const locale = useAppStore((s) => s.settings.locale);
  const tabCount = useAppStore((s) => Object.keys(s.tabs).length);
  const activePane = useAppStore((s) =>
    s.selection.activePaneId ? (s.panes[s.selection.activePaneId] ?? null) : null,
  );
  const isComposing = useAppStore((s) => s.ui.isComposing);
  const totalBytesReceived = usePocStore((s) => s.totalBytesReceived);
  const totalRepliesBlocked = usePocStore((s) => s.totalRepliesBlocked);

  return (
    <div className="grid grid-cols-2 gap-1 border-b border-zinc-800 p-2 font-mono text-[11px]">
      <div>
        {t(locale, "poc.totalTabs")}: <span className="text-zinc-100">{tabCount}</span>
      </div>
      <div>
        {t(locale, "poc.colsRows")}:{" "}
        <span className="text-zinc-100">
          {activePane ? `${activePane.cols}x${activePane.rows}` : "-"}
        </span>
      </div>
      <div>
        {t(locale, "poc.bytesReceived")}:{" "}
        <span className="text-zinc-100">{formatBytes(locale, totalBytesReceived)}</span>
      </div>
      <div>
        {t(locale, "poc.repliesBlocked")}:{" "}
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
          {isComposing ? t(locale, "poc.imeComposing") : t(locale, "poc.imeIdle")}
        </span>
      </div>
    </div>
  );
}

/** Isolated benchmark results table (Phase 0 PoC tooling). */
export function BenchmarkPanel() {
  const locale = useAppStore((s) => s.settings.locale);
  const benchResults = usePocStore((s) => s.benchResults);

  return (
    <div className="border-b border-zinc-800 p-2">
      <div className="mb-1 font-semibold text-zinc-300">{t(locale, "poc.runBench")}</div>
      {benchResults.length === 0 ? (
        <p className="text-[11px] text-zinc-500">{t(locale, "poc.noBenchYet")}</p>
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

/** Isolated diagnostic log panel (opt-in verbose metadata only; no typed/IME text). */
export function DiagnosticLogPanel() {
  const locale = useAppStore((s) => s.settings.locale);
  const logs = usePocStore((s) => s.logs);
  const diagnosticsEnabled = usePocStore((s) => s.diagnosticsEnabled);

  return (
    <>
      <div className="flex items-center justify-between border-b border-zinc-800 px-2 py-1">
        <span className="font-semibold text-zinc-300">{t(locale, "poc.imePanel")}</span>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-1 text-[10px] text-zinc-400">
            <Switch.Root
              checked={diagnosticsEnabled}
              onCheckedChange={(v) => pocStore.getState().setDiagnosticsEnabled(v)}
              className="relative inline-flex h-3.5 w-6 items-center rounded-full bg-zinc-700 data-[checked]:bg-emerald-600"
            >
              <Switch.Thumb className="block h-2.5 w-2.5 translate-x-0.5 rounded-full bg-white transition-transform data-[checked]:translate-x-3" />
            </Switch.Root>
            <span>{t(locale, "poc.diagnosticsLog")}</span>
          </label>
          <button
            type="button"
            onClick={() => pocStore.getState().clearLogs()}
            className="text-[11px] text-zinc-400 hover:text-zinc-200"
          >
            {t(locale, "poc.clearLog")}
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
