---
description: "uiterm Phase 0 統合検証・性能測定・最終技術選定レポート"
---

# uiterm Phase 0 統合検証レポート

> 検証時点の記録。統合後の判定、測定条件の差、および修正後の IPC 仕様は [独立検証レポート](./validation.md) を参照。

本ドキュメントは、[docs/poc/backend.md](./backend.md)・[docs/poc/frontend.md](./frontend.md)・[docs/poc/interface.md](./interface.md) を踏まえ、Tauri 2 実ウィンドウ上で `herdr-core` と `wterm` を結合した統合検証・バグ修正・アプリ全体性能測定・比較評価および最終技術選定判断をまとめたものである。

---

## 1. 統合検証で実施した修正と検証結果

### 1.1 `attachId = 0` 先行送信レースの修正（フロントエンド＋バックエンド二重防御）

- **問題**: [src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts) において、`terminalAttach()` の Promise が解決して `session.attachId` が設定される前に、初回リペイントの Tauri Channel コールバック（16 KiB 以上のデータによる `terminalAck` や、端末クエリによる `onAutoResponse`）や `ResizeObserver` / ユーザー入力が発火した場合、`attachId = 0`（JSON-RPC 制御チャンネル）宛てにフレームが送信される可能性があった。
- **修正**:
  - **フロントエンド（[src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts)）**: `session.attachId > 0` を全送信 IPC の前提条件とし、アタッチ完了前に発生した `unackedBytes`・`pendingWrites`（`user` / `automatic`）・`pendingResize` をキューに保持し、`terminalAttach()` 解決直後に実 `attachId` で一括フラッシュする `flushPostAttach()` を実装。
  - **バックエンド（[src-tauri/src/herdr/client.rs](../../src-tauri/src/herdr/client.rs)）**: `terminal_detach` / `terminal_write` / `terminal_resize` / `terminal_focus` / `terminal_facts` / `terminal_ack` で `attach_id == 0` を明示的に拒否する `ensure_valid_attach_id()` を追加（`code: "invalid_args"`）。
- **検証**:
  - Vitest（[src/terminal/__tests__/poc.test.ts](../../src/terminal/__tests__/poc.test.ts)）：`terminalAttach()` 解決前に 20 KiB のデータと自動応答を発生させ、`attachId = 0` での送信が 0 件であること、解決後に実 `attachId = 7` で ACK・自動応答がフラッシュされることを検証済み。
  - Rust 統合テスト（[src-tauri/tests/herdr_integration.rs](../../src-tauri/tests/herdr_integration.rs)）：`terminal_ack(0, ...)` / `terminal_write(0, ...)` が `invalid_args` で安全に拒否されることを検証済み。

### 1.2 バックエンド起点リサイズ通知のループ防止

- **問題**: `herdr-core` から `OUTPUT_FRAME_RESIZE` を受信して `adapter.resize(cols, rows)` を呼ぶと、描画器側の `onResize` コールバックが同期発火し、`terminalResize()` をバックエンドへ送り返して無限リサイズループになる構造だった。
- **修正**: `ManagedSession` に `applyingBackendResize` ガードフラグと `lastSentCols` / `lastSentRows` を導入。バックエンド起点のリサイズ適用中は `onResize` からの送り返しを抑止し、さらに同一サイズへの重複リサイズ送信も重複排除した。
- **検証**: Vitest にて `OUTPUT_FRAME_RESIZE (120x40)` 受信時に `terminalResize` が 0 回、ユーザー起点のリサイズ `(132x43)` で 1 回、同一 `(132x43)` の再通知で重複送信されないことを検証済み。

### 1.3 IPC エラー処理とセッションライフサイクル管理

- **修正**:
  - すべての非同期 IPC 呼び出しに `.catch()` と構造化エラーフォーマッタ（`[code] message (hint)`）を適用し、未処理 Promise rejection を排除。
  - `createSession()` の途中で失敗した場合に DOM コンテナ・WASM アダプタ・作成済み `herdr-core` セッションをすべてクリーンアップするロールバック処理を追加。
  - `session.disposed` フラグにより破棄後の遅延フレーム書き込み・ACK 送信を防止。
  - `startEventStream()` により `eventsSubscribe()` を接続し、OSC 7501 状態遷移（`status` / `turn`）・セッション終了（`finished`）・切断（`disconnected`）を診断ログへ出力。
  - ツールバーに実行モード表示（`tauri+herdr-core` / `mock`）を追加。
  - [src-tauri/Cargo.toml](../../src-tauri/Cargo.toml) に `default-run = "uiterm"`、[src-tauri/tauri.conf.json](../../src-tauri/tauri.conf.json) に macOS `.app` バンドル設定を追加。

