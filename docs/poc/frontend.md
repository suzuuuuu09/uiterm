---
description: "uiterm Phase 0 フロントエンド・ターミナル描画の技術検証レポート"
---

# uiterm Phase 0 フロントエンド・ターミナル描画 検証レポート

## 1. 検証概要と検証範囲

本ドキュメントは、uiterm Phase 0 におけるフロントエンド基盤およびターミナル描画ライブラリの独立した技術評価・実装検証結果をまとめたものである。

### 1.1 検証環境

| 項目 | 内容 |
| --- | --- |
| OS / CPU | macOS 15.5 (Build 24F74) / Apple Silicon (`arm64`) |
| Node.js / pnpm | Node.js `v24.22.2` / pnpm `12.9.0` |
| ツールチェーン | `vite-plus@1.8.0` (Vite 8.3 / Rolldown / Vitest 5.1.1 / Oxlint 1.94.0 / Oxfmt 0.79.0) |
| 比較対象ネイティブ環境 | Ghostty `1.4.0` (67 MB) / Herdr `0.9.5` (59 MB + `herdr-core`) |

### 1.2 検証済み項目と未検証項目の明確化

- **実測・実装検証済み**：
  - 候補ライブラリ全5種の npm パッケージ実体・公開ソースコード・型定義・WASM/JS アーティファクトサイズの調査
  - Node.js (`--expose-gc`) 上での `libghostty` WASM コア（`@wterm/ghostty`, `@gespenst/core/headless`, `ghostty-web`）の 1 / 5 / 10 端末ベンチマーク（スループット・RSS 増加・破棄後メモリ解放・自動応答生成）
  - Vite+ / React 19 / React Compiler / Tailwind CSS v4 / Base UI / Zustand Vanilla / TypeScript 辞書 i18n / Lucide / Valibot を統合したフロントエンド PoC の構築・型検査・ビルド・単体テスト・Lint
  - [docs/poc/interface.md](./interface.md) に準拠した `UitermBackendClient`（`TauriIpcClient` / `MockIpcClient`）との接続実装
- **未検証（実機テストが必要）**：
  - **Tauri WKWebView（macOS 実機）** 上での macOS 標準日本語 IME による対話入力（変換候補ウィンドウの厳密な座標追従・確定時 Enter 挙動の目視確認）
  - **Linux（NixOS / Hyprland / ネイティブ Wayland / WebKitGTK）** および **Fcitx5 + Mozc** での実機動作・IME 入力・フォントフォールバック
  - ブラウザ / WebView 実描画パイプライン（DOM vs WebGL2 vs WebGPU）における GPU メモリ使用量と 60fps スクロール体感

> [!IMPORTANT]
> 本レポートでは、ソースコード解析およびヘッドレス実測で確認できた事実と、実機（WKWebView / WebKitGTK + 実 IME）で未検証の事項を明確に区別して記載している。

---

## 2. ターミナル描画ライブラリの調査・比較

### 2.1 調査対象（2026年11月時点の最新リリース）

1. **`@wterm/dom@0.6.0` + `@wterm/ghostty@0.6.0`** (`vercel-labs/wterm`)
   - DOM テキスト描画を基本とし、オプションで `renderer: "webgl"`（WebGL2 ライブ描画＋DOM テキスト選択/アクセシビリティ維持）のハイブリッド構成に対応。
   - コアは軽量 Zig WASM（`@wterm/core`: 53.8 KB）と `libghostty` WASM（`@wterm/ghostty`: 585.6 KB）を差し替え可能。
2. **`@gespenst/core@0.2.0`** (`tobilg/gespenst`)
   - `libghostty` WASM（1,162 KB）を専用 Web Worker（`OffscreenCanvas`）で実行し、WebGPU / WebGL2（背景）＋ Canvas 2D（文字）のハイブリッド描画を行う。
3. **`ghostty-web@0.4.0`** (`coder/ghostty-web`)
   - `libghostty` WASM（413.1 KB）＋ Canvas 2D レンダラ。`xterm.js` 互換 API を提供。
4. **`@xterm/xterm@6.0.0`** + `@xterm/addon-fit@0.11.0` + `@xterm/addon-webgl@0.19.0`
   - 純 TypeScript/JS の VT エンジン＋ DOM / Canvas / WebGL レンダラ。最も実績が長い。
