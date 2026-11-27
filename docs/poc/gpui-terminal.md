---
description: "ADR-0010 GPUI PoC（Stage 1〜3）実装・描画最適化・3者性能比較・互換性・NixOS/Hyprland調査レポート"
---

# GPUI ターミナル PoC（Stage 1〜3）検証レポート

本ドキュメントは、[ADR-0010](../adr/0010-evaluate-native-gpui-terminal.md) に基づきブランチ `poc/gpui-terminal`（基準コミット `5d2ac57`）で実施した **GPUI PoC（Stage 1〜3）** の実装内容、描画最適化前後の検証、`baseline/phase2-tauri`（`5ef4abb`）およびネイティブ Ghostty（`1.4.0`）との同一条件リリースビルド比較、日本語 IME・互換性回帰検証、NixOS / Hyprland 調査結果、ならびに正式採用判断のための材料をまとめたものである。

> [!IMPORTANT]
> 本 PoC は評価専用の隔離コード（`poc/gpui-terminal/`）であり、Phase 2 製品コードと `baseline/phase2-tauri` は変更していない。`flake.nix` には PoC 用の `gpui` 開発シェルを追加した。また、自動テストで検証した項目と、実機の日本語 IME / キーボード / トラックパッドでユーザーが確認すべき未検証項目、ならびに Linux 実機未検証（コード/Flake 静的調査）を明確に区別して記録する。

---

## 1. 実装内容（Stage 1〜3）

Stage 1・2 で検証した GPUI フォントシェーピング・IME 状態管理・ネイティブ `libghostty-vt` ベンチマークを基盤とし、Stage 3 では実際に `fish`・Neovim・Codex 等の対話型 CLI を操作できるライブターミナルと最適化レンダラを `poc/gpui-terminal/` に実装した。

| モジュール | 主な責務・実装記号 |
| --- | --- |
| [`poc/gpui-terminal/src/main.rs`](../../poc/gpui-terminal/src/main.rs) | `--uiterm-pty-init` 最速エントリ、シグナル監視（`SIGTERM`/`SIGINT` self-pipe による `herdr-core` クリーンアップ）、`LiveTerminalView`、PTY 非同期ポンプ、`sync_resize_if_needed`、`TerminalInputHandler`（`apple_press_and_hold_enabled() -> false`）、`BenchScenario` / `--bench-window`、`GpuiPocView` |
| [`poc/gpui-terminal/src/renderer.rs`](../../poc/gpui-terminal/src/renderer.rs) | `RowShapeCache`（`RenderState::next_dirty()` + `RowId` + `hash_styled_cells`、`MAX_CACHED_ROWS = 128`）、`shape_extracted_row`（複数 `TextRun` 統合、一様背景行右端延長、SGR 2/8/7）、`paint_terminal_frame`（選択ハイライト、カーソル描画、IME オーバーレイ、スクロールバー） |
| [`poc/gpui-terminal/src/vt.rs`](../../poc/gpui-terminal/src/vt.rs) | `VtTerminal`（`ghostty-vt 0.9.0` / `libghostty-vt 1.0.1`、2,000 行 / 10 MiB 履歴、`DEFAULT_TERMINAL_THEME`）、`Osc133Normalizer`（`redraw=0` 補完）、Mode 2026（`render_hold` + 100ms 解除）、Mode 2048（リサイズ応答）、`KeyEncoder`/`MouseEncoder`/`paste::encode`/`SelectionGesture`、`--bench-vt` |
| [`poc/gpui-terminal/src/herdr.rs`](../../poc/gpui-terminal/src/herdr.rs) | `herdr-core` v0.1.22（`CORE_VERSION`）隔離起動、`DirCleanupGuard`、`HerdrHandle`（`FRAME_DATA` と `FRAME_AUTOMATIC_INPUT` の厳密分離）、`HerdrOutputStream`（VT 処理後の `FRAME_CREDIT` 返却）、`CloseInfo`、`HerdrGuard::stop_blocking` |
| [`poc/gpui-terminal/src/input.rs`](../../poc/gpui-terminal/src/input.rs) | `classify_keydown`・`match_app_action`・`gpui_key_to_ghostty`（通常印字キー/Space は IME (`NSTextInputContext`) へ委譲、`Cmd` 各種 / `Option-as-Meta` / Ctrl / 特殊キー分離、IME 変換中ショートカット抑止） |
| [`poc/gpui-terminal/src/ime.rs`](../../poc/gpui-terminal/src/ime.rs) | `TerminalImeState`（Cmd+Tab / focus-blur 間の未確定文字列維持、`Commit` 直接返却、`"あ"` → `"あ"` 連続確定、Esc/unmark 安全消去、`--probe-ui` 用 `ImeBuffer` 維持） |
| [`poc/gpui-terminal/src/os.rs`](../../poc/gpui-terminal/src/os.rs) | `PTY_IUTF8_INIT_FLAG`、`enable_pty_iutf8`、`wrap_argv_with_pty_iutf8_init`、`resolve_os_default_shell`（`getpwuid_r`）、`build_child_shell_env`（隔離 XDG の子シェル復元） |
| [`poc/gpui-terminal/src/fonts.rs`](../../poc/gpui-terminal/src/fonts.rs) | `FontPreset`（Default / `UDEV Gothic 35NF` / Menlo）、`build_terminal_font_styled`、Unicode / PUA / 結合文字 / ZWJ セル幅計算 |
| [`poc/gpui-terminal/tests/compatibility.rs`](../../poc/gpui-terminal/tests/compatibility.rs) | 実 `herdr-core` + `--uiterm-pty-init` + `fish` + `nvim` + `codex` 互換性統合テスト |