### 1.4 Tauri 実ウィンドウ起動および CLI / TUI 互換性

- **Tauri 実ウィンドウ（macOS WKWebView）**: ビルド済み `uiterm.app` を `nix develop` 環境から実起動し、WKWebView + React + `wterm` + `libghostty` WASM が Tauri IPC 経由で隔離 `herdr-core` v0.1.22 を起動し、初回シェル（`term-1`）を作成・アタッチしてライブ稼働することを確認。
- **`fish`（対話 UTF-8）**: `fish -i` 内で日本語文字列 `日本語FISH_OK` の入力・出力・終了を確認（`ok=true`）。
- **`nvim --clean`**: Framed Mode アタッチストリーム上で起動〜alt-screen 描画（12,506 bytes）〜`:q!` 正常終了（`exit_code=0`）を確認。
  - ※ 補足：`herdr-core` の CLI `hc send` はデフォルトでブラケットペーストを用いるため、Neovim のノーマルモード遷移用途には生キー送信（`DATA` フレームまたは `hc keys`）を使う必要があることを確認した。
- **`codex`**: `codex --version` および `codex --help`（126 行出力）の PTY 実行を確認。対話セッションの長時間稼働は API 課金・ユーザー設定保護の制約から未実施。

### 1.5 macOS WKWebView 日本語 IME 実機テストで発見した問題と修正

実機テスト（`これはテストです。` 入力→Backspace 削除）により以下を確認した：

- **IME 入力・確定（合格）**: `compositionstart` / `compositionupdate` / `compositionend` が正常に動作し、確定直後の `keydown` は `keyCode=229` でガードされ重複入力や改行誤送信は発生しなかった。
- **発見した問題（日本語削除時に `bash#` プロンプトまで消える現象）**:
  - **根本原因**:
    1. `nix develop` が `$SHELL` を Nix stdenv の最小 `bash`（`bash-5.3p20`・readline 非搭載）に上書きしていたため、シェルがカーネル canonical（cooked）モードで動作していた。
    2. `herdr-core` が開く macOS PTY の termios が `-iutf8`（UTF-8 入力モード OFF）かつ `LANG=""` だったため、cooked モードでの Backspace が 1 文字（2 カラム）ではなく 1 バイト（3 バイト = 3 カラム）単位で `\b \b` を出力し、全角 9 文字（18 カラム / 27 バイト）の削除で 9 カラム余計に消えてプロンプトが削られた。
  - **修正**: [src-tauri/src/commands.rs](../../src-tauri/src/commands.rs) の `terminal_create` にて、`argv` / `line` 未指定時は OS のユーザーアカウントに設定されたデフォルトログインシェル（Unix の `getpwuid(geteuid())->pw_shell`、取得不可時のみ `$SHELL` へフォールバック）を使用するよう変更し、`LANG` が空の場合は UTF-8 ロケール（`LC_CTYPE` または `en_US.UTF-8`）を補完するようにした。
  - **補足（Phase 1 以降の課題）**: cooked モードで動く単純な CLI（`cat`・`read` 等）でも全角削除が崩れないよう、`herdr-core` 側 PTY 初期化で `IUTF8` フラグを有効化する提案・検証を行うことが望ましい。

---

## 2. アプリ全体の性能測定と Ghostty / Herdr 比較

### 2.1 測定環境

- **マシン**: macOS 15.5 (Darwin 24.5.0, Apple Silicon `arm64`)
- **ビルド**: `pnpm tauri build --bundles app`（`tauri 2.12.7`, `rustc 1.99.1`, `vite-plus 1.8.0`, `herdr-core 0.1.22`）
- **比較対象（同一マシン上の既存インストール・実行中プロセスを読み取り専用で計測）**:
  - `/Applications/Ghostty.app`（Ghostty 1.4.0、ネイティブ Zig + Metal）
  - `/Applications/Herdr.app`（Herdr 0.9.5、ネイティブ Rust + GPUI）

### 2.2 配布用アプリ容量・起動時間・アプリ全体 RSS / CPU 比較

| 指標 | uiterm (Phase 0 PoC) | Ghostty 1.4.0 | Herdr 0.9.5 |
| --- | --- | --- | --- |
| **配布アプリ容量（macOS `.app`）** | **13.73 MiB**（+ `herdr-core` 6.3 MiB = **20.0 MiB**） | 67 MiB | 59 MiB（+ CLI 23 MiB） |
| **起動時間（初回ライブ端末到達）** | コールド **1,274.2 ms** / ウォーム **534.1 ms** | 未実測（既存起動中） | 未実測（既存起動中） |
| **アプリ全体 RSS（1 端末アイドル時）** | **237.75 MiB**（WebKit 全ヘルパー込み） | 48.47 MiB | 93.97 MiB（App 56.09 + Server 37.88） |
| **アイドル時 CPU 使用率** | **0.1%** | — | — |

