import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { Locale } from "../i18n/dict.ts";
import type { TerminalBackendId } from "../transport/schemas.ts";

export interface TabMeta {
  readonly id: string;
  readonly title: string;
  readonly cols: number;
  readonly rows: number;
}

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

export interface PocState {
  locale: Locale;
  backendId: TerminalBackendId;
  tabs: readonly TabMeta[];
  activeTabId: string | null;
  suppressAutoReplies: boolean;
  pauseHiddenRendering: boolean;
  isComposing: boolean;
  totalBytesReceived: number;
  totalRepliesBlocked: number;
  logs: readonly DiagnosticEvent[];
  benchRunning: boolean;
  benchResults: readonly BenchmarkRow[];

  setLocale: (locale: Locale) => void;
  setBackendId: (backendId: TerminalBackendId) => void;
  setTabs: (tabs: readonly TabMeta[], activeTabId: string | null) => void;
  setActiveTabId: (id: string | null) => void;
  updateTabSize: (id: string, cols: number, rows: number) => void;
  setSuppressAutoReplies: (value: boolean) => void;
  setPauseHiddenRendering: (value: boolean) => void;
  setIsComposing: (value: boolean) => void;
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

function nowTime(): string {
  const d = new Date();
  return `${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/**
 * Zustand Vanilla store for UI & workspace metadata ONLY.
 * Terminal output bytes NEVER enter this store.
 */
export const pocStore = createStore<PocState>((set) => ({
  locale: "ja",
  backendId: "wterm-ghostty-dom",
  tabs: [],
  activeTabId: null,
  suppressAutoReplies: true,
  pauseHiddenRendering: true,
  isComposing: false,
  totalBytesReceived: 0,
  totalRepliesBlocked: 0,
  logs: [],
  benchRunning: false,
  benchResults: [],

  setLocale: (locale) => set({ locale }),
  setBackendId: (backendId) => set({ backendId }),
  setTabs: (tabs, activeTabId) => set({ tabs, activeTabId }),
  setActiveTabId: (activeTabId) => set({ activeTabId }),
  updateTabSize: (id, cols, rows) =>
    set((s) => ({
      tabs: s.tabs.map((tab) => (tab.id === id ? { ...tab, cols, rows } : tab)),
    })),
  setSuppressAutoReplies: (suppressAutoReplies) => set({ suppressAutoReplies }),
  setPauseHiddenRendering: (pauseHiddenRendering) => set({ pauseHiddenRendering }),
  setIsComposing: (isComposing) => set({ isComposing }),
  addStats: (bytesDelta, repliesDelta) =>
    set((s) => ({
      totalBytesReceived: s.totalBytesReceived + bytesDelta,
      totalRepliesBlocked: s.totalRepliesBlocked + repliesDelta,
    })),
  resetStats: () => set({ totalBytesReceived: 0, totalRepliesBlocked: 0 }),
  pushLog: (kind, detail) =>
    set((s) => {
      const next: DiagnosticEvent = { id: nextLogId++, time: nowTime(), kind, detail };
      const merged = [...s.logs, next];
      return { logs: merged.length > MAX_LOGS ? merged.slice(merged.length - MAX_LOGS) : merged };
    }),
  clearLogs: () => set({ logs: [] }),
  setBenchRunning: (benchRunning) => set({ benchRunning }),
  addBenchResult: (row) => set((s) => ({ benchResults: [...s.benchResults, row] })),
  clearBenchResults: () => set({ benchResults: [] }),
}));

export function usePocStore<T>(selector: (state: PocState) => T): T {
  return useStore(pocStore, selector);
}