### 主要な設計ポイント

1. **ユーザー入力と自動応答の分離**: `VtTerminal::feed` が返した VT 自動応答（DA1・DSR・OSC 等）および `resize()` 時の Mode 2048 レポートは `HerdrHandle::try_send_automatic`（`FRAME_AUTOMATIC_INPUT = 6`）のみで送信し、未交渉時の `FRAME_DATA` フォールバックを行わない。
2. **リサイズ順序保証と OSC 133**: `LiveTerminalView::sync_resize_if_needed` でセル metrics から `cols x rows` を算出し、まず `FRAME_RESIZE` を送り、その後 `FRAME_AUTOMATIC_INPUT`（Mode 2048）を送出する。出力ストリームには `Osc133Normalizer` を通し、`;redraw=` 省略時の `;redraw=0` を補完する。
3. **macOS IME / キー入力ルーティング**: GPUI `0.2.2`（`src/platform/mac/window.rs`）のディスパッチ順序に合わせ、`on_key_down` で単純印字キー（未修飾または Shift のみの文字・Space）に対して `cx.stop_propagation()` を呼ばず `NSTextInputContext.handleEvent:` へ流すことで、ASCII 確定と macSKK / 日本語 IME 変換の両立を実現している。また `TerminalInputHandler` で `apple_press_and_hold_enabled() -> false` とし、Vim/Neovim でのキーリピート（`j`/`k` 等）を有効化した。
4. **安全なプロセス終了**: 各インスタンスを `/tmp/u-gp-<tag>-<pid>-<seq>` に隔離し、既定の Herdr ソケットを使用せず、他インスタンスを停止しない。ウィンドウ終了・`Cmd+Q`・`SIGTERM` / `SIGINT` いずれの経路でも当該セッションの `herdr-core` / `holder` と一時ディレクトリを確実に後始末する。

---

## 2. 測定環境と条件

### ハードウェア・OS・構成バージョン

- **環境**: Apple M3 / 16 GiB RAM / macOS 15.5 (`24F74`) / Retina 2560 × 1664
- **対象ビルド**: すべてリリースビルドで比較
  - GPUI PoC: `uiterm-gpui-poc` (`opt-level=3`, `lto="thin"`, `codegen-units=1`, `strip=true`), `gpui 0.2.2`, `ghostty-vt 0.9.0` (`libghostty-vt 1.0.1`), `herdr-core 0.1.22`
  - Tauri Baseline: ブランチ `baseline/phase2-tauri`（`5ef4abb`）から一時 detached worktree でビルドした `uiterm.app` + `herdr-core 0.1.22`（`@wterm/dom 0.6.0`, `@wterm/ghostty 0.6.0`, `Ghostty WASM v1.3.1`）
  - Ghostty: インストール済み `/Applications/Ghostty.app` **v1.4.0**（Zig 0.17.0, CoreText, Metal, kqueue）
- **統一ベンチマーク条件**:
  - ウィンドウ/端末サイズ: GPUI `--bench-window` は `106×32`。ライブウィンドウ起動時は OS のタイリング/ウィンドウサイズ制約により GPUI `84×48`・Tauri `82×46`・Ghostty `85×53` へ同程度の面積に自動リサイズされた（`stty size` で実測記録）。
  - フォント・サイズ・履歴: `Menlo` 14px（Tauri `5ef4abb` は Phase 2 既定の `ui-monospace, "SF Mono", "Menlo", "JetBrains Mono"` 14px / 1000×700 ウィンドウ）、スクロールバック上限 2,000 行（Ghostty は `config-default-files = false`, `font-family = Menlo`, `font-size = 14`, `window-width = 106`, `window-height = 32`, `scrollback-limit-lines = 2000`, `background-opacity = 1`, `background-blur = false`, `shell-integration = none`, `window-save-state = never` で隔離設定）。
  - 隔離環境: 全アプリの通常計測で `HOME`・`XDG_*`・`HC_*` を隔離（`macism` および `nvim --startuptime` のみユーザーの実際の `~/.config/nvim` を読み取り専用で測定し、ログは `/tmp` 一時ファイルへ出力）。起動中の既存ユーザー Ghostty（PID `1670`）および既存 `herdr-core` には一切干渉していない。

> [!NOTE]
> **VT 解析単体性能と実画面描画性能の分離**: ヘッドレス `libghostty-vt` スループット（`--bench-vt` / `scripts/bench-terminals.mjs`）と、CoreText シェーピング・Metal GPU/WKWebView 描画を含む実ウィンドウ性能を別節で明確に分離して報告する。

---

## 3. 描画性能の最適化と前後比較（Step 2）

### 3.1 初期実装（v1）の検証で判明した逆行と原因

最適化の効果を検証するため、`--render-mode uncached` と `--render-mode cached`（`RenderState::next_dirty()` + `RowId` キャッシュ + `hash_styled_cells`）を `106×32`・Menlo 14px・履歴 2,000 行・各 3 試行で測定した。