#### uiterm 実起動プロセスツリー内訳（1 端末アイドル・3 秒安定後）

| プロセス | RSS | CPU | 役割 |
| --- | --- | --- | --- |
| `com.apple.WebKit.WebContent` | 88.83 MiB | 0.0% | React + `wterm` + `libghostty` WASM（※PoC では比較用 4 エンジンも同梱中） |
| `uiterm` | 76.44 MiB | 0.0% | Tauri 2 Rust ホスト + WKWebView フレームワーク |
| `com.apple.WebKit.GPU` | 37.94 MiB | 0.0% | WKWebView GPU コンポジタ |
| `com.apple.WebKit.Networking` | 13.16 MiB | 0.0% | WKWebView ネットワークプロセス |
| `herdr-core`（daemon） | 10.03 MiB | 0.1% | セッション管理・VT エンジン |
| `herdr-core`（holder） | 5.42 MiB | 0.0% | PTY 保持プロセス |
| `bash`（シェル） | 5.94 MiB | 0.0% | ライブシェル |
| **合計** | **237.75 MiB** | **0.1%** | — |

> [!NOTE]
> **評価**: 配布容量は Ghostty / Herdr の約 1/3（さらに Phase 1 で比較用 4 エンジン削除＋`lto`/`strip` 適用で削減可能）。一方、メモリ使用量は WKWebView の固定ベースライン（約 216 MiB）があるため、ネイティブ描画の Ghostty（48 MiB）や Herdr（94 MiB）よりは明確に大きい。Chromium 同梱（Electron 系の 400〜600+ MiB）と比べれば軽量であり、「TypeScript / Web UI による高いカスタマイズ性・開発速度」とのトレードオフとして許容範囲内と判断する。

### 2.3 1 / 5 / 10 端末スケーリング（バックエンド `herdr-core` + `holder`）

| 端末数 | 生成時間 | `herdr-core` RSS | `holder` RSS | 合計（バックエンド） |
| --- | --- | --- | --- | --- |
| 1 端末 | 26.8 ms | 8.34 MiB | 5.09 MiB | **13.44 MiB** |
| 5 端末 | 78.0 ms | 9.45 MiB | 6.17 MiB | **15.62 MiB** |
| 10 端末 | 131.8 ms | 9.95 MiB | 7.36 MiB | **17.31 MiB** |

※ フロントエンド側の 1 / 5 / 10 端末スケーリング（`@wterm/ghostty` WASM：`+5.2` / `+23.4` / `+46.1 MiB`、`dispose()` 後 99.5% 解放）は [docs/poc/frontend.md](./frontend.md) に実測記録済み。

### 2.4 長時間連続出力時のメモリ安定性（20 秒間・計 20 MiB 連続出力）

| 経過時間 | `herdr-core` RSS | `holder` RSS |
| --- | --- | --- |
| `t = 1s` | 15.44 MiB | 7.48 MiB |
| `t = 5s` | 15.47 MiB | 8.44 MiB |
| `t = 10s` | 14.38 MiB | 7.45 MiB |
| `t = 15s` | 13.31 MiB | 6.58 MiB |
| `t = 20s` | 13.42 MiB | 6.53 MiB |

スクロールバック上限によりメモリは完全に有界（増加傾向なし・リークなし）。

---

## 3. 日本語 IME（macOS WKWebView）手動検証手順

macOS 実ウィンドウでの日本語 IME 対話入力は自動化できないため、以下の手順で手動確認を実施すること。

### 起動コマンド

```sh
# 開発モード（HMR 付き）
nix develop --command pnpm tauri dev

# またはビルド済み .app を直接起動（HERDR_CORE_BIN を引き継ぐ）
nix develop --command open -W src-tauri/target/release/bundle/macos/uiterm.app
```

### 手動確認チェックリスト