5. **その他の有力候補：`restty@0.3.0`**
   - `libghostty-vt` WASM ＋ WebGPU/WebGL2 ＋ TypeScript 独自テキストシェイパー。パッケージ総容量が **9.2 MB**（JS チャンク単体 2.5 MB + 2.1 MB）と非常に重く、フォントフォールバックが Local Font Access API（Safari/WKWebView/WebKitGTK 非対応）や明示的なフォント URL 指定に依存するため、uiterm の「Lightweight / Japanese First / WebKit 前提」の原則とは相性が悪い。

### 2.2 評価基準別 比較マトリクス

| 評価基準 | `wterm` + `@wterm/ghostty` (0.6.0) | `@gespenst/core` (0.2.0) | `ghostty-web` (0.4.0) | `@xterm/xterm` (6.0.0) | `restty` (0.3.0) |
| --- | --- | --- | --- | --- | --- |
| **VT・ANSI 互換性** | **◎** `libghostty` WASM（Kitty keyboard/images, OSC 133, 2026 sync, reflow） | **◎** `libghostty` WASM（最新版フル機能） | **○** `libghostty` WASM（一部 API 未露出） | **○** 独自 JS パーサ（Kitty 系・一部最新拡張で差） | **◎** `libghostty-vt` WASM |
| **日本語 IME（コード解析）** | **◎** `<textarea>` をカーソル位置へ配置・可視化。`composing \|\| isComposing \|\| keyCode === 229` ガードあり。`compositionend` 後の重複 `input` を 250ms 窓で除去 | **✕（致命的懸念）** `handleKeyDown` が `isComposing` / `keyCode === 229` を検査せず **無条件で `e.preventDefault()`** を呼ぶ実装（WebKit 等で IME 入力が阻害される恐れ大） | **✕（致命的懸念）** `<textarea>` を持たず `<div tabindex="0">` に直接 `composition*` を貼る実装。WebKit での未確定文字列表示・候補位置が成立しない | **○** 実績ある隠し `<textarea>` + `compositionView`。ただし WebKit の `insertReplacementText`（Issue #6284）等の既知課題あり | **△** `event.isComposing` のみチェック（Safari 向け `keyCode === 229` ガードなし） |
| **全角・半角・絵文字** | **◎** `libghostty` の Grapheme cluster / East Asian Width 処理＋ブラウザネイティブ描画 | **◎** `libghostty` ＋ Canvas 2D シェイピング | **○** `libghostty` ＋ Canvas 2D 2パス描画 | **○** Unicode11 addon 等が必要。WebGL アトラスは大量の漢字でメモリ増 | **○** `libghostty` ＋ 独自シェイパー（外部フォント依存） |
| **フォントフォールバック** | **◎** CSS `font-family` による OS ネイティブフォールバック（日本語フォント自動適用） | **○** Canvas 2D テキスト描画のため CSS フォントスタックを利用可能 | **○** Canvas 2D のため CSS フォントスタックを利用可能 | **○** DOM/Canvas は良好、WebGL はグリフアトラス再生成コストあり | **✕** 独自シェイパーのため WebKit でシステム日本語フォント自動フォールバック困難 |
| **VT 描画・解析性能** | **◎** 10k 行解析 10.1〜14.5ms (67〜96 MB/s)。`renderer: "webgl"` 切替可 | **◎** 10k 行解析 4.0〜15.7ms (62〜242 MB/s) + Worker 分離 | **○** 10k 行解析 14.5〜27.6ms (35〜67 MB/s)、Canvas 2D のみ | **○** 実用十分だが JS パーサ＋大量出力時の GC 負荷あり | **◎** WebGPU/WebGL2 高速 |
| **メモリ使用量・破棄解放** | **◎** 端末ごとに独立 WASM インスタンスのため **`dispose()` 後に 99.5% 解放**（実測確認） | **△** 共有 WASM ヒープのためピークは低いが **`dispose()` 後も OS へ返却されない** | **△** 共有 WASM インスタンスのため **`dispose()` 後も線形メモリが縮小しない** | **○** JS ヒープなので GC で解放されるが、WebGL コンテキスト上限に注意 | **✕** 初期ロードだけで 4.6 MB 超の JS/WASM |
| **バンドルサイズ (Raw / Gzip)** | WASM `585.6 KB` (`193.8 KB`) + JS 約 `190 KB` (`~45 KB`) | WASM `1,162 KB` (`348.5 KB`) + JS `157 KB` (`43.4 KB`) | WASM `413.1 KB` (`121.5 KB`) + JS `665.9 KB` (`189.2 KB`) | JS `336.9 KB` (`86.4 KB`) + WebGL `123.6 KB` (`35.1 KB`) | パッケージ総量 **9.2 MB** |
| **WebKit 互換性** | **◎** 標準 DOM ベースのため WKWebView / WebKitGTK で安定。WebGL 非対応時も DOM にフォールバック | **△** デフォルトの Worker + `OffscreenCanvas` + WebGPU は WebKitGTK / WKWebView で白画面・コンテキスト喪失リスク（`worker: false` 回避可） | **△** Canvas 2D は動くが IME 構造が WebKit 非適合 | **○** 動作実績多いが WebKit 固有 IME バグあり | **✕** WebGPU / Local Font Access 前提で WebKitGTK 不利 |
| **herdr-core 接続性** | **○** `write(Uint8Array)` 対応。ただし `onData` に自動応答が混ざるため、`write()` 同期区間ガードで分離（PoC で実証済み） | **◎** `input` イベントに `source: 'reply'` を標準付与（分離が最も綺麗） | **△** `onData` に混在。API 抽象度が低く制御しにくい | **△** `write()` が非同期バッファ経由のため、自動応答の厳密分離が困難 | **△** 独自トランスポート前提 |
| **非表示端末の負荷制御** | **◎** `setRenderingPaused(true)` を標準装備（VT 解析のみ継続し描画停止） | **△** 明示的な描画停止 API なし（v0.2.0） | **△** 明示的な描画停止 API なし（v0.4.0） | **△** rAF 任せで明示的な停止 API なし | **△** 明示的な停止 API なし |
| **API 安定性・保守性** | **○** `0.6.0`（活発に更新、モジュール分離良好、Vercel Labs） | **△** `0.2.0`（設計は先進的だが初期段階） | **△** `0.4.0`（Coder 発だが IME/描画機能が限定的） | **◎** `6.0.0`（VS Code 採用、極めて成熟） | **△** `0.3.0`（機能過多・巨大） |