1. **v1 キャッシュ（`MAX_CACHED_ROWS=256`・スパン毎 `ShapedLine`・Alternate Screen も `by_row_id` 使用）で見つかった問題**:
   - `libghostty-vt` の仕様確認テスト（`dirty_tracking_and_row_id_behave_consistently_across_partial_scroll_and_selection`）により、スクロール発生時に **`RowId` はそのまま維持される一方で `meta.dirty = Dirty::Full` および各行の `row.dirty() = true` が立つ** こと、また `\rEDITED\r\nL7` のように同フレーム内で書き換え後にスクロールした場合も `RowId` が引き継がれることが判明。`RowId` のみに頼ると古い文字を残すバグになるため、全セルの内容＋スタイルを 64-bit FNV-1a でハッシュ検証（`hash_styled_cells`）する設計を採用した。
   - **GPUI `ShapedLine` 構造体のメモリ特性**: `gpui 0.2.2`（`src/text_system/line.rs`）の `ShapedLine` は `SmallVec<[DecorationRun; 32]>` をインライン保持しており **1 インスタンスあたり約 3.1 KiB** を消費する。トークン/色変化ごとに `ShapedLine` を分割していた v1 では、`scroll-stream` のキャッシュ推定サイズが **15,587.4 KiB** に膨らんだ。
   - **`nvim-ctrl-du`（`Screen::Alternate`）での性能・メモリ逆行**: `uncached` の `prepaint` 中央値 `964 µs`・GPUI RSS `68.69 MiB` に対し、v1 `cached` は `1,394 µs`（**+44.6% 悪化**）・GPUI RSS `99.23 MiB`（**+30.5 MiB 増**）となった。Neovim の `Ctrl+D`/`Ctrl+U` では行が `Screen::Alternate` 上で書き換わるため `Dirty::Full` + ヒット率 `3.0%` となり、`by_row_id` ハッシュ探索と 256 エントリの `ShapedLine` 保持が純粋なオーバーヘッドになっていた。

### 3.2 測定結果に基づく改良設計（v2）

実測に基づいて以下の簡素化と最適化（v2）を実施した。

1. **同一行内の複数 `TextRun` 統合**: 連続する ASCII セルおよび単一コードポイント BMP CJK 文字を色・属性変化を跨いで 1 本の `ShapedLine`（`Vec<TextRun>`、`force_width` 指定でグリッド整列保証）へ統合し、1 インライン約 3.1 KiB の `ShapedLine` 生成数を約 75〜80% 削減。一方、絵文字・ZWJ・結合文字・半角カナ・PUA Nerd Font アイコンは個別セル配置を維持し、文字崩れと累積ズレを防止。
2. **`Screen::Alternate` の `Dirty::Full` キャッシュ無効化と容量上限の引き下げ**: `MAX_CACHED_ROWS` を `256` → **`128`**（32 行画面×4 画面分）へ縮小し、`by_row_id` + `hash_styled_cells` はスクロール履歴が機能する `Screen::Primary` のみに限定。`Screen::Alternate` の `Dirty::Full`（`Ctrl+D`/`Ctrl+U` 等）では `by_row_id` を参照・登録せず直接 `viewport_rows` を更新する。なお、`Screen::Primary`/`Alternate` 双方で `Dirty::Clean` と `Dirty::Partial`（`RenderState::next_dirty()`）は常に有効。
3. **無効化条件**: `CacheConfigKey`（フォントプリセット、Nerd Font fallback、フォントサイズ、行間、ディスプレイ倍率、`cols×rows`、`Screen::{Primary, Alternate}`、色/パレットハッシュ `colors_hash`）の変化時に `by_row_id` と `viewport_rows` を全破棄。さらに `selection_generation` 変化時に選択範囲のみ更新し、128 行超過時は世代順 LRU で破棄する（`renderer::tests::cache_eviction_bounds_memory_and_clears_on_config_change` で検証）。

### 3.3 最適化前後比較（v2・各 3 試行の中央値、106×32 Menlo 14px 履歴 2,000 行）

※ `Burst5MiB` は v1 計測時にコマンドエコー内の `GPUI_BURST_END` 文字列と自己一致し `0.4 KiB` で早期終了するベンチマーク上の不具合があったため、v2 で `printf '%s%s\n' GPUI_ BURST_END` に修正し実データ `5,250.6 KiB` で再計測した。

| シナリオ | 方式 | `prepaint` p50 / p95 (µs) | `shape_line` 回数 | キャッシュヒット率 | 推定キャッシュ上限 (KiB) | フレーム間隔 p95 / >16.67ms (n) | RSS 合計 (MiB) | 主な効果・備考 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `single-row`（1行連続更新） | `uncached` | 856 / 1,122 | 56,160 | 0.0% | 0.0 | 18.65 ms / 179 (360) | 67.47 | 全 32 行を毎回シェーピング |
| `single-row`（同） | `cached` | **233 / 295** | **515** | **96.6%** | 3.43 | 18.65 ms / 170 (360) | 62.59 | **`prepaint` -72.8% / シェーピング -99.1%** |
| `scroll-stream`（ライブ出力流） | `uncached` | 1,132 / 1,499 | 53,940 | 0.0% | 0.0 | 18.66 ms / 168 (360) | 84.48 | 毎フレーム全行再生成 |
| `scroll-stream`（同） | `cached` | **810 / 1,078** | **1,864** | **93.2%** | 1,997.27 | 18.67 ms / 169 (360) | 82.81 | **`prepaint` -28.4% / シェーピング -96.5%** |
| `scroll-history-1`（履歴 ±1行） | `uncached` | 1,012 / 1,512 | 34,551 | 0.0% | 0.0 | 18.67 ms / 172 (360) | 68.86 | 996.1 KiB (10k行) 履歴 |
| `scroll-history-1`（同） | `cached` | **473 / 879** | **681** | **98.0%** | 1,232.50 | 18.66 ms / 175 (360) | 67.75 | **`prepaint` -53.3% / シェーピング -98.0%** |
| `scroll-history-6`（履歴 ±6行） | `uncached` | 1,394 / 1,864 | 34,551 | 0.0% | 0.0 | 18.64 ms / 179 (360) | 69.22 | 各フレーム 6 行新規出現 |
| `scroll-history-6`（同） | `cached` | **1,297 / 1,859** | **6,036** | **82.5%** | 1,232.50 | 18.62 ms / 174 (360) | 71.06 | **シェーピング -82.5%**, `prepaint` -7.0% |
| `nvim-ctrl-du`（AltScreen 60往復） | `uncached` | 1,390 / 1,605 | 12,840 | 0.0% | 0.0 | 18.61 ms / 55 (120) | 72.84 | アクションRTT中央値 3.97 ms |
| `nvim-ctrl-du`（同） | `cached` | **1,381 / 1,625** | 12,635 | 1.6% | **0.25** | 18.66 ms / 58 (120) | 73.52 | **v1 の +45% 遅延/+30 MiB 悪化を解消** |
| `burst-5mib`（5,250.6 KiB 流入） | `uncached` | 482 / 10,843 | 6,306 | 0.0% | 0.0 | 29.11 ms / 11 (22) | 87.05 | `aux_ms = 414.91 ms` |
| `burst-5mib`（同） | `cached` | 557 / 11,095 | 6,306 | 4.1% | 3,909.90 | 29.93 ms / 9 (22) | 92.14 | `aux_ms = 422.59 ms`（全行更新のため差なし） |
| `input-latency`（100回同期打鍵） | `uncached` | — | — | — | 0.0 | — | 47.36 | PTY+VT+prepare p50 **0.14 ms** / p95 **0.17 ms** |
| `input-latency`（同） | `cached` | — | — | — | 0.0 | — | 46.50 | PTY+VT+prepare p50 **0.06 ms (-57%)** / p95 **0.09 ms** |

