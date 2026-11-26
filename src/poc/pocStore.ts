import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { TerminalBackendId } from "../validation/settingsSchemas.ts";

export interface DiagnosticEvent {
  readonly id: number;
  readonly time: string;
  readonly kind: "ime" | "input" | "auto-reply" | "system";
  readonly detail: string;
}

export interface BenchmarkRow {
  readonly backendId: TerminalBackendId;
  readonly count: number;
  readonly mountMs: number;
  readonly burst10kMs: number;
  readonly burstMBps: number;
  readonly hiddenBurstMs: number;
  readonly switchAvgMs: number;
  readonly disposeMs: number;
  readonly heapDeltaMB: number | null;
}

export interface PocDiagnosticsState {
  /**
   * Explicit opt-in for verbose Phase 0 PoC diagnostics (disabled by default in product mode
   * so terminal input metadata / IME events are not recorded unless requested).
   */
  diagnosticsEnabled: boolean;
  totalBytesReceived: number;
  totalRepliesBlocked: number;
  logs: readonly DiagnosticEvent[];
  benchRunning: boolean;
  benchResults: readonly BenchmarkRow[];

  setDiagnosticsEnabled: (value: boolean) => void;
  addStats: (bytesDelta: number, repliesDelta: number) => void;
  resetStats: () => void;
  pushLog: (kind: DiagnosticEvent["kind"], detail: string) => void;
  clearLogs: () => void;
  setBenchRunning: (running: boolean) => void;
  addBenchResult: (row: BenchmarkRow) => void;
  clearBenchResults: () => void;
}

let nextLogId = 1;
const MAX_LOGS = 80;
const MAX_LOG_DETAIL_CHARS = 160;

function nowTime(): string {
  const d = new Date();
  return `${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/**
 * Isolated PoC-only store for Phase 0 benchmarks and diagnostic probes.
 * Product state lives exclusively in `src/state/appStore.ts`.
 */
export const pocStore = createStore<PocDiagnosticsState>((set) => ({
  diagnosticsEnabled: false,
  totalBytesReceived: 0,
  totalRepliesBlocked: 0,
  logs: [],
  benchRunning: false,
  benchResults: [],

  setDiagnosticsEnabled: (diagnosticsEnabled) => set({ diagnosticsEnabled }),
  addStats: (bytesDelta, repliesDelta) =>
    set((s) => ({
      totalBytesReceived: s.totalBytesReceived + bytesDelta,
      totalRepliesBlocked: s.totalRepliesBlocked + repliesDelta,
    })),
  resetStats: () => set({ totalBytesReceived: 0, totalRepliesBlocked: 0 }),
  pushLog: (kind, detail) =>
    set((s) => {
      if (!s.diagnosticsEnabled && kind !== "system") {
        return s;
      }
      const bounded =
        detail.length > MAX_LOG_DETAIL_CHARS
          ? `${detail.slice(0, MAX_LOG_DETAIL_CHARS - 3)}...`
          : detail;
      const next: DiagnosticEvent = { id: nextLogId++, time: nowTime(), kind, detail: bounded };
      const merged = [...s.logs, next];
      return { logs: merged.length > MAX_LOGS ? merged.slice(merged.length - MAX_LOGS) : merged };
    }),
  clearLogs: () => set({ logs: [] }),
  setBenchRunning: (benchRunning) => set({ benchRunning }),
  addBenchResult: (row) => set((s) => ({ benchResults: [...s.benchResults, row] })),
  clearBenchResults: () => set({ benchResults: [] }),
}));

export function usePocStore<T>(selector: (state: PocDiagnosticsState) => T): T {
  return useStore(pocStore, selector);
}