---

## 3. 日本語対応の検証結果

### 3.1 ソースコード解析による IME 実装の評価

1. **`@wterm/dom@0.6.0` (`node_modules/@wterm/dom/dist/input.js`)**：
   - `handleKeyDown` 先頭で `if (this.composing || e.isComposing || e.keyCode === 229)` を判定し、`e.preventDefault()` を呼ばずに return する。これにより、Safari / WKWebView 特有の「`compositionend` が Enter の `keydown` より先に発火する」バグ（WebKit Bug #165004）による **変換確定時 Enter 誤送信** を `keyCode === 229` で防げる構造になっている。
   - `handleCompositionStart` で隠し `<textarea>` を `positionTextarea()` によりカーソルセル座標（`col * charWidth`, `row * rowHeight`）へ移動し、`opacity: 1`・端末の前景色/背景色で可視化、`scrollWidth` に応じて横幅を拡張する。これにより **未確定文字列のインライン表示** と **OS 標準 IME の変換候補ウィンドウ位置追従** が成立する設計となっている。
   - `handleCompositionEnd` 後に一部ブラウザが発火する重複 `input` イベントを `COMPOSITION_INPUT_DEDUP_MS = 250` で重複排除している。
   - DOM テキスト描画のため、ブラウザのネイティブ選択・コピーおよび CSS `font-family` による日本語フォントフォールバック（`"Hiragino Sans"`, `"Noto Sans Mono CJK JP"` 等）がそのまま機能する。
   - **注意点**：`renderer: "webgl"` モードでは出力中は WebGL2 キャンバスに描画し、アイドル時に DOM テキストを同期する仕組みのため、高速出力中の選択や IME 重畳表示の挙動は WKWebView 実機で追加確認が必要。
2. **`@gespenst/core@0.2.0` (`node_modules/@gespenst/core/dist/index.js`)**：
   - `handleKeyDown(e)` が `isComposing` や `keyCode === 229` を検査せずに末尾で無条件に `e.preventDefault()` を実行している。この仕様ではブラウザ（特に WebKit）の IME 変換パイプラインが阻害されるため、現状のままでは日本語最優先の uiterm に採用できない。
3. **`ghostty-web@0.4.0` (`node_modules/ghostty-web/dist/ghostty-web.js`)**：
   - `<textarea>` を生成せず、非編集要素の `<div tabindex="0">` に `compositionstart` / `compositionend` を登録している。WebKit では非編集要素上で日本語 IME の未確定文字列表示や変換候補ウィンドウのアンカーが正しく機能しない。