### 最適化の採否判断

測定結果から、以下を **採用** し、不要な複雑化は **不採用** とした:
- **採用**: (1) `TextRun` 統合による `ShapedLine` 削減（全シナリオでシェーピング回数 60〜80% 減・メモリ大幅削減）、(2) `RenderState::next_dirty()`（入力レイテンシ p50 を 0.14ms→0.06ms、部分更新 `prepaint` を -72.8% 短縮）、(3) `Screen::Primary` 限定・128 行上限の `RowId` + `content_hash` キャッシュ（スクロール時の CoreText シェーピングを -82〜-98% 削減、キャッシュメモリ 1.2〜2.0 MiB 以内）。
- **不採用**: `Screen::Alternate` の `Dirty::Full` に対する `RowId` ハッシュキャッシュは +44.6% 遅延と +30.5 MiB RSS 悪化を招き効果がマイナスであったため不採用とした。なお、`prepaint` 時間は 0.2〜1.3 ms と 16.67 ms (60Hz VSync) 予算に対して十分小さいため、`frame_interval` 中央値は全条件で VSync 律速（`16.67 ms`）となっているが、省電力・CPU 負荷抑制の観点で最適化維持が妥当である。

---

## 4. 3者比較（GPUI PoC vs `baseline/phase2-tauri` `5ef4abb` vs Ghostty `1.4.0`）

### 4.1 GUI 実装・システム性能比較（リリースビルド）

| 比較項目 | GPUI PoC（Stage 3 `cached`） | `baseline/phase2-tauri`（`5ef4abb`） | Ghostty `1.4.0` |
| --- | --- | --- | --- |
| **実端末サイズ** (`stty size`) | 84 列 × 48 行（ベンチ時 106×32） | 82 列 × 46 行（1000×700 ウィンドウ） | 85 列 × 53 行（設定 106×32） |
| **1. 起動時間: Window 可視化** (3 試行中央値) | 211.19 ms (`196.39〜223.40`) | 178.94 ms (`169.32〜192.65`) | 294.41 ms (`283.60〜297.22`, `open -n`) |
| **1'. シェルコマンド実行準備完了** | **167.03 ms** | 476.15 ms（WKWebView+WASM初期化待ち） | 248.11 ms |
| **2a. アイドル時 Physical Footprint** | **71.12 MiB** | 121.47 MiB | 126.69 MiB |
| **2a'. アイドル時 合計 RSS** | **88.58 MiB** | 245.55 MiB (WebKit 3プロセス含) | 128.59 MiB |
| **2a''. アイドル時 CPU 使用率** (5s 積算) | 6.26% | 3.33% | **1.38%** |
| **2b. 30秒 256 KiB/s 負荷ピーク Footprint** | 154.51 MiB（終了1.5s後 **93.74 MiB**） | 435.44 MiB（終了1.5s後 307.80 MiB） | **144.76 MiB**（終了1.5s後 128.83 MiB） |
| **2b'. 同 ピーク RSS** | 156.45 MiB | 437.47 MiB (`WebContent` 260.42 MiB) | **155.28 MiB** |
| **2b''. 同 負荷平均 CPU 使用率** | 6.62% (+0.36pt) | 27.30% (+23.97pt) | **5.20%** (+3.82pt) |
| **3. 入力→画面準備遅延** (100 回同期) | **p50 0.06 ms / p95 0.09 ms / p99 13.23 ms** (PTY+VT+shape; 実表示は VSync 16.67ms 制約) | 外部非改変バイナリからは内部描画完了時刻を非侵入計測不可（※制約注記） | 同左（外部非改変バイナリから内部フレーム完了時刻は非計測） |
| **4. Neovim `Ctrl+D`/`Ctrl+U`** (60 往復) | 総計 **2,342.82 ms** (`120 frames`, アクションRTT **p50 3.97ms / p95 5.30ms**, frame **p95 18.66ms**) | 未自動計測（`docs/status.md` で「追従性の改善がまだ十分でない場合あり」として残存記録） | 本計測ではキー合成送信を見送り未計測 |
| **5. 大量出力 5 MiB PTY ドレイン** (3 試行) | 116.00 ms（描画含む 5.13 MiB 完走 `422.59 ms`） | 128.15 ms（30s 負荷で WebContent 260 MiB 増） | **113.84 ms** |
| **6. スクロールフレーム間隔** (360 フレーム) | `scroll-stream`: **p50 16.66 / p95 18.67 / p99 18.73 ms**<br>`scroll-history-1`: **p50 16.67 / p95 18.66 / p99 18.87 ms**<br>(超過 `>25ms`: それぞれ 3/360) | Phase 2 参考値 (`e05fd66`, `scrollTop` 23px 減算): **p95 17–18 ms / max 19 ms** (`>25ms`: 0/360。JS計測で WebKit 合成時間を除く) | 外部非改変バイナリのためフレーム単位間隔は未計測 |
| **7a. 前景端末内 `macism` 実行時間** (4 試行) | **29.23 ms** (`[40.56, 30.01, 25.64, 28.45]`) | **4,556.96 ms** (`[5123.53, 4465.66, 4648.26, 1645.07]`) | 39.28 ms (`[52.40, 39.64, 38.93, 36.13]`) |
| **7b. 実設定 Neovim 起動・`:qa` 終了** (実 TTY, 4 回) | Wall **97.33 ms** (`--startuptime` **28.17 ms**) | Wall **88.69 ms** (`--startuptime` **28.79 ms**) | Wall 106.79 ms (`--startuptime` **28.23 ms**) |
| **8. 配布容量（論理バイト）** | 単体 **6,312,432 B (6.02 MiB)**<br>`herdr-core` (6.30 MiB) 合算 **12,915,664 B (12.32 MiB)** | `.app` **3,833,876 B (3.66 MiB)**<br>`herdr-core` 合算 **10,437,108 B (9.95 MiB)**（※OS WebKit 利用） | `.app` **67,299,199 B (64.18 MiB)**<br>バイナリ **45,165,232 B (43.07 MiB)** |