1. **接続確認**: ツールバーに緑の `tauri+herdr-core` バッジが表示され、端末にシェルプロンプトが出ること。描画エンジンが `wterm + libghostty (DOM)` であることを確認。
2. **未確定文字列・変換候補位置**: 日本語 IME を ON にして `にほんごにゅうりょく` と入力し、未確定文字列がカーソル位置にインライン表示され、変換候補ウィンドウがカーソル直下に追従すること。
3. **変換確定 Enter の非誤送信**: 変換候補を Enter で確定した際、シェルへ改行（コマンド実行）が誤送信されないこと。右ペインの IME ログに `compositionend` と `keyCode=229` が記録されること。
4. **変換中ショートカットの誤作動防止**: 変換中に `Ctrl+C` / `Backspace` / 矢印キーを操作しても変換操作として処理され、アプリ側ショートカットが誤作動しないこと。
5. **全角・半角・フォントフォールバック**: ツールバーの「日本語テスト」ボタンを押し、ASCII 40 桁と全角 20 文字の右端が揃うこと、日本語フォントが正常に表示されること。
6. **`fish` / `nvim` 実対話**: 端末内で `fish` や `nvim --clean` を起動し、日本語入力・編集・リサイズ・終了が崩れないこと。
7. **自動応答分離**: 「クエリ送信」ボタンを押し、右ペインのログで `AUTO(kind=6)` または `BLOCKED` としてユーザー入力（`input`）と分離記録されること。

---

## 4. NixOS / Hyprland 向け再現検証手順（未実測項目）

本環境は macOS のため、NixOS / Hyprland（ネイティブ Wayland + WebKitGTK + Fcitx5 + Mozc）での実機確認は**未実施**である。NixOS 実機では以下をそのまま実行して検証する：

```sh
# 1. Nix Flakes 開発環境へ入る（Wayland / WebKitGTK / GTK3 / herdr-core 0.1.22 自動導入）
nix develop

# 2. バックエンド単体・統合テスト
(cd src-tauri && cargo test -- --nocapture)

# 3. フロントエンドテスト・ビルド
pnpm install --frozen-lockfile
pnpm typecheck && pnpm test -- --run

# 4. ネイティブ Wayland で Tauri アプリを起動（GDK_BACKEND=wayland,x11 は shellHook で設定済み）
pnpm tauri dev
```

NixOS 実機での確認観点：
- `GDK_BACKEND=wayland` 下で WebKitGTK ウィンドウが正常描画されること（GPU 描画不具合が出る場合は `WEBKIT_DISABLE_DMABUF_RENDERER=1` の要否を確認）
- Fcitx5 + Mozc でのインライン未確定文字列表示・候補位置・変換確定 Enter 挙動
- `wterm` の DOM モードと WebGL モードの安定性差異

---

## 5. 残存する問題・Phase 1 への申し送り

1. **実機未検証 2 件**: macOS WKWebView での日本語 IME 手動対話テスト、および NixOS / Hyprland 実機でのビルド・Wayland/Fcitx5 動作確認。
2. **比較用 4 エンジンの削除**: Phase 0 PoC で比較用に導入した `@gespenst/core`・`ghostty-web`・`@xterm/*`・`restty` を Phase 1 開始時に削除する（バンドルサイズ・`WebContent` RSS 削減、および `@gespenst/core` 起因の `node:url` externalization 警告解消）。
3. **配布ビルドの最適化**: `src-tauri/Cargo.toml` に `[profile.release] lto = true; strip = true; codegen-units = 1` を追加し、配布バイナリをさらに軽量化する。また `herdr-core` バイナリを Tauri sidecar として同梱するか、外部依存（Nix/Homebrew/自動取得）とするかを Phase 1 で最終決定する。
4. **ドキュメント整合**: [docs/plan/architecture.md](../plan/architecture.md) の国際化欄に `i18next` が残っているため、Phase 0 の方針決定（TypeScript 辞書ベース・`i18next` 非導入）に合わせて更新する。

---

## 6. Phase 0 最終技術選定判断

| 領域 | 採用技術 | 最終判断 | 根拠 |
| --- | --- | --- | --- |
| デスクトップ基盤 | **Tauri 2** (`2.12.7`) | **採用確定** | Raw Binary IPC（Channel + Request）がゼロ Base64 で高速・簡潔。`.app` 13.73 MiB と軽量。 |
| 端末セッション基盤 | **`herdr-core`** (`v0.1.22`) | **採用確定** | 10 端末 17.3 MiB、20 秒連続出力でもメモリ完全有界、`AUTOMATIC_INPUT` (kind 6) / クレジットフロー制御 / OSC 7501 / OpenRPC を完備。 |
| 端末描画・フロント VT | **`wterm`** (`@wterm/dom` + `@wterm/ghostty`) | **採用確定（DOM 標準）** | `libghostty` WASM、`keyCode === 229` を含む最も堅牢な IME 構造、`setRenderingPaused(true)`、`dispose()` 後 99.5% メモリ解放。`gespenst` / `ghostty-web` は IME に致命的懸念があり不採用。 |
| フロント UI | **React 19 + React Compiler + Tailwind v4 + Base UI + Zustand Vanilla + Lucide + Valibot** | **採用確定** | 全コンポーネントが Vite+ で 0 errors / 0 warnings で統合動作。端末出力をストアに流さない構成を実証済み。 |
| 国際化 | **TypeScript 辞書**（`i18next` 非導入） | **採用確定（変更）** | 依存ゼロ・型安全・即時切替可能。 |
| TypeScript 設定 | **外部 Node.js 24 LTS + JSON-RPC over stdio** | **採用確定** | ネイティブ型除去によりトランスパイラ不要。起動 32〜50 ms・RSS 約 35 MiB、アプリへのランタイム同梱不要。 |
| 開発ツールチェーン | **Vite+ (`vite-plus`) + pnpm + Nix Flakes** | **採用確定** | ビルド 578〜745 ms、テスト 134〜185 ms。`pnpm dev/build` 経由で Tauri CLI・Nix と安定連携。 |

