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
| 開発ツールチェーン | **Vite+ (`vite-plus`) + pnpm + Nix Flakes** | **採用確定** | ビルド 666〜745 ms、テスト 185 ms。`pnpm dev/build` 経由で Tauri CLI・Nix と安定連携。 |