**補足分析**:
- **Tauri のメモリと負荷時 CPU**: 30 秒間の 256 KiB/s ストリーミングで Tauri 版は `WebContent` 単体が `260.42 MiB` まで増え、ピーク Footprint `435.44 MiB`（GPUI の約 **2.8 倍**、Ghostty の約 **3.0 倍**）、負荷 CPU `27.30%`（GPUI の **4.1 倍**）に達し、負荷終了 1.5 秒後も `307.80 MiB`（RSS `420.50 MiB`）が高止まりした。対して GPUI PoC は負荷中 `154.51 MiB`・CPU `6.62%` に収まり、終了 1.5 秒後に `93.74 MiB` まで即座に解放された。
- **`macism` 遅延の再現**: `baseline/phase2-tauri`（`5ef4abb`）では起動直後〜前景状態での `macism` 実行が **1.65〜5.12 秒（中央値 4,556.96 ms）** に悪化する事象を直接再現した。一方、同じ端末・環境でネイティブ AppKit/Metal を持つ GPUI PoC は **29.23 ms**（Ghostty 相当の 39.28 ms）で安定しており、WKWebView 環境とネイティブアプリ環境で入力ソース問い合わせ（TIS）の挙動に大きな差があることが確認された。
- **Neovim 起動と DSR/OSC 11**: 標準入出力をパイプ接続し端末応答が返らない場合、ユーザー実設定の `nvim --startuptime +qa` は `E1568: Terminal did not respond to DSR request for 'background' color`（実時間 約 1.12 秒）を起こす場合があるが、実 PTY 接続した GPUI・Tauri・Ghostty の 3 者ではいずれも警告なし・実時間 `88.7〜106.8 ms`（内部 startup `28.2〜28.8 ms`）で完了した。
- **入力遅延の比較制約**: 基準バイナリ（Tauri `5ef4abb`・Ghostty `1.4.0`）を無改変のまま外部から計測したため、入力から Metal / WebKit 実画面フレーム完了までの正確な内訳は GPUI（`--scenario input-latency` / `nvim-ctrl-du`）でのみ直接取得でき、Tauri・Ghostty との完全同条件レイテンシ比較にはウィンドウ撮影や内部フックが必要になる点は制約として明記する。

### 4.2 ヘッドレス VT 解析性能（実画面描画と分離）

実画面描画とは分離し、同じ 10,000 行ペイロード（`996.1 KiB`、120×40、履歴 2,000 行）を各 VT コアへ投入した純粋パーサ性能は以下。

| 条件 | ネイティブ `libghostty-vt` (`--bench-vt`) | `@wterm/ghostty` WASM (`scripts/bench-terminals.mjs`) | ネイティブ優位比 |
| --- | ---: | ---: | ---: |
| 1 端末生成・書込速度 | **0.01 ms** / **2.52 ms** (**386.7 MiB/s**) | 0.2 ms / 9.5 ms (102.0 MiB/s) | 約 **3.8 倍** |
| 10 端末生成・書込速度 | **0.04 ms** / **2.41 ms** (**403.6 MiB/s**) | 1.2 ms / 9.3 ms (104.8 MiB/s) | 約 **3.9 倍** |
| 10 端末負荷時 RSS 増分 | **+23.50 MiB** | +49.84 MiB | **-52.8%** |

---

## 5. 日本語入力・互換性の回帰検証（Step 4）

### 5.1 検証結果の区別（自動検証 vs 実機手動検証必要）