---

## 7. Phase 0 最終検証・最終性能測定（2026-11-26 統合後）

両エージェント（品質保証・実装整理）の成果が `main`（`5ea6c3d`）へマージされた後の最終構成に対し、ローカル作業ツリーの [src-tauri/src/commands.rs](../../src-tauri/src/commands.rs) 調整（デフォルトログインシェル `-l` 付与および XDG ディレクトリ復元）を含む状態で最終検証と最終性能測定を実施した。

### 7.1 ユーザーによる macOS 実機手動検証結果

macOS 15.5 (`arm64`) の Tauri 実ウィンドウ（WKWebView + `wterm` DOM + `libghostty` WASM + `herdr-core` v0.1.22）において、ユーザーによる手動検証で以下を確認済み：

- [x] ターミナルの基本操作
- [x] `fish`・Neovim の動作
- [x] 日本語 IME による入力・変換・確定
- [x] 変換確定時の Enter 誤送信がないこと
- [x] Backspace による日本語文字の削除
- [x] 変換候補ウィンドウの表示位置

保留・次フェーズ以降への持ち越し：
- **Nerd Font の表示**: Phase 2（ターミナル実装）で対応するため、Phase 0 では未実装。
- **NixOS / Hyprland 実機検証**: 実機が現在利用できないため、MVP 完成条件（Phase 6）まで保留。

### 7.2 自動検証・ビルド・実ウィンドウ確認結果

| 確認項目 | コマンド | 結果 |
| --- | --- | --- |
| TypeScript 型検査 | `pnpm typecheck` (`tsc --noEmit`) | **合格**（0 errors） |
| Lint | `pnpm lint` (`vp lint`, 23 files / 96 rules) | **合格**（0 warnings, 0 errors, 203 ms） |
| フロントエンドテスト | `pnpm test -- --run` (Vitest 5.1.1) | **合格**（2 files / 13 passed, 134 ms） |
| フロントエンドビルド | `pnpm build` (`vp build`) | **合格**（578〜606 ms, 警告 0 件） |
| Rust Clippy | `cargo clippy --locked --all-targets -- -D warnings` | **合格**（0 warnings） |
| Rust 単体・統合テスト | `cargo test --locked -- --nocapture` | **合格**（unit 7/7 + integration 5/5 passed） |
| CLI 互換統合テスト | `cargo test --locked installed_cli_compatibility_in_isolated_pty -- --ignored` | **合格**（1/1 passed: `fish`, `nvim`, `codex`） |
| リリースビルド | `pnpm tauri build --bundles app` | **合格**（`uiterm.app` **4.22 MiB**、31.98 s） |
| Tauri 実ウィンドウ動作確認 | `scripts/bench-app.py` 経由で `uiterm.app` を起動・実操作 | **合格**（WKWebView 描画・隔離 `herdr-core` 起動・`fish` セッション稼働・PTY 入出力を確認） |
| セッション終了時リソース解放 | `osascript` による GUI 正常終了（計 6 回）+ `post-close` 計測 | **合格**（全 6 回で `uiterm`・`WebKit.*`・`herdr-core`・`holder`・`fish` の残存 PID 0 件、`/tmp/uiterm.*` 削除確認） |

### 7.3 最終性能測定結果（今後の開発基準値）

測定環境: macOS 15.5 (Darwin 24.5.0, Apple Silicon `arm64`)、`scripts/bench-app.py`（`ps` / `lsof` / `footprint`）を使用。

> [!NOTE]
> **比較・解釈上の注意**: 過去の PoC 値や既存稼働中の Ghostty / Herdr は測定条件（計測点・プロセス帰属・稼働中の端末数・共有ページの扱い）が異なるため、直接の優劣比較には使わず条件差を明記して併記する。推定値と実測値は明確に区別する。