### 3.2 日本語・文字幅テストパターンの実装

PoC（[src/transport/mockHerdrTransport.ts](../../src/transport/mockHerdrTransport.ts) および [src/App.tsx](../../src/App.tsx)）に以下の検証ツールを実装した：
- 全角・半角の桁揃え比較（ASCII 40 桁 vs 全角 20 文字）
- 曖昧幅文字（`○●△▲□■☆★※→←↑↓`）、半角カナ、絵文字・ZWJ 結合（`👨‍💻 🇯🇵 👨‍👩‍👧‍👦 👍🏽`）、NFD 結合文字（`か\u3099`, `e\u0301`）、罫線ボックス
- キャプチャフェーズでの IME 診断ログ（`compositionstart` / `compositionupdate` / `compositionend` / `keydown(isComposing, keyCode)` / `onUserInput`）

### 3.3 プラットフォーム・IME 別の検証状況

| 環境 | 状態 | 所見 |
| --- | --- | --- |
| macOS 15.5 + ブラウザ / WKWebView + macOS 標準 IME | **コード・PoC 構築済み（実機対話入力は未検証）** | `@wterm/dom` は `keyCode === 229` ガード・カーソル追従 `<textarea>`・重複排除を完備しており構造的に適合。Phase 0 完了前に Tauri 実ウィンドウ上で「変換確定 Enter・変換中ショートカット（Cmd/Ctrl 系）・長文変換・選択コピー」の手動実機確認を行うこと。 |
| Linux (NixOS / Hyprland / Wayland) + WebKitGTK + Fcitx5 + Mozc | **未検証（本環境は macOS のため実行不可）** | WebKitGTK + Wayland 環境では `OffscreenCanvas`/WebGL の不安定性や Fcitx5 のプリエディット位置ずれが既知のリスク。DOM ベース＋`<textarea>` カーソル配置を採る `wterm`（`renderer: "dom"`）が最もリスクが低いと評価されるが、必ず実機で検証すること。 |

---

## 4. UI 技術スタック互換性検証

すべての採用予定技術をフロントエンド PoC に組み込み、`pnpm typecheck` / `pnpm build` / `pnpm test -- --run` / `pnpm lint` が **0 errors / 0 warnings** で通過することを確認した。

| 技術 | 検証バージョン | 互換性・所見 |
| --- | --- | --- |
| **Vite+** | `vite-plus@1.8.0` | `vp dev` / `vp build` (643ms) / `vp test` (Vitest 5.1.1) / `vp lint` (Oxlint 1.94.0) すべて正常動作。ただし `vite-plus` の `defineConfig` の型と `@vitejs/plugin-react` / `@tailwindcss/vite` の `Plugin` 型に型定義上の不整合があり、`vite.config.ts` で `as PluginOption[]` キャストが必要だった。また `vite/client` ではなく `vite-plus/client` を参照する必要がある。 |
| **React + React Compiler** | `react@19.3.1`, `babel-plugin-react-compiler@1.0.0`, `@rolldown/plugin-babel@0.3.1` | `vite.config.ts` で `@vitejs/plugin-react@6.3.0` の `reactCompilerPreset()` を `@rolldown/plugin-babel` 経由で適用し、正常にビルド・動作確認。 |
| **Tailwind CSS v4** | `tailwindcss@4.3.5`, `@tailwindcss/vite@4.3.5` | `@import "tailwindcss";` および `@theme` による日本語フォールバック込みモノスペースフォント定義が正常に動作。 |
| **Base UI** | `@base-ui/react@1.9.0` | 旧 RC パッケージ（`@base-ui-components/react`）ではなく正式版 `@base-ui/react` を採用。`Tabs` / `Switch` が React 19 + React Compiler + Tailwind v4 で問題なく動作。 |
| **Zustand Vanilla** | `zustand@5.1.0` (`zustand/vanilla`) | UI・タブメタデータのみを管理し、**ターミナル出力バイト列は一切ストアに流さない**構成（`TerminalSessionManager` → `TerminalAdapter.write(Uint8Array)` 直接パイプ）を実装・テストで確認。 |
| **TypeScript 辞書 i18n** | 自作 (`src/i18n/dict.ts`) | `i18next` 非導入。`ja` / `en` の型安全な辞書と即時切替を実装。バンドル増ゼロ。 |
| **Lucide** | `lucide-react@1.80.0` | Tree-shaking 含め正常動作。 |
| **Valibot** | `valibot@1.5.0` | リサイズ・バックエンド ID・セッションメタデータのスキーマ検証を実装・Vitest で検証済み。 |

