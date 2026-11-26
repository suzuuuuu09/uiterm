export type Locale = "ja" | "en";

const ja = {
  appTitle: "uiterm Phase 0 PoC",
  backend: "描画エンジン",
  addTab: "タブ追加",
  spawn1: "1個",
  spawn5: "5個",
  spawn10: "10個",
  closeAll: "全破棄",
  testJapanese: "日本語・文字幅",
  testVt: "VT/ANSI",
  testQuery: "自動応答テスト",
  testResync: "再同期",
  testBurst: "大量出力 (10k行)",
  runBench: "性能測定",
  benchRunning: "測定中...",
  imePanel: "IME / 自動応答ログ",
  clearLog: "クリア",
  suppressReplies: "自動応答を抑制",
  pauseHidden: "非表示時の描画停止",
  copySelection: "選択コピー",
  focusTerm: "フォーカス",
  localeToggle: "EN",
  activeTab: "アクティブ",
  totalTabs: "タブ数",
  colsRows: "サイズ",
  bytesReceived: "受信バイト",
  repliesBlocked: "抑制応答",
  imeComposing: "IME変換中",
  imeIdle: "IME待機",
  noBenchYet: "「性能測定」を押すと1/5/10端末・大量出力・切替・破棄を計測します。",
  backendWtermDom: "wterm + libghostty (DOM)",
  backendWtermWebgl: "wterm + libghostty (WebGL)",
  diagnosticsLog: "診断ログ記録",
} as const;

export type DictKey = keyof typeof ja;

const en: Record<DictKey, string> = {
  appTitle: "uiterm Phase 0 PoC",
  backend: "Renderer",
  addTab: "New Tab",
  spawn1: "1",
  spawn5: "5",
  spawn10: "10",
  closeAll: "Dispose All",
  testJapanese: "JP / Width",
  testVt: "VT / ANSI",
  testQuery: "Auto-Reply",
  testResync: "Resync",
  testBurst: "Burst (10k)",
  runBench: "Benchmark",
  benchRunning: "Running...",
  imePanel: "IME / Auto-Reply Log",
  clearLog: "Clear",
  suppressReplies: "Suppress Auto-Replies",
  pauseHidden: "Pause Hidden Paint",
  copySelection: "Copy Selection",
  focusTerm: "Focus",
  localeToggle: "JA",
  activeTab: "Active",
  totalTabs: "Tabs",
  colsRows: "Grid",
  bytesReceived: "Bytes RX",
  repliesBlocked: "Replies Blocked",
  imeComposing: "IME Composing",
  imeIdle: "IME Idle",
  noBenchYet: "Click Benchmark to measure 1/5/10 terminals, burst throughput, switching, and disposal.",
  backendWtermDom: "wterm + libghostty (DOM)",
  backendWtermWebgl: "wterm + libghostty (WebGL)",
  diagnosticsLog: "Log Diagnostics",
};

const dictionaries: Record<Locale, Record<DictKey, string>> = { ja, en };

export function t(locale: Locale, key: DictKey): string {
  return dictionaries[locale][key];
}