#### (1) 配布容量・フロントエンドバンドルサイズ

| 対象 | 旧 PoC（整理前） | 最終実測（統合後） | 変化・備考 |
| --- | ---: | ---: | --- |
| 配布用 `uiterm.app`（論理サイズ） | 13.73 MiB | **4.22 MiB** (4,422,468 B) | **-69.3%**。内訳: `uiterm` 3.56 MiB + `poc_bench` 672.0 KiB + 資材 |
| `herdr-core` v0.1.22（macOS arm64） | 6.30 MiB | **6.30 MiB** (6,603,232 B) | 必須バックエンドバイナリ |
| **実質配布容量（`uiterm.app` + `herdr-core`）** | 20.03 MiB | **10.51 MiB** (11,025,700 B) | **-47.5%**（`poc_bench` 移設で約 9.85 MiB へさらに削減可能） |
| フロントエンド `dist/` 合計 | 3.4 MB | **1.1 MiB** | `index.js` 476.94 kB (gzip 147.12 kB), `index.css` 21.66 kB (gzip 5.67 kB), `ghostty-vt.wasm` 599.62 kB (gzip 199.83 kB), `wasm-inline.js` 73.47 kB (gzip 31.96 kB, 実行時未ロード) |
| *(参考)* Ghostty 1.4.0 `.app` | 67 MiB (du) | **64.18 MiB**（論理サイズ） | 同一マシン `/Applications/Ghostty.app` |
| *(参考)* Herdr 0.1.16 `.app` | 59 MiB (du) | **58.84 MiB**（論理サイズ） | 同一マシン `/Applications/Herdr.app`（別途 `herdr-core` 6.30 MiB / CLI `herdr` 0.9.5 23.37 MiB） |

#### (2) 起動時間（`scripts/bench-app.py` 計測点: プロセス起動 → `window_visible`）

| 区分 | 試行数 | 中央値 | 範囲 | 備考 |
| --- | ---: | ---: | --- | --- |
| ビルド後初回起動（同一ログインセッション内） | 1 | **477.8 ms** | — | 真のログイン後コールド起動（OS キャッシュ未ロード）は未実測 |
| **ウォーム起動**（GUI 正常終了→PID 消滅後に再起動） | 5 | **158.5 ms** | **148.2〜177.6 ms** | 全 5 回で GUI 終了後の残存プロセス 0 件・ソケット削除を確認 |

※ §2.2 の旧 PoC 値（コールド 1,274.2 ms / ウォーム 534.1 ms）は「初回プロンプト到達」を計測点としていたため、今回の `window_visible` とは計測点が異なる。

#### (3) アイドル時 CPU・アプリ全体 RSS / Physical Footprint（1 端末安定後 20 秒・11 サンプル）

| 指標 | 中央値 | 範囲 | 備考 |
| --- | ---: | --- | --- |
| アプリ全体 RSS 合計（参考値・共有ページ重複あり） | **242.42 MiB** | 242.19〜244.36 MiB | 全 7 プロセス合計 |
| **アプリ全体 Summary Physical Footprint（実測）** | **120.48 MiB** | **120.37〜123.99 MiB** | `footprint` コマンドによる実物理メモリ消費 |
| アイドル時 CPU 合計（`ps` 平滑化値） | **2.3%** | 0.6〜4.3% | 主に `uiterm` + `WebKit.WebContent`（カーソル描画等） |

**プロセス別内訳（`idle-1` 最終サンプル、計 7 プロセス）**:

| プロセス | RSS | Physical Footprint | CPU |
| --- | ---: | ---: | ---: |
| `com.apple.WebKit.WebContent` | 79.34 MiB | 60.69 MiB | 2.3% |
| `uiterm` | 79.02 MiB | 23.16 MiB | 2.0% |
| `com.apple.WebKit.GPU` | 43.17 MiB | 18.49 MiB | 0.0% |
| `com.apple.WebKit.Networking` | 14.95 MiB | 5.19 MiB | 0.0% |
| `herdr-core`（daemon） | 10.30 MiB | 4.52 MiB | 0.0% |
| `herdr-core`（holder） | 5.22 MiB | 3.03 MiB | 0.0% |
| `fish`（ログインシェル） | 10.36 MiB | 6.06 MiB | 0.0% |
| **合計 / Summary** | **242.36 MiB** | **120.51 MiB** | **4.3%** |

**参考比較（同一マシン上の既存稼働中プロセスを読み取り専用計測・条件非同一）**:

| 対象（既存稼働中・条件非同一） | プロセス数 | RSS 合計中央値 | Summary Footprint 中央値 | CPU 中央値 | 条件差 |
| --- | ---: | ---: | ---: | ---: | --- |
| Ghostty 1.4.0（本体 PID 単体） | 1 | 34.44 MiB | 129.57 MiB | 0.0% | 既存ユーザーセッション稼働中（配下に `herdr`・`fish`×12・`codex`×2 の計 19 プロセス、合計 Footprint 393.00 MiB） |
| Herdr 0.1.16 + `herdr-core` + `fish` | 5 | 54.73 MiB | 153.98 MiB | 0.25% | 既存ユーザーセッション稼働中（`Herdr` 本体 Footprint 122.22 MiB + `herdr-core` 群 16.04 MiB + `fish` 16.22 MiB） |

#### (4) 1・5・10 端末スケーリング（バックエンド実測・フロントエンド単体実測）

実ウィンドウ UI 上での 5 / 10 タブ自動生成は未実施（macOS Accessibility 自動化の制約のため。Phase 1 以降で継続検証）。構成要素ごとの実測値は以下の通り（両者の単純加算は実ウィンドウの実測値ではないため推定合算は行わない）：

**A. バックエンド + 実シェル（隔離 `herdr-core` + `holder` + `fish -i`、`footprint` / `ps` 実測）**:

| 端末数 | 生成時間（増分） | プロセス数 | RSS 合計 | Summary Footprint | CPU |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 端末 | 34.4 ms | 3 | 23.36 MiB | **11.61 MiB** | 0.0% |
| 5 端末 | 46.1 ms (+4) | 7 | 62.67 MiB | **33.57 MiB** | 0.1% |
| 10 端末 | 53.2 ms (+5) | 12 | 113.80 MiB | **62.97 MiB** | 0.0% |
| 全 `kill` 後 | — | 2 | 16.83 MiB | **9.38 MiB** | 0.0% |

**B. フロントエンド VT/WASM 単体（Node.js 24 ヘッドレス、`@wterm/ghostty` `scrollbackLines=2000`、10,000 行負荷）**:

| 端末数 | 生成時間 | 10k 行解析 | スループット | アイドル RSS 増分 | 負荷後 RSS 増分 | 破棄+GC 後 RSS 残 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 端末 | 0.2 ms | 9.1 ms | 106.6 MB/s | +0.97 MB | +4.98 MB | +5.13 MB |
| 5 端末 | 0.6 ms | 9.2 ms | 105.7 MB/s | +4.88 MB | +24.95 MB | +0.74 MB |
| 10 端末 | 1.2 ms | 9.1 ms | 107.3 MB/s | +9.63 MB | +49.83 MB | **-0.06 MB** |

#### (5) 大量出力時の負荷（実ウィンドウ 1 端末 + `poc_bench` バックエンド単体）

- **実ウィンドウ 1 端末（`scripts/bench-app.py workload --mib 5` × 3 回連続バースト、23 サンプル）**:
  - 5 MiB 出力〜完了マーカー到達所要時間: **547.1 ms / 362.5 ms / 371.1 ms**（2〜3 回目は実ウィンドウ描画込みで約 **13.5〜13.8 MiB/s**）
  - RSS 中央値 **387.80 MiB**（範囲 242.36〜391.19 MiB）
  - Summary Footprint 中央値 **271.01 MiB**（範囲 120.53〜403.59 MiB）
  - CPU 中央値 **2.6%**（最大 **119.3%**）
- **バックエンド単体（release `poc_bench`、`opt-level="s"` + LTO + strip）**:
  - 5 MiB バースト: **74.27 ms**（**68.17 MiB/s**）、ストール中 ch0 制御 RPC **0.24 ms**（`opt-level="s"` による顕著な性能劣化なし）
  - 10 端末同時生成: **78.56 ms**、全破棄後 `live_after_kill=0`、停止後 `core_still_alive=false`, `holder_still_alive=false`
  - TypeScript 設定ランタイム（Node.js 24 JSON-RPC stdio）: 起動 **33.71〜34.56 ms**（初回 123.04 ms）、RPC **21.30〜21.62 ms**、RSS **33.18〜35.62 MiB**

#### (6) 長時間連続出力時のメモリ増加傾向（実ウィンドウ 1 端末・60 秒間 256 KiB/s 連続出力、計 ~15 MiB）

※ 30 分 soak テストは未実施（60 秒連続出力での傾向実測。長期利用の本格検証は Phase 6 で実施）。

