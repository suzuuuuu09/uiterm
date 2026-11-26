---
description: "uiterm Phase 0 最終工程における実装整理・軽量化・最適化レポート"
---

# uiterm Phase 0 実装整理・軽量化レポート

本ドキュメントは、uiterm Phase 0 最終工程におけるフロントエンド依存関係の整理、メモリ・CPU 使用量の最適化、および Rust リリースビルド・配布容量の最適化内容、技術的根拠、テスト結果、統合後の測定項目、残存課題をまとめたものである。

> [!IMPORTANT]
> 本作業は別 worktree での品質保証・技術検証（Codex 担当）と並行して実施している。本ドキュメントに記載するアプリ起動時の RSS 等は**最適化の方向性を確認するための参考値**であり、正式な性能比較は両エージェントの変更を統合した後に同一条件で再測定する。

---

## 1. 依存関係・実装の整理

### 1.1 ターミナル描画ライブラリの限定と不要コード削除

[docs/poc/frontend.md](./frontend.md) および [docs/poc/integration.md](./integration.md) の選定判断に基づき、ターミナル描画スタックを **`@wterm/dom` + `@wterm/ghostty`（`libghostty` WASM）** に限定した。

- **削除した直接依存関係（[package.json](../../package.json)・[pnpm-lock.yaml](../../pnpm-lock.yaml) 更新済み）**:
  - `@gespenst/core`
  - `@wterm/core`（直接インポートなし。`@wterm/dom` の推移依存としてのみ存在）
  - `@xterm/xterm`
  - `@xterm/addon-fit`
  - `@xterm/addon-webgl`
  - `ghostty-web`
- **削除した比較用アダプター**:
  - `src/terminal/gespenstAdapter.ts`
  - `src/terminal/ghosttyWebAdapter.ts`
  - `src/terminal/xtermAdapter.ts`
- **整理したフロントエンド設定・コード**:
  - [src/transport/schemas.ts](../../src/transport/schemas.ts): `TerminalBackendIdSchema` を `"wterm-ghostty-dom" | "wterm-ghostty-webgl"` に限定
  - [src/terminal/createAdapter.ts](../../src/terminal/createAdapter.ts): `WTermGhosttyAdapter` のみ生成するよう整理
  - [src/index.css](../../src/index.css): `@gespenst/core/style.css`・`@xterm/xterm/css/xterm.css` の不要インポートを削除
  - [src/i18n/dict.ts](../../src/i18n/dict.ts)・[src/App.tsx](../../src/App.tsx): 削除済み描画エンジンの選択肢・辞書キーを削除
  - [vite.config.ts](../../vite.config.ts): `optimizeDeps.exclude` を `["@wterm/ghostty"]` に整理
  - [scripts/bench-terminals.mjs](../../scripts/bench-terminals.mjs): `@wterm/ghostty` 単体（`scrollbackLines` 5,000 vs 2,000 比較）向けに更新
- **保持したもの**:
  - 過去の比較検証記録（[docs/poc/frontend.md](./frontend.md)・[docs/poc/integration.md](./integration.md)）
  - 性能測定 UI・日本語/文字幅テスト・VT/ANSI テスト・自動応答プローブ・IME 診断ログ機能

### 1.2 Vite+ ビルド警告の解消とバンドルサイズ削減

| 指標 | 整理前 | 整理後 | 削減率 |
| --- | ---: | ---: | ---: |
| Vite+ ビルド警告数 | 3 件（`node:url`・`node:fs/promises` externalization + 500 kB 超過） | **0 件** | **-100%** |
| `dist/` 合計サイズ | 3.4 MB | **1.1 MB** | **-67.6%** |
| メイン JS チャンク（Raw / Gzip） | 1,577.66 kB / 452.01 kB | **474.50 kB / 146.33 kB** | **-69.9% / -67.6%** |
| CSS（Raw / Gzip） | 26.39 kB / 6.54 kB | **22.42 kB / 6.06 kB** | **-15.0% / -7.3%** |
| 同梱 WASM | 2 種計 1,789.53 kB | **1 種 599.62 kB**（gzip 199.83 kB） | **-66.5%** |

---

## 2. メモリ・CPU 使用量の最適化

### 2.1 WKWebView の固定コストと削減可能なコストの切り分け