> [!NOTE]
> `@gespenst/core@0.2.0` をバンドルに含めると、`vp build` 時に `node:url` / `node:fs/promises` の browser externalization 警告が発生する（Node.js 向けローダが混在しているため）。`@wterm/dom` + `@wterm/ghostty` のみの構成に絞ればこの警告は解消する。

---

## 5. バックエンド連携（`docs/poc/interface.md`）の検証

バックエンド担当者が定義した [docs/poc/interface.md](./interface.md) および [src/ipc/types.ts](../../src/ipc/types.ts) / [src/ipc/client.ts](../../src/ipc/client.ts) / [src/ipc/mock.ts](../../src/ipc/mock.ts) に従い、[src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts) を実装・接続した。

### 5.1 検証項目ごとの確認結果

1. **初期画面・継続的バイナリストリーム**：
   - `terminalAttach` の `onFrame` コールバックで `OutputFrame` (`kind: "data"`) の `Uint8Array` を受け取り、React / Zustand を介さず `adapter.write(frame.bytes)` へ直接渡す構成を実装。
   - フロー制御として 16 KiB ごとに `terminalAck(attachId, bytes)` をバッチ送信する仕組みを実装。
2. **ユーザー入力と端末自動応答の分離（最重要課題）**：
   - `docs/poc/interface.md` 第4章では、`terminalWrite(attachId, data, kind)` において端末自動応答に `kind: "automatic"`（`herdr-core` の `AUTOMATIC_INPUT` フレーム kind 6）を指定することが求められている。
   - **`wterm` (`@wterm/dom@0.6.0`) の挙動**：`WTerm` はユーザー入力と `bridge.getResponse()`（DA1/DA2/DSR/OSC 11 等への自動応答）を同じ `onData` コールバックへ流す。しかし、同期コアである `GhosttyCore` 使用時は `_drainResponses()` が `term.write(bytes)` の **同期実行中** に呼ばれる。
   - **実装した分離手法**：[src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts) で `this.isWriting = true` フラグで `term.write(bytes)` を囲み、`isWriting === true` 中の `onData` を `onAutoResponse`（`kind: "automatic"` または抑制）へ振り分けることで、バイト内容の推測に頼らず分離できることを確認した。
   - **注意点**：将来 `@wterm/core` の Web Worker コア（非同期書き込み）へ移行するとこの同期フラグは使えなくなるため、本番実装では `@wterm/dom` 上流へ `onResponse` コールバック分離を要望するか、薄いラッパーで `core.getResponse()` をフックする設計が望ましい。
3. **リサイズ・フォーカス**：
   - フロントエンド側リサイズ → `terminalResize(attachId, cols, rows)`。
   - バックエンド側 `OUTPUT_FRAME_RESIZE`（`kind: "resize"`）→ 後続 `DATA` 適用前に `adapter.resize(cols, rows)` を実行。
   - タブ切替時 → `terminalFocus(attachId, isActive)` を通知。
4. **再同期**：
   - `resyncSession()` にて `terminalDetach` → `terminalAttach` を再実行し、初期画面フルリペイントを受信するフローを実装。
5. **VT エンジン二重動作（`herdr-core` + フロントエンド `libghostty`）の影響**：
   - **CPU / 解析コスト**：10,000 行（約 1 MB）の解析時間は `@wterm/ghostty` で **10.1〜14.5 ms** と極めて高速であり、二重 VT 解析自体はボトルネックにならない。
   - **メモリコスト**：フロントエンド側にもスクロールバックを持つため、`scrollbackLines` の上限管理が重要（10 端末 × 5,000 行で約 89.5 MB 増加、ただし破棄時に 99.5% 回収可能）。バックエンド側 `herdr-core` とフロントエンド側でスクロールバック上限を適切に制限する必要がある。
   - **自動応答の競合**：`herdr-core` の `automatic_input`（kind 6）対応により、フロントエンドが `kind: "automatic"` を正しく付与すれば二重応答・誤入力は防止できる。

### 5.2 通信仕様（`docs/poc/interface.md`）に関する所見・提案（変更は未実施）

通信仕様は非常によく設計されており、致命的な問題はない。ただし実運用に向けて以下 2 点を検討事項として記録する：