| 項目 | 自動検証の結果 | 実機手動確認の状況（GUI / 実キーボード） |
| --- | --- | --- |
| **1. `IUTF8` と日本語 Backspace** | `os::tests` 3件 ＋ `tests/compatibility.rs::pty_iutf8_init_enables_utf8_backspace_on_real_herdr_pty` 合格（実 `herdr-core` PTY で `--uiterm-pty-init` 経由の `iutf8` 有効化・`あいうえお`+DEL×5→削除・`A日本`+DEL+`B`→`A日B` 確認） | **未実施（要実機確認）**: ライブウィンドウ内の `stty -a` と実打鍵確認 |
| **2. macSKK 未確定文字と Cmd+Tab 往復** | `ime::tests` 5件合格（`blur`/`focus` 往復で未確定 `▽じぶん` 保持・確定後 `""` + `"自分"` 1回送信・連続 `"あ"`→`"あ"` 確定の回帰テスト） | **未実施（要実機確認）**: 実際の macSKK 2.23+ と `Cmd+Tab` 往復動作 |
| **3. 変換中の Enter・Esc と候補ウィンドウ追従** | `ime::tests` + `input::tests` 3件合格（変換中のショートカット/`Option-as-Meta` 抑止、Esc 取消後 `unmark_text` でフォーカス復帰時に復活しないこと、カーソル基準 `bounds_for_range` 算出） | **未実施（要実機確認）**: 変換中 Enter/Esc、画面中央・右端・最下行での OS/macSKK 候補ウィンドウ位置追従 |
| **4. 日本語・絵文字・結合文字のセル幅** | `fonts::tests::cell_width_aligns_ascii_40_and_cjk_20_and_pua_and_zwj` ＋ `--probe-fonts` 合格（ASCII40桁=`337.148px` と CJK20字=`337.148px` の誤差 `0.000px`、`🦀`/`👨‍👩‍👧‍👦`/`か\u{3099}`/`ｱ` 計測） | **未実施（要実機確認）**: ライブウィンドウ上での目視レイアウト確認 |
| **5. Nerd Font PUA 文字** | `fonts::tests` + `--probe-fonts` 合格（`Symbols Nerd Font Mono` 未導入環境でも `UDEV Gothic 35NF` フォールバックにより `U+E0B0`, `U+E0A0`, `U+E718`, `U+F015`, `U+F024B` 全描画、幅 `8.39px`・`delta=0.000px`） | **未実施（要実機確認）**: fish プロンプト・Neovim アイコンの目視確認 |
| **6. Neovim リサイズと Alternate Screen / CLI 互換** | `tests/compatibility.rs::installed_clis_fish_nvim_codex_and_resize_altscreen_work_in_isolated_pty` 合格（`fish -l` `32 106`、実 `nvim --clean` の `\x1b[?1049h/l`・`FRAME_RESIZE` + Mode 2048 による `106x32` 追従、`codex --version`） | **未実施（要実機確認）**: 対話 Neovim・対話 Codex でのライブ入力/ドラッグリサイズ |
| **7. Bracketed Paste** | `vt::tests::verifies_inverse_color_behavior_and_bracketed_paste_sanitization` 合格（Mode 2004 OFF で `\r` 変換、ON で `\x1b[200~...\x1b[201~` ラップおよび内部 `\x1b` サニタイズ確認） | **未実施（要実機確認）**: 実際のクリップボードからの複数行 `Cmd+V` |
| **8. SGR 2（淡色）・SGR 8（隠し）・SGR 7（反転）背景色** | `vt::tests` + `renderer::tests::resolves_sgr2_faint_sgr8_invisible_and_sgr7_inverse_backgrounds` 合格（SGR 2/8 の `bg_rgb` 保持・SGR 7 の fg/bg 反転確認、一様行右端延長ロジック確認） | **未実施（要実機確認）**: Codex/再現コマンドの画素色差・右端描画の目視確認 |
| **9. 選択中の文字描画とコピー** | `vt::tests`（`RowId`/Selection 結合テストで `selected_text() == Some("L2")` および `row.selection() == Some((0, 1))` 確認）+ `input::tests`（選択有無での `Cmd+C` 分岐・`Ctrl+C` 非奪取）+ `paint_terminal_frame` 内の `end_x - start_x + 1` 幅ハイライト描画実装 | **未実施（要実機確認）**: 実マウスドラッグ/ダブル/トリプルクリック選択と `Cmd+C` コピー中の文字崩れ有無 |

---

## 6. NixOS / Hyprland（Wayland）調査結果（実機未検証）

`gpui 0.2.2`、`blade-graphics 0.7.1`、`ghostty-vt-sys 0.9.0`（`GHOSTTY.lock` = `libghostty-vt 1.0.1`）、および本リポジトリの [`flake.nix`](../../flake.nix) を静的調査した結果は以下のとおり（**Linux 実機未ビルド・未動作確認**）。

1. **ビルド依存の不足（`ghostty-vt-sys` 側）**:
   - `ghostty-vt-sys 0.9.0` の `GHOSTTY.lock` は Apple (`xcframework`) と Windows GNU のみ事前ビルドバイナリを配布しており、Linux 向けプリビルドを含まない。
   - したがって Linux (`x86_64-linux` / `aarch64-linux`) でビルドするには、Ghostty `1.0.1` ソース・`libghostty-vt-1.0.1-deps.tar.gz` と対応する **Zig コンパイラ** を Nix で用意するか、`pkg-config` (`libghostty-vt-static`) / `GHOSTTY_VT_LIB_DIR` で事前ビルド済み静的ライブラリを供給する必要がある。[`flake.nix`](../../flake.nix) の `gpui` シェルは固定 nixpkgs の Zig 0.17 を供給する。Cargo のビルドスクリプトは固定リリースのソースと依存アーカイブを取得し、同梱ハッシュで検証する。