Tauri 2（macOS）の RSS（整理前 237.75 MiB）は、大きく以下の 2 つに分かれる。

1. **WKWebView / OS フレームワークの固定ベースライン（削減困難）**:
   - `uiterm`（Tauri Rust ホスト + AppKit/WKWebView フレームワーク共有ページ）: 約 75〜82 MiB
   - `com.apple.WebKit.GPU`（Metal コンポジタ。`transparent: false` 維持により常時再合成ループは回避済み）: 約 38〜42 MiB
   - `com.apple.WebKit.Networking`（WebKit XPC ネットワークプロセス）: 約 13〜15 MiB
   - `com.apple.WebKit.WebContent` の空ページ初期化コスト（JavaScriptCore + DOM/Layout 基盤）: 約 40〜50 MiB
2. **アプリケーション側の削減可能コスト（今回の最適化対象）**:
   - JS バンドルのパース・JIT メモリ（1.58 MB → 474.5 KB へ削減）
   - 端末ごとの WASM 線形メモリ（スクロールバック行数上限 + 破棄時の明示解放）
   - React 再レンダリング・Zustand 状態更新頻度
   - 非表示端末の初回描画・冗長な IPC 送信
   - 診断ログの文字列保持サイズ・イベント購読/タイマー残留

### 2.2 実施した最適化と技術的根拠

| 対象領域 | 発見した問題・根拠 | 実施した対策 | 対象ファイル |
| --- | --- | --- | --- |
| **ターミナル破棄時のリソース解放（重要）** | `@wterm/dom@0.6.0` の `WTerm.destroy()` をソース解析したところ、外部注入された `options.core`（`GhosttyCore`）の `dispose()` を**呼び出していない**ことを確認。結果として Zig 側の `deinit(termPtr)` と 5 種の内部転送バッファ解放が行われていなかった。 | `WTermGhosttyAdapter` で `core` 参照を保持し、`dispose()` および `mount()` のエラー・レース経路で `core.dispose()` を必ず呼び出すよう修正。`disposeSession()` で `pendingWrites` / `pendingResize` もクリア。 | [src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts), [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts) |
| **WASM ロード方式** | `@wterm/ghostty/dist/wasm-bindings.js` を解析し、`WebAssembly.compileStreaming()` と `WebAssembly.Module` キャッシュ（最大 4 エントリ）によるネイティブコード共有、および端末別独立 `WebAssembly.Instance`（破棄時 GC で線形メモリ返却）が既に最適設計であることを確認。 | 既存方式を維持。なお `@wterm/dom` が推移依存する `@wterm/core` の `wasm-inline-*.js`（73.5 kB）は動的インポートであり、`core: GhosttyCore` 指定時は実行時未ロード（実行メモリ 0 B）であることを確認。 | [src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts) |
| **スクロールバック上限** | `herdr-core` がバックエンド側で正の履歴を保持する二重 VT 構成において、フロントエンドのデフォルト `scrollbackLines = 5000` は 10 端末負荷時に +89.61 MB を消費していた。 | デフォルト `DEFAULT_SCROLLBACK_LINES` を **2,000 行**へ調整（`TerminalAdapterOptions` で上書き可能）。ヘッドレス実測で 10 端末負荷時 RSS 増分が **+89.61 MB → +49.81 MB（-44.4%）**、破棄+GC 後 **-0.17 MB**（完全回収）を確認。 | [src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts) |
| **React 不要再レンダリング** | `App` ルートが `totalBytesReceived`・`logs`・`isComposing` を含む全 12 スライスを購読しており、PTY 出力フレームやキー入力ごとにヘッダー・タブバー・ビューポート全体が再レンダリングされていた。また `updateTabSize` が同値でも新配列を生成していた。 | `StatusSummary`・`BenchmarkPanel`・`DiagnosticLogPanel`・`TabBar` を独立コンポーネントへ分離し購読を局所化。`updateTabSize`・`setIsComposing` に同値スキップを追加。統計フラッシュを `queueMicrotask` から 100 ms スロットルへ変更。 | [src/App.tsx](../../src/App.tsx), [src/store/pocStore.ts](../../src/store/pocStore.ts), [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts) |
| **非表示ターミナルの描画・IPC 負荷** | 非表示タブ生成時に `WTerm` 初期化完了まで `renderingPaused: false` で初回描画と `fit()` が走っていた。またタブ切替のたびに全非表示タブへ重複して `terminalFocus(..., false)` IPC を送信していた。 | `TerminalAdapterOptions.renderingPaused` を追加し、非表示タブは `new WTerm(..., { renderingPaused: true })` で初期化・初回 `fit()` をスキップ。`ManagedSession.focused` と `activeId` を追跡し、状態変化した端末のみ `terminalFocus` IPC を送信。 | [src/terminal/types.ts](../../src/terminal/types.ts), [src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts), [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts) |
| **不要タイマー・イベント購読** | `eventsSubscribe()` に対応する `stopEventStream()` がなく、アンマウント時に `eventsUnsubscribe()` が呼ばれなかった。 | `stopEventStream()` を追加して `App` アンマウント時に購読解除。`disposeAll()` で統計フラッシュタイマーを確実にキャンセル。 | [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts), [src/App.tsx](../../src/App.tsx) |
| **診断ログのメモリ消費** | 大きなペースト時に `formatControlBytes()` が入力全体を文字列化・置換し `pocStore.logs` に保持する構造だった。 | 入力プレビューを先頭 128 バイト（`MAX_LOG_BYTES_PREVIEW`）、ログ詳細を最大 160 文字（`MAX_LOG_DETAIL_CHARS`）に制限。さらに IME/入力/自動応答ログの記録を切り替える `diagnosticsEnabled` トグルを追加（デフォルト ON で Phase 0 診断機能を維持）。 | [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts), [src/store/pocStore.ts](../../src/store/pocStore.ts), [src/components/TerminalPane.tsx](../../src/components/TerminalPane.tsx) |