1. **リサイズ通知のループ防止ガイドラインの明記**：
   - `OUTPUT_FRAME_RESIZE` を受けてフロントエンドが `adapter.resize(cols, rows)` を呼ぶと、描画ライブラリ側の `onResize` コールバックが再発火し、`terminalResize` をバックエンドへ送り返す可能性がある。
   - **提案**：フロントエンド側でバックエンド起点のリサイズ適用中は `terminalResize` 送出を抑止するルールを `docs/poc/interface.md` 第 3 章または第 7 章に追記する。
2. **初回アタッチ時の `terminalAck` タイミング**：
   - 初期クレジット 256 KiB に対し、16 KiB 未満の小さなチャンクしか届かないアイドル端末でも、一定時間経過またはフレーム単位で ack すべきかどうかの目安があると実装がブレない（現状の「16 KiB 以上または 1 フレームごと」で十分だが、大量出力時以外は 16 KiB 未満でも蓄積が 256 KiB に達することはないため 16 KiB 閾値で問題ないことを確認済み）。

---

## 6. 性能測定結果・測定条件・再現手順

### 6.1 測定条件と再現手順

#### A. ヘッドレス VT コア測定（実測済み）
- **再現コマンド**：`node --expose-gc scripts/bench-terminals.mjs`
- **負荷ペイロード**：ANSI SGR カラー＋日本語 UTF-8 を含む 10,000 行（合計 **996.1 KB**、200 行ずつチャンク投入）、グリッド `120x40`、`scrollbackLines = 5000`。
- **測定指標**：インスタンス生成時間、10,000 行投入時間・スループット (MB/s)、アイドル時 RSS 増分、全端末 10,000 行投入後 RSS 増分、`dispose()` + GC 後の残存 RSS 増分、破棄時間。

#### B. ブラウザ / Tauri 実描画ベンチマーク（PoC UI 内蔵）
- **再現手順**：`pnpm dev`（または `pnpm tauri dev`）を起動し、ツールバーの「描画エンジン」を選択して「性能測定」ボタンを押下。
- **測定項目**：1 / 5 / 10 端末の同時マウント時間、アクティブ端末 10,000 行出力、非表示端末 10,000 行出力（`setRenderingPaused` 有効/無効比較）、10 回タブ切替の平均レイテンシ、全端末破棄時間。

### 6.2 アーティファクトサイズ比較（実測）

| パッケージ / ファイル | Raw サイズ | Gzip サイズ |
| --- | ---: | ---: |
| `@wterm/core` (`wterm.wasm`) | 53.8 KB | 23.5 KB |
| `@wterm/ghostty` (`ghostty-vt.wasm`) | **585.6 KB** | **193.8 KB** |
| `@wterm/dom` (`wterm.js` + `input.js` + `renderer.js` + `gpu-renderer.js`) | 190.0 KB | 45.0 KB |
| `@gespenst/core` (`ghostty-vt.wasm`) | 1,162.0 KB | 348.5 KB |
| `@gespenst/core` (`index.js` + `worker.js`) | 157.3 KB | 43.4 KB |
| `ghostty-web` (`ghostty-vt.wasm`) | 413.1 KB | 121.5 KB |
| `ghostty-web` (`ghostty-web.js`) | 665.9 KB | 189.2 KB |
| `@xterm/xterm` (`xterm.mjs`) + `addon-webgl` + `addon-fit` | 462.4 KB | 122.3 KB |

### 6.3 1 / 5 / 10 端末 ヘッドレス VT ベンチマーク結果（実測）

#### `@wterm/ghostty@0.6.0`（端末ごとに独立した WASM インスタンス）

| 端末数 | 生成時間 | 10k 行解析 | スループット | アイドル RSS 増 | 負荷後 RSS 増 | **破棄+GC 後 RSS 残** | 破棄時間 | 自動応答数 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 41.0 ms (初回) | 14.5 ms | 67.3 MB/s | +7.95 MB | +22.44 MB | +21.80 MB *(初回 JIT/キャッシュ含む)* | 0.17 ms | 2 |
| 5 | 0.8 ms | 10.1 ms | **95.8 MB/s** | +4.97 MB | +44.98 MB | **+0.53 MB** | 0.11 ms | 2 |
| 10 | 1.5 ms | 10.3 ms | **94.9 MB/s** | +9.77 MB | +89.50 MB | **+0.45 MB** | 0.21 ms | 2 |

#### `@gespenst/core/headless@0.2.0`（共有 WASM ランタイム）