2. **GPUI Linux 描画バックエンドと Flake 依存**:
   - `gpui 0.2.2` は Linux でデフォルトで `wayland`・`x11`・`font-kit`・`cosmic-text`・`xkbcommon`・`blade-graphics` を有効化する。
   - GPU レンダリングは **Vulkan**（`blade-graphics` → `ash`）を使うため、Linux 実行・ビルド環境には既存の Wayland / xkbcommon に加えて **`vulkan-loader` と GPU ドライバ ICD / Mesa Vulkan、ならびに `fontconfig` / `freetype` 等の動的ライブラリパス** が必要になる（`gpui` シェルは Vulkan loader とフォントライブラリを供給し、GPU ドライバは NixOS 側の `hardware.graphics` を使用する。Linux 実機での構成検証は未了）。
3. **Hyprland / Wayland IME（Fcitx5 / IBus）の挙動**:
   - `gpui 0.2.2` の Wayland クライアント（`src/platform/linux/wayland/client.rs`）は `zwp_text_input_manager_v3` / `zwp_text_input_v3`（`PreeditString`・`CommitString`・`Done`・`set_cursor_rectangle`）を実装しており、Hyprland 標準の text-input-v3 とプロトコル上は接続できる。ただし、1 バイト長の `CommitString` を `KeyDown` 経路へ振り分ける分岐や `last_serial` 検証が存在するため、Fcitx5 での日本語 1 文字確定・連続変換・未確定候補位置は Linux 実機での検証が必須。
4. **Nerd Font アイコン専用フォントのフィルタ**:
   - `gpui 0.2.2`（`src/platform/linux/text_system.rs` L239–244）の `CosmicTextSystem` は、ロード時に **`'m'` グリフを持たないフォント（`charmap().map('m') == 0`）をデータベースから除外する**（例外は Windows 用 `SegoeFluentIcons` のみ）。
   - そのため Linux 環境でアイコン専用フォント `"Symbols Nerd Font Mono"` をそのまま指定しても無視される可能性が高く、`UDEV Gothic 35NF` や Nerd Font パッチ済み等幅フォントのように Latin `'m'` を含むフォントを優先フォールバックへ並べる必要がある。

---

## 7. 残存不具合・制約

1. **GPUI アイドル時 CPU（6.26%）**: 負荷時は 6.62% と非常に低いものの、アイドル 5 秒間の CPU 消費率が Ghostty（1.38%）や Tauri（3.33%）より高い（Cursor Blink タイマー 530ms / 初期フレーム・アニメーション周りと推測されるが、アイドル時再描画抑制の余地がある）。
2. **実機手動操作の未検証**: 実機 GUI 上での macSKK / macOS 標準日本語入力の実打鍵、`Cmd+Tab` 後の Space→変換→Enter/Esc、トラックパッドスクロール（p50/p95 の実体感）・ドラッグ選択・右クリック/`Cmd+V`、対話 Neovim / Codex の目視確認が未実施。
3. **PoC スコープ外の機能**: タブ管理、画面分割、設定ファイル/テーマ UI、コマンドパレット、マルチウィンドウ、macOS `.app` バンドル化・署名（現状スタンドアロン実行時は `nix develop` 環境または同階層の `herdr-core` / `HERDR_CORE_BIN` が必要）、NixOS/Hyprland 実機ビルドは未実装。
4. **上流クレート依存**: `gpui 0.2.2` および `ghostty-vt 0.9.0` は比較的新しい公開版であり、将来的な破壊的変更や macOS での Metal シェーダコンパイル（PoC では Xcode 不要な `runtime_shaders` 機能を使用）を製品版ではリリース事前コンパイルへ切り替えるか検討が必要。

---

## 8. 実機検証手順と起動コマンド

ユーザー環境の実キーボード・macSKK・トラックパッド・画面描画を目視確認するため、以下のコマンドをリポジトリルートから実行してください。

### 起動コマンド

```sh
# 1. 通常ライブターミナル起動（既定シェルの隔離起動、Menlo 14px / 106x32 / キャッシュ有効）
nix develop .#gpui --no-write-lock-file --command \
  ./poc/gpui-terminal/target/release/uiterm-gpui-poc --font-preset menlo --font-size 14

# 2. 普段 Ghostty 等で使っている UDEV Gothic 35NF での起動例（上部ツールバーなし: --minimal）
nix develop .#gpui --no-write-lock-file --command \
  ./poc/gpui-terminal/target/release/uiterm-gpui-poc --minimal --font-preset udev --font-size 13

# 3. IME・フォントプローブ UI の起動（診断用）
nix develop .#gpui --no-write-lock-file --command \
  ./poc/gpui-terminal/target/release/uiterm-gpui-poc --probe-ui
```

Linux では `nix develop .#gpui --command cargo build --locked --release --manifest-path poc/gpui-terminal/Cargo.toml` でビルドする。Hyprland で起動経路、Fcitx5 の日本語と ASCII の確定、Nerd Font を確認する。GPUI 0.2.2 は ASCII 一文字の IME 確定を物理キーと同じイベント型で渡すため、未確定中の確定漏れについては上流の区別が必要。

Kitty キーボード拡張で反復・解放イベントや全キーの報告を要求するアプリについては、現在のライブ端末は押下とテキスト確定のみを送るため、一部のキー報告が欠ける。

fish、Neovim、Codex を PATH に用意し、[検証手順](../development.md) に従ってテスト用 HOME・XDG を隔離してから、CLI 互換テストを実行する。

```sh
nix develop .#gpui --command cargo test --locked --manifest-path poc/gpui-terminal/Cargo.toml \
  --test compatibility installed_clis_fish_nvim_codex_and_resize_altscreen_work_in_isolated_pty -- --ignored
```

### 手動検証チェックリスト