### 2.3 スクロールバック上限変更のヘッドレス実測結果（`node --expose-gc scripts/bench-terminals.mjs`）

10,000 行（996.1 KB、ANSI カラー＋日本語 UTF-8、`120x40`）を投入した場合の比較：

| 設定 | 端末数 | 10k 行解析 | スループット | 負荷後 RSS 増分 | 破棄+GC 後 RSS 残 | 自動応答数 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `scrollbackLines = 5000`（変更前） | 5 | 9.9 ms | 98.2 MB/s | +44.84 MB | +0.38 MB | 2 |
| `scrollbackLines = 5000`（変更前） | 10 | 9.8 ms | 99.3 MB/s | +89.61 MB | +0.55 MB | 2 |
| **`scrollbackLines = 2000`（変更後）** | 5 | **9.4 ms** | **103.2 MB/s** | **+24.86 MB** (-44.6%) | **+0.02 MB** | 2 |
| **`scrollbackLines = 2000`（変更後）** | 10 | **9.4 ms** | **103.2 MB/s** | **+49.81 MB** (-44.4%) | **-0.17 MB** | 2 |

---

## 3. ビルド・ディスク容量の最適化

### 3.1 Rust `[profile.release]` 比較実測（macOS arm64・整理済み `dist/` 同梱）

| プロファイル構成 | ビルド時間 | `uiterm` バイナリサイズ | 削減率（対デフォルト） | 評価 |
| --- | ---: | ---: | ---: | --- |
| 0. デフォルト (`opt-level=3`, `lto=false`, `cgu=16`, `strip=false`) | 0.47 s *(キャッシュ済)* | 10.80 MiB (11,325,224 B) | — | 基準（整理前は ~12 MiB） |
| A. Thin LTO + Strip (`opt-level=3`, `lto="thin"`, `cgu=1`, `strip=true`) | 96.30 s | 5.97 MiB (6,259,664 B) | -44.7% | 速度優先だがサイズ削減幅は中程度 |
| **B. Tauri 推奨 (`opt-level="s"`, `lto=true`, `cgu=1`, `strip=true`, `panic="abort"`)** | **78.40 s** | **3.19 MiB (3,349,344 B)** | **-70.4%** | **採用**。ループベクタライズを維持しつつ大幅削減 |
| C. 最小サイズ (`opt-level="z"`, `lto=true`, `cgu=1`, `strip=true`, `panic="abort"`) | 54.03 s | 2.41 MiB (2,531,472 B) | -77.7% | 追加削減は 0.78 MiB に留まり、インライン抑止による速度低下リスクあり |