| 端末数 | 生成時間 | 10k 行解析 | スループット | アイドル RSS 増 | 負荷後 RSS 増 | **破棄+GC 後 RSS 残** | 破棄時間 | 自動応答数 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3.5 ms | 15.7 ms | 61.8 MB/s | -2.31 MB | +9.42 MB | +9.48 MB | 0.36 ms | 2 |
| 5 | 1.3 ms | 4.2 ms | **230.3 MB/s** | +0.09 MB | +21.16 MB | **+21.19 MB** | 0.73 ms | 2 |
| 10 | 2.1 ms | 4.0 ms | **242.0 MB/s** | +0.11 MB | +26.42 MB | **+26.66 MB** | 1.11 ms | 2 |

#### `ghostty-web@0.4.0`（共有 WASM インスタンス）

| 端末数 | 生成時間 | 10k 行解析 | スループット | アイドル RSS 増 | 負荷後 RSS 増 | **破棄+GC 後 RSS 残** | 破棄時間 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.8 ms | 27.6 ms | 35.3 MB/s | +0.30 MB | +14.44 MB | +14.66 MB | 0.17 ms |
| 5 | 1.3 ms | 15.0 ms | 64.9 MB/s | +3.48 MB | +7.78 MB | **+8.35 MB** | 0.03 ms |
| 10 | 2.6 ms | 14.5 ms | 67.0 MB/s | +6.83 MB | +12.06 MB | **+12.71 MB** | 0.05 ms |

### 6.4 Ghostty / Herdr との参考比較（同一 macOS 15.5 環境での実測）

| アプリケーション | アプリバンドル容量 | 実測プロセス RSS（稼働時） | 備考 |
| --- | ---: | ---: | --- |
| **Ghostty 1.4.0** | 67 MB | ~56.4 MB | ネイティブ Zig + Metal |
| **Herdr 0.9.5** | 59 MB | ~63.1 MB + `herdr-core` ~12.6 MB | ネイティブ UI + `herdr-core` |
| **uiterm Frontend (VT 10 端末アイドル増分)** | — *(Tauri 全体はバックエンド側で計測)* | +9.77 MB（`@wterm/ghostty` 10 端末アイドル） | 破棄時にほぼ全量解放可能 |

> [!TIP]
> **メモリ特性の決定的な違い**：WebAssembly の線形メモリ（`WebAssembly.Memory`）は仕様上縮小できない。共有 WASM インスタンス方式（`@gespenst/core`, `ghostty-web`）はピーク消費が小さい反面、大量出力タブを閉じてもメモリが OS に返らない。一方、`@wterm/ghostty` は端末ごとに独立した WASM インスタンスを持つため、`dispose()` 後の GC で線形メモリごと OS へ解放される（10 端末負荷後 `+89.5 MB` → 破棄後 `+0.45 MB`）。長時間稼働しタブを開閉し続ける uiterm のユースケースでは、この特性が大きな利点となる。

---

## 7. 発見した問題と代替案

| # | 問題 | 影響 | 対策・代替案 |
| --- | --- | --- | --- |
| 1 | `@wterm/dom` が `onData` にユーザー入力と端末自動応答を混在させる | 二重 VT 環境で自動応答を `kind: "user"` として送ると PTY 誤動作の原因になる | 同期 `GhosttyCore` 使用時は `write()` 実行中フラグで分離可能（PoC 実装済み）。将来 Worker 化する場合は `core.getResponse()` ラッパーまたは上流 PR で `onResponse` を分離する。 |
| 2 | `@gespenst/core@0.2.0` の `handleKeyDown` が無条件に `e.preventDefault()` を呼ぶ | 日本語 IME 変換が阻害される恐れが高い | 現状は不採用。将来再評価する場合は上流修正を待つ必要がある。 |
| 3 | `ghostty-web@0.4.0` に隠し `<textarea>` がない | WebKit での日本語 IME 未確定文字列表示・候補位置が成立しない | 不採用。 |
| 4 | `@wterm/ghostty` の 10 端末×5,000 行負荷時ピーク RSS（+89.5 MB） | タブ数増加＋長大スクロールバックでメモリ消費が増える | `scrollbackLines` の上限管理（例：標準 2,000〜5,000 行）と、非表示タブの `setRenderingPaused(true)` を必須とする。 |
| 5 | `vite-plus@1.8.0` の `defineConfig` と外部 Vite プラグインの型定義不整合 | `tsc --noEmit` で型エラーが発生 | `vite.config.ts` で `plugins: [...] as PluginOption[]` とし、`src/vite-env.d.ts` で `vite-plus/client` を参照する。 |