1. **日本語 IME（macSKK と macOS 標準日本語入力）**
   - [ ] `stty -a` で `iutf8` が有効であり、`stty -echo; cat; stty echo` 上で日本語文字を Backspace 削除しても UTF-8 破損しないこと。
   - [ ] macSKK で未確定文字列（`▽...`）表示中に `Cmd+Tab` で他アプリへ移動し、戻った際に未確定表示が維持されること。続けて Space → 変換確定 Enter → 送信 Enter で 1 回だけ送信されること。
   - [ ] 変換中に `Esc` を押すと未確定文字が消え、他アプリ往復後も復活しないこと。変換中に `Cmd+C`/`Cmd+A`/`Option+b`/`Backspace`/矢印でショートカット誤爆や PTY 漏れが起きないこと。
   - [ ] 画面中央・右端・最下行で IME 変換候補ウィンドウの位置がカーソルに追従すること。
2. **フォント・文字幅・SGR 描画・選択**
   - [ ] Nerd Font アイコン（`fish` プロンプト / Neovim ステータスライン）、全角日本語、絵文字、結合文字（`か\u{3099}`）の右端が揃い崩れないこと。
   - [ ] 以下を実行し、通常文字・淡色（DIM）・隠し文字（hidden）・行末右端まで青背景 `#406090` が均一に保たれること:
     ```fish
     printf '\e[48;2;64;96;144m\e[2KNormal \e[2mDIM \e[22mnormal \e[8mhidden\e[28m\e[0m\n'
     ```
   - [ ] マウスドラッグ・ダブルクリック・トリプルクリックで日本語や絵文字を含む範囲を選択し、描画崩れがないこと、`Cmd+C` でコピー・`Cmd+V` でブラケットペースト（複数行が即時実行されない）できること、選択中でも `Ctrl+C` が前景コマンドへ届くこと。
3. **対話 CLI・Neovim・スクロール・後始末**
   - [ ] `nvim` で日本語編集、`Ctrl+D`/`Ctrl+U` 高速スクロール、ウィンドウドラッグリサイズ、フォーカス離脱・復帰時の `FocusLost`/`FocusGained`、`macism` 応答体感（Phase 2 の 4.5 秒待ちが起きないこと）を確認。
   - [ ] `seq 1 10000` 出力後にトラックパッドで 2,000 行スクロールバックを行き来し、カクつきや文字崩れがないこと。
   - [ ] `Cmd+Q` またはウィンドウ閉鎖後、`ps aux | rg 'uiterm-gpui-poc|herdr-core'` で起動した `uiterm-gpui-poc` と専用 `herdr-core` / `holder` / `/tmp/u-gp-*` が残存していないこと（既存ユーザーセッションに影響がないこと）。

---

## 9. ADR-0010 正式採用に向けた判断材料

本評価結果から得られる利点・懸念・前提条件を中立的に整理する。最終判断は上記実機検証チェックリストの結果を踏まえて行う。

| 判断観点 | GPUI + ネイティブ `libghostty-vt` 路線 | 現行 Tauri + WKWebView + WASM 路線 (`5ef4abb`) |
| --- | --- | --- |
| **メモリ効率（負荷時/ピーク）** | 30s 負荷ピーク **154.51 MiB**（終了後 **93.74 MiB** に即回復）。Ghostty 1.4.0 (`144.76 MiB`) と同等水準 | 30s 負荷ピーク **435.44 MiB**（`WebContent` `260.42 MiB`、終了後 `307.80 MiB` 滞留） |
| **CPU・VT 処理性能** | 負荷平均 CPU **6.62%**、VT 単体 **387〜404 MiB/s**。コア共有の Rust 単一バイナリ（6.02 MiB） | 負荷平均 CPU **27.30%**、VT WASM **102〜105 MiB/s** |
| **`macism` 遅延問題** | 前景端末内で **29.23 ms**（Ghostty 39.28 ms と同等、遅延非発生） | 本評価で **4,556.96 ms** の遅延を再検証（WebKit/TIS 周りの固有リスクあり） |
| **実装複雑度・開発体制** | IME・選択・クリップボード・マルチタブ・ショートカット・設定 UI 等の UI 基盤を自前で GPUI 実装する必要あり。Rust エコシステム・GPUI 上流追従コストが増す | HTML/CSS/React の豊富な UI 資産・ブラウザ標準の IME/選択/フォントフォールバックを活用可能。ただし WebKit プロセス肥大化とパッチ保守（`@wterm/dom`）が課題 |
| **内蔵ブラウザ要件（Phase 3+）** | GPUI 単体では汎用 Web ブラウザを内蔵できず、必要時は外部ブラウザ連携または別 WebView 統合設計が必要 | WKWebView / WebKitGTK 上の製品構造上、将来的な Web UI 拡張と親和性が高い |
| **NixOS / Hyprland 対応** | `ghostty-vt-sys` の Zig ビルド統合 + Vulkan 依存解決 + Linux 実機検証が先行ゲート | 既存の Nix Flake で静的評価済み（ただし実機での Wayland + WebKitGTK + Fcitx5 検証は同様に未実施） |

**正式採用前に確認すべきゲート条件**:
1. ユーザー実機での macSKK・標準日本語 IME・トラックパッド操作に重大な違和感がないこと。
2. GPUI アイドル時 CPU（現状 6.26%）の低減原因を特定できること。
3. NixOS / Hyprland 環境で `ghostty-vt-sys` の Zig ビルドおよび Vulkan + `zwp_text_input_v3`（Fcitx5）の動作確認の見通しが立つこと。
4. 内蔵ブラウザ非搭載方針を含め、Tauri 基盤のリスク（メモリ 435 MiB・`macism` 4.5s）と GPUI 自前 UI 構築コストのトレードオフがプロダクト要件と合致すること。