**採用設定（[src-tauri/Cargo.toml](../../src-tauri/Cargo.toml)）**:
- `[profile.release]` に **構成 B**（`opt-level = "s"`, `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`）を適用。
- `[lib]` の `crate-type` をデスクトップ専用の `["rlib"]` へ整理（モバイル用の未使用 `staticlib` / `cdylib` を削減）。
- Rust バックエンドは Unix Socket / IPC 中継が主体の I/O バウンド処理と想定しており、`opt-level = "s"` による体感性能への影響は未計測のため、統合後の性能測定で確認する。

### 3.2 配布容量（`herdr-core` を含む実質的な配布容量）

| 対象 | 最適化前 | 最適化後 | 備考 |
| --- | ---: | ---: | --- |
| `uiterm.app`（macOS バンドル） | 13.73 MiB | **4.20 MiB** (-69.4%) | 内訳: `uiterm` 約 3.2 MiB（§3.1 のバイナリ 3.19 MiB）+ `poc_bench` 672 KiB + `Info.plist`/アイコン |
| `herdr-core` v0.1.22（macOS arm64） | 6.30 MiB | 6.30 MiB | 必須バックエンドバイナリ（Linux x86_64 tarball は 3.30 MB） |
| **実質合計配布容量（`uiterm.app` + `herdr-core`）** | **20.03 MiB** | **10.50 MiB** (-47.6%) | Ghostty 67 MiB の約 1/6、Herdr 82 MiB（59+23）の約 1/8 |

> [!TIP]
> **追加削減余地（672 KiB）**: `uiterm.app/Contents/MacOS/` にベンチマーク用バイナリ `poc_bench`（672 KiB）が同梱されている。これは `src-tauri/src/bin/poc_bench.rs` が `[[bin]]` ターゲットとして認識されるためである。Codex 側と合意の上で `examples/poc_bench.rs`（`cargo run --example poc_bench`）へ移設すれば、`uiterm.app` は **約 3.55 MiB**（`herdr-core` 込み **約 9.85 MiB**）までさらに軽量化できる。

### 3.3 開発キャッシュ容量（Rust・Nix・pnpm）の現状と改善策

※ 制約に従い、グローバルキャッシュ削除や Nix GC は実行していない。

- **現状（本ワークスペース）**:
  - `src-tauri/target`: **8.0 GiB**（debug + release 複数プロファイル比較ビルドの増分キャッシュを含む）
  - `node_modules`: **352 MiB**（pnpm ハードリンク参照。`vite-plus`・`@tauri-apps/cli` 等の開発ツールが大部分を占める）
- **改善策**:
  1. **Rust (`target/`)**:
     - `crate-type = ["rlib"]` 化（実施済み）により不要な `libuiterm_lib.a` / `.dylib` 生成を抑止。
     - 複数 worktree 運用時は環境変数 `CARGO_TARGET_DIR` を共有ディレクトリへ集約、または `sccache` 導入により worktree ごとの 3〜8 GiB 重複を防止。
     - リリースビルド時のみ LTO を有効化し、`dev` プロファイルはデフォルトの高速増分ビルドを維持。
  2. **pnpm**:
     - グローバルコンテンツアドレス可能ストアにより worktree 間で実ディスク消費はほぼ重複しない。定期的に不要パッケージを `pnpm store prune`（ユーザー判断で実行）することで回収可能。
  3. **Nix**:
     - `flake.nix` の `devShell` は既に最小構成（`rustc`/`cargo`/`clippy`/`rustfmt`/`pkg-config`/`nodejs_24`/`pnpm`/`herdr-core` + Linux 時のみ WebKitGTK 群）。
     - 開発者の任意タイミングで `nix-collect-garbage -d` / `nix store optimise` を実行する運用を推奨。

---

## 4. 動作確認・テスト結果