---

## 8. 最終的な描画ライブラリの推奨と根拠

### 推奨：**`@wterm/dom` + `@wterm/ghostty`（`libghostty` WASM）**

**デフォルトレンダラは `renderer: "dom"` とし、高負荷端末向けの最適化オプションとして `renderer: "webgl"` を検証・検討する。**

#### 推奨根拠
1. **日本語対応（Japanese First）の設計適合性が最も高い**：
   - `keyCode === 229` ガード・カーソル追従可視 `<textarea>`・`compositionend` 重複排除を標準実装しており、Safari / WKWebView の既知の IME 仕様差異を明確に考慮している。
   - DOM テキスト描画により、日本語フォントフォールバック・全角文字の選択・コピーがブラウザネイティブ品質で動作する。
2. **軽量性・長期稼働時のメモリ解放に優れる**：
   - 端末ごとの独立 WASM インスタンスにより、タブ破棄後に WASM 線形メモリがほぼ完全に回収される（実測：10 端末負荷後 +89.5 MB → 破棄後 +0.45 MB）。
   - `setRenderingPaused(true)` を標準装備しており、非表示タブ・ワークスペースの描画負荷を直接抑止できる。
3. **`libghostty` による高い VT 互換性と解析性能**：
   - 10,000 行（約 1 MB）の ANSI + 日本語出力を約 10 ms（~95 MB/s）で解析でき、fish / Neovim / Codex 等のモダン CLI/TUI 要件（Kitty keyboard, OSC 133, 2026 synchronized output, soft-wrap reflow）を満たす。
4. **WebKit（WKWebView / WebKitGTK）との親和性**：
   - `OffscreenCanvas` / WebGPU / Local Font Access に依存せず、標準 DOM をベースに動作するため、macOS WKWebView と Linux WebKitGTK (Wayland) 双方での動作リスクが最も低い。

#### フォールバック（代替候補）
- もし Phase 0 後半の **WKWebView / WebKitGTK 実機 IME テスト** で `@wterm/dom` に回避不能な描画・IME 障害が見つかった場合は、**`@xterm/xterm@6.0.0`**（DOM/Canvas レンダラ）を第一代替候補とする。ただしその場合は VT エンジンが `libghostty` ではなくなり、非同期 `write()` に伴う自動応答分離の追加設計が必要になる。
- また、`@gespenst/core` は `source: 'reply'` 分離と Worker 性能が非常に優秀なため、上流で IME（`handleKeyDown` の `preventDefault`）と WebKit 互換性が修正されれば将来の再評価候補として有力である。

---

## 9. 成果物一覧

- フロントエンド PoC エントリ・UI：[src/App.tsx](../../src/App.tsx), [src/components/TerminalPane.tsx](../../src/components/TerminalPane.tsx)
- 描画ライブラリアダプター：[src/terminal/types.ts](../../src/terminal/types.ts), [src/terminal/wtermAdapter.ts](../../src/terminal/wtermAdapter.ts), [src/terminal/gespenstAdapter.ts](../../src/terminal/gespenstAdapter.ts), [src/terminal/ghosttyWebAdapter.ts](../../src/terminal/ghosttyWebAdapter.ts), [src/terminal/xtermAdapter.ts](../../src/terminal/xtermAdapter.ts), [src/terminal/createAdapter.ts](../../src/terminal/createAdapter.ts)
- セッション管理（`UitermBackendClient` 接続・非 React バイナリパイプ）：[src/terminal/sessionManager.ts](../../src/terminal/sessionManager.ts)
- 状態管理（Zustand Vanilla）：[src/store/pocStore.ts](../../src/store/pocStore.ts)
- 国際化辞書（TypeScript 辞書、`i18next` 不使用）：[src/i18n/dict.ts](../../src/i18n/dict.ts)
- スキーマ検証（Valibot）・テストペイロード：[src/transport/schemas.ts](../../src/transport/schemas.ts), [src/transport/mockHerdrTransport.ts](../../src/transport/mockHerdrTransport.ts)
- 単体テスト・ヘッドレスベンチマーク：[src/terminal/__tests__/poc.test.ts](../../src/terminal/__tests__/poc.test.ts), [scripts/bench-terminals.mjs](../../scripts/bench-terminals.mjs)