| 経過時間 | RSS 合計 | Summary Footprint | CPU | `WebContent` FP | `WebKit.GPU` FP | `uiterm` FP | `herdr-core` FP |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `t = 0.0 s`（バースト直後） | 358.5 MiB | 271.4 MiB | 0.5% | 187.8 MiB | 23.9 MiB | 24.1 MiB | 23.7 MiB |
| `t = 15.3 s` | 409.4 MiB | 457.3 MiB | 35.6% | 287.4 MiB | 95.6 MiB | 24.5 MiB | 23.8 MiB |
| `t = 30.5 s` | 432.4 MiB | 458.9 MiB | 23.8% | 297.9 MiB | 86.6 MiB | 24.6 MiB | 23.8 MiB |
| `t = 45.8 s` | 422.0 MiB | 488.0 MiB | 29.7% | 322.3 MiB | 91.2 MiB | 24.6 MiB | 23.9 MiB |
| `t = 61.0 s`（出力完了直後） | 436.4 MiB | 484.7 MiB | 23.1% | 327.3 MiB | 95.4 MiB | 24.6 MiB | 23.9 MiB |
| `t = 66.1 s`（停止 6 秒後） | 441.6 MiB | **351.3 MiB** | 2.9% | **266.8 MiB** | **24.0 MiB** | **24.5 MiB** | **23.9 MiB** |

- **`uiterm`（Rust ホスト）および `herdr-core`（daemon + holder）**: 60 秒連続出力を通して +0.2〜+0.5 MiB の完全フラット（リーク兆候なし）。
- **`WebKit.GPU`**: 負荷中 84〜96 MiB へ上昇するが、出力停止 6 秒後に **24.0 MiB** へ即時復帰。
- **`WebKit.WebContent`**: 連続出力中に 187.8 MiB → 328.6 MiB まで上昇し、出力停止後に 266.8 MiB まで降下したが、アイドル時（60.7 MiB）より高い水準に留まる。JSC の GC タイミング・DOM 描画バッファの回収挙動を含め、Phase 1〜6 で継続監視・対策を行う。

#### (7) セッション破棄後のリソース解放（`post-close`）

- **GUI 正常終了**: `post-close` 計測およびウォーム起動 5 回の計 6 回すべてで、`app_alive: false`、関連プロセス数 **0**、RSS **0 KiB**、`/tmp/uiterm.*` ソケットディレクトリ削除を確認。
- **強制終了（`SIGTERM` 等）**: `RunEvent::Exit` が発火せず `herdr-core` / `holder` / シェルと `/tmp/uiterm.*` が残留することを確認済み（Phase 1 のプロセスライフサイクル管理で対応）。

---

## 8. Phase 1 以降への申し送り事項

1. **セキュリティ強化（Phase 1・配布前必須）**:
   - Tauri Capabilities の最小権限化（`AppManifest::commands` とウィンドウ限定）および CSP 設定（現状 `null`）。
   - TypeScript 設定の実行信頼境界（明示的に信頼したパスのみ実行）と `herdr-core` 配布形態（sidecar 同梱 vs 外部検証）の確定。
2. **プロセスライフサイクル管理（Phase 1）**:
   - `SIGTERM` / `SIGINT`・クラッシュ時に `herdr-core` / `holder` / シェルが残留する問題への対策（シグナルハンドリングまたは親プロセス死活監視）。
   - `poc_bench` の `examples/` 移設による `.app` 672 KiB 削減。
   - `flake.nix` からの `x86_64-darwin` 除外および macOS 最小ターゲット整合。
   - [docs/plan/architecture.md](../plan/architecture.md) の国際化欄を TypeScript 辞書ベースへ整合。
3. **ターミナル機能・描画改善（Phase 2）**:
   - **Nerd Font の表示対応**。
   - `herdr-core` 上流への PTY 初期化時 `IUTF8` フラグ有効化提案（`cat` 等の canonical モード CLI での全角削除対応）。
   - 連続大量出力後の `WebKit.WebContent` メモリ回収挙動の調査・最適化、および実ウィンドウでの 5 / 10 タブ負荷計測・30 分 soak テスト（Phase 6）。
4. **未検証項目の継続追跡（MVP 完成条件・Phase 6）**:
   - NixOS / Hyprland（ネイティブ Wayland + WebKitGTK + Fcitx5 + Mozc）実機検証。
   - 真のログイン後コールド起動計測および Ghostty / Herdr との同一条件比較。

---

## 9. Phase 0 完了判定

重大な不具合は検出されず、macOS arm64 実機での基本操作・`fish`/`nvim`・日本語 IME 動作、自動テスト・リリースビルド・GUI 正常終了時のリソース解放、および今後の基準値となる性能測定をすべて完了した。**Phase 0 を完了とし、Phase 1（基盤構築）へ移行可能と判断する。**