| 確認項目 | コマンド | 結果 |
| --- | --- | --- |
| 型検査 | `pnpm typecheck` (`tsc --noEmit`) | **合格**（0 errors） |
| Lint | `pnpm lint` (`vp lint`, 22 files / 96 rules) | **合格**（0 warnings, 0 errors） |
| フロントエンドテスト | `pnpm test -- --run` (Vitest 5.1.1) | **合格**（8 / 8 tests passed, 161 ms） |
| Rust 静的解析 | `cargo clippy --all-targets -- -D warnings` | **合格**（0 warnings） |
| Rust 単体・統合テスト | `cargo test -- --nocapture` | **合格**（unit 2/2 + integration 2/2 = 4/4 passed） |
| リリースビルド | `pnpm tauri build --bundles app` | **合格**（`uiterm.app` 4.20 MiB, Vite+ 警告 0 件） |
| Tauri 実ウィンドウ起動 | `nix develop --command .../uiterm.app/Contents/MacOS/uiterm` | **合格**（起動・WKWebView 描画・隔離 `herdr-core` v0.1.22 起動およびアタッチ動作を確認） |
| 基本的なターミナル操作 | `herdr_integration`（シェル起動・I/O・リサイズ・自動応答分離・OSC 7501・再アタッチ） | **合格** |

※ 参考値（並行作業中の 1 端末起動直後スナップショット）: `com.apple.WebKit.WebContent` RSS は 88.83 MiB → **77.67 MiB**（79,536 KiB）と約 11 MiB の減少傾向を確認。確定値は統合後に再測定する。

---

## 5. 統合後に測定すべき項目

Codex 担当 worktree の変更と統合した後に、以下を同一条件で正式測定・記録すること：

1. **アプリ全体 RSS・CPU（1 端末アイドル・安定後）**: `uiterm` + `WebKit.WebContent` + `WebKit.GPU` + `WebKit.Networking` + `herdr-core` + `holder` + シェルの全プロセス合計と内訳
2. **複数端末スケーリング（1 / 5 / 10 端末）**: マウント時間・合計 RSS・10,000 行バースト（アクティブ/非表示）・タブ切替レイテンシ・全破棄＋GC 後の RSS 回収量（`core.dispose()` 修正の効果検証）
3. **起動時間**: コールド起動 / ウォーム起動の初回ライブ端末到達時間
4. **長時間連続出力・タブ開閉繰り返し**: 20 秒連続出力および 10 端末作成→負荷→破棄の反復におけるメモリリーク有無
5. **配布容量**: `poc_bench` 移設後の最終 `uiterm.app` サイズ + `herdr-core` 込み合計容量
6. **実機互換性**:
   - macOS WKWebView での日本語 IME 手動対話テスト（変換候補位置・確定 Enter 非誤送信・全角 Backspace・`diagnosticsEnabled` ON/OFF 双方）
   - NixOS / Hyprland（ネイティブ Wayland + WebKitGTK + Fcitx5 + Mozc）実機検証

---

## 6. 残存する問題・Codex 担当領域への変更提案

1. **`poc_bench` の `.app` 同梱（672 KiB）**【変更提案】:
   - `src-tauri/src/bin/poc_bench.rs` を `src-tauri/examples/poc_bench.rs` へ移設（または別ワークスペースクレートへ分離）することで、リリース `.app` へのベンチマークバイナリ同梱を防ぎ、約 672 KiB 削減できる。
2. **`SIGTERM` / `SIGINT` 受信時の隔離 `herdr-core` 残留**【変更提案】:
   - Tauri 実ウィンドウ起動テスト中、プロセスを `SIGTERM` で終了した際に `RunEvent::Exit` が発火せず、隔離起動した `herdr-core` デーモンおよび `holder` が残留する現象を確認した（通常 GUI 終了時は `RunEvent::Exit` 経由で停止）。Phase 1 のプロセスライフサイクル管理でシグナルハンドリングまたは親プロセス監視（死活監視）を検討すること。
3. **`@wterm/core` の未ロード遅延チャンク（73.5 kB）**:
   - `@wterm/dom` がフォールバック用に `@wterm/core` をインポートしているため、`dist/assets/wasm-inline-*.js`（73.5 kB / gzip 32.0 kB）が生成される。`GhosttyCore` 使用時は実行時にロードされないためメモリ影響は 0 だが、上流 `@wterm/dom` がコア分離サブパスエクスポートを提供した際に再検討する価値がある。
4. **未実施の実機検証**:
   - macOS WKWebView での日本語 IME 手動対話テストおよび NixOS / Hyprland 実機検証は引き続き統合後の必須確認項目として残る。
