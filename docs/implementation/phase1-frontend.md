---
description: "uiterm Phase 1（基盤構築）TypeScript・アプリケーション基盤の実装記録と統合確認事項"
---

# Phase 1 フロントエンド・アプリケーション基盤 実装レポート

本ドキュメントは、uiterm Phase 1（基盤構築）における TypeScript・アプリケーション基盤の設計・実装内容、検証結果、および Rust バックエンド（Codex 担当 worktree）との統合時に必要な変更・確認事項をまとめたものである。

---

## 1. ディレクトリ構成と責務整理

Phase 0 の PoC 構成から製品実装へ移行するため、`src/` 配下を責務単位で整理し、PoC 専用の診断・ベンチマークコードは `src/poc/` へ明確に分離した。

```text
src/
├── main.tsx                        # エントリポイント
├── App.tsx                         # Phase 1 ワークベンチシェル（Action Registry 経由で操作）
├── index.css                       # Tailwind CSS v4 + wterm スタイル
│
├── app/
│   └── runtime.ts                  # コンポジションルート（store / logger / sessionManager / actions を結合）
│
├── state/                          # Zustand Vanilla 状態管理（React 非依存）
│   ├── types.ts                    # Window / Workspace / Tab / Pane / Selection / Session / Agent / UI / Settings 型
│   ├── coreSync.ts                 # herdr-core スナップショット・差分イベント・OSC 7501 同期ロジック
│   ├── appStore.ts                 # createAppStore() / appStore / useAppStore()
│   └── __tests__/appStore.test.ts
│
├── actions/                        # 共通 Action Registry（GUI / CLI / キーバインド / TS設定 / フック）
│   ├── types.ts                    # ActionId / BuiltinActionMap / ActionCaller / ActionOwner / ActionDefinition
│   ├── registry.ts                 # ActionRegistry（登録・実行・canExecute・Valibot 検証・i18n・エラー正規化）
│   ├── builtin.ts                  # 組み込みアクション（terminal.* / tab.* / app.*）
│   └── __tests__/registry.test.ts
│
├── ipc/                            # Tauri IPC クライアント・モック（Phase 0 契約を維持）
│   ├── generated/index.ts          # Codex 側 ts-rs 生成型の接続口（シーム）
│   ├── types.ts                    # 正準 IPC 型定義・バイナリフレームデコーダ・UitermBackendClient
│   ├── client.ts                   # TauriIpcClient + isTauriRuntime()
│   ├── mock.ts                     # MockIpcClient（テスト用イベント注入 emitTestEvent を追加）
│   └── __tests__/ipc.integration.test.ts
│
├── terminal/                       # ターミナル描画・セッション管理（製品コード）
│   ├── types.ts                    # TerminalAdapter 契約
│   ├── createAdapter.ts            # WTermGhosttyAdapter 生成
│   ├── wtermAdapter.ts             # @wterm/dom + @wterm/ghostty（core.dispose() 保証・scrollbackLines=2000）
│   ├── sessionManager.ts           # TerminalSessionManager（appStore / Logger / AppError 統合）
│   └── __tests__/
│       ├── poc.test.ts             # Phase 0 回帰テスト（11 件すべて保持）
│       └── unicode.test.mjs        # Phase 0 Unicode / IME 回帰テスト（2 件すべて保持）
│
├── validation/                     # Valibot による外部入力の実行時検証
│   ├── ipcSchemas.ts               # IPC 境界スキーマ + parseExternalInput()
│   ├── settingsSchemas.ts          # UserSettings / TerminalBackendId / Locale スキーマ
│   └── __tests__/validation.test.ts
│
├── errors/                         # 構造化エラー管理
│   ├── appError.ts                 # AppError / fromIpcError() / normalizeError() / runSafeAsync()
│   └── __tests__/appError.test.ts
│
├── logging/                        # レベル付き・上限付き・プライバシー保護ログ
│   ├── logger.ts                   # Logger / sanitizeLogData() / appLogger
│   └── __tests__/logger.test.ts
│
├── i18n/                           # TypeScript 辞書 + Intl API（i18next 非導入）
│   ├── types.ts                    # Locale / TranslationParams
│   ├── locales/ja.ts               # 日本語辞書（TranslationKey の正）
│   ├── locales/en.ts               # 英語辞書（Record<TranslationKey, string> で完全一致を型強制）
│   ├── format.ts                   # Intl API ラッパー（数値・バイト・日時・時刻・相対時間）
│   ├── index.ts                    # t() / translateError() / resolveSystemLocale()
│   └── __tests__/i18n.test.ts
│
├── ui/
│   └── TerminalViewport.tsx        # 製品用端末ホスト（IME 変換状態のみ管理、文字列は記録しない）
│
└── poc/                            # PoC 専用コード（製品状態から隔離）
    ├── pocStore.ts                 # ベンチマーク結果・オプトイン診断ログ（デフォルト OFF）
    ├── probes.ts                   # 日本語文字幅・VT/ANSI・自動応答テストペイロード
    ├── mockHerdrTransport.ts       # Phase 0 回帰テスト用モックトランスポート
    └── PocPanels.tsx               # 診断・ベンチマーク UI パネル
```

不要となった Phase 0 の暫定パス（`src/store/pocStore.ts`、`src/components/TerminalPane.tsx`、`src/i18n/dict.ts`、`src/transport/*`）は整理し、型の重複を排除した。

---

## 2. 状態管理（Zustand Vanilla）

- **React 非依存**: `createAppStore()` を `zustand/vanilla` で実装し、`ActionRegistry` や `TerminalSessionManager` から直接操作可能にした。React コンポーネントからは薄いフック `useAppStore(selector)` のみを利用する。
- **管理対象**:
  - `windows` / `workspaces` / `tabs` / `panes`: Window → Workspace → Tab → Pane の階層構造（レイアウト・表示状態は uiterm が所有）。
  - `selection`: `activeWindowId` / `activeWorkspaceId` / `activeTabId` / `activePaneId`。
  - `sessions` / `coreConnection`: `herdr-core` の実行状態ミラー（正は `herdr-core`）。`events_subscribe` の初期スナップショットおよび差分 `CoreEvent` を `src/state/coreSync.ts` で同期し、`seq <= lastSeq` の古いイベントを破棄、`missed` イベント時は `event.terminals` で全量再同期する（`docs/poc/interface.md` §6 準拠）。
  - `agentStates`: OSC 7501 の `status` / `turn` イベントを共通形式（`idle | working | waiting_input | waiting_approval | done | error`）へ正規化して保持。
  - `ui` / `settings`: サイドバー表示・IME 変換中フラグ・通知・ユーザー設定。
- **不変条件**: ターミナル出力バイト列・VT グリッド・スクロールバック履歴・描画バッファは Zustand に一切流さず、従来通り `TerminalSessionManager` → `TerminalAdapter.write(bytes)` へ直接渡す。

---

## 3. Action Registry

- **操作主体と所有者の明示**:
  - 実行主体 `ActionCaller`: `"gui" | "cli" | "keybind" | "config" | "hook" | "internal"`
  - 状態・副作用の所有者 `ActionOwner`: `"ui" | "state" | "terminal" | "app"`
- **型安全性と実行時検証**:
  - `BuiltinActionMap` により組み込みアクション ID ごとに引数型・戻り値型をコンパイル時に推論しつつ、Phase 4 のユーザー定義アクション向けに任意の文字列 ID も登録可能とした。
  - 全アクション定義に Valibot の `argsSchema` を必須とし、実行時に外部引数（CLI / 設定 / フック / GUI）を検証する。不正引数は `AppError(code: "ACTION_INVALID_ARGS")` で拒否する。
- **実行可否判定・エラー処理・国際化**:
  - `canExecute(id, args?, options?)` で事前判定と不可理由の翻訳キー（`reasonKey`）を返す。
  - 実行時の例外は `normalizeError()` で `AppError` へ正規化し、`ActionResult<T>` として返却しつつ構造化ログへ記録する。
  - 各アクションは `titleKey` / `descriptionKey` を持ち、`getLabel(id, locale)` で日本語・英語の名称を取得できる。
- **接続済み組み込みアクション（9 種）**:
  - `terminal.create` / `terminal.close` / `terminal.focus` / `terminal.resync` / `terminal.copySelection`
  - `tab.select`
  - `app.setLocale` / `app.setRenderer` / `app.toggleSidebar`

---

## 4. 型定義・データ検証と `ts-rs` 接続方針

- **型と実行時スキーマの役割分離**:
  - **TypeScript 型（`src/ipc/types.ts`、`src/state/types.ts`）**: コンパイル時の契約。IPC 契約は既存の `docs/poc/interface.md` を一切変更せず維持した。
  - **Valibot スキーマ（`src/validation/`）**: 外部境界（IPC パラメータ/エラー、Action 引数、ユーザー設定、リサイズ範囲）の実行時検証。`satisfies v.GenericSchema<...>` により `src/ipc/types.ts` との乖離をコンパイル時に検出する。
- **Rust 型生成（`ts-rs`）との接続方針**:
  - `src/ipc/generated/index.ts` を生成物の接続口として確保済み。
  - Codex 側で `src-tauri/src/herdr/types.rs` の各構造体へ `#[derive(TS)] #[ts(export, export_to = "../../src/ipc/generated/")]`（または同等設定）を導入した際、`src/ipc/types.ts` の手動型定義を `src/ipc/generated/index.ts` からの再エクスポートへ差し替えるだけで、フロントエンド他層のインポートパスを一切変更せず移行できる。

---

## 5. エラー・ログ管理

- **構造化エラー（`src/errors/appError.ts`）**:
  - ユーザー向け文言（`messageKey` + `messageParams` → `translateError(locale, err)`）と開発者向け診断情報（`code` / `ipcCode` / `hint` / `diagnostics` / `cause` → `toDiagnostics()` / `formatDeveloperSummary()`）を分離。
  - `IpcError` のコード（`unavailable`, `core_disconnected`, `not_found`, `invalid_attach`, `invalid_args`, `unsupported`, `timeout`, `io_error`）を `AppErrorCode` へ自動マッピング。
  - 非同期処理向けに `runSafeAsync()` を用意し、未処理 Promise rejection を防止。
- **プライバシー保護ログ（`src/logging/logger.ts`）**:
  - `debug | info | warn | error` のログレベルフィルタとリングバッファ上限（デフォルト 200 件・設定変更可）を実装。
  - `sanitizeLogData()` が認証情報系キー（`password`, `token`, `secret`, `apiKey`, `authorization`, `cookie`, `credential`, `env`）、端末入力系キー（`input`, `terminalInput`, `rawBytes`）、IME テキスト系キー（`imeData`, `imeText`, `composedText`）および `Uint8Array` / `ArrayBuffer` を自動的に `[REDACTED]` / `[binary NB redacted]` へマスクし、文字列長も 160 文字で打ち切る。
  - `TerminalViewport.tsx` でも IME の `e.data`（未確定・確定文字列）はログに記録しない設計へ改めた。

---

## 6. 国際化（TypeScript 辞書 + Intl API）

- `i18next` は導入せず、`src/i18n/locales/ja.ts` と `src/i18n/locales/en.ts` の TypeScript 辞書で `TranslationKey` の完全一致を型レベルで強制。
- `{param}` 形式の軽量テンプレート補間に対応し、UI 文言・アクション名・エラーメッセージ・エージェント状態をすべて辞書管理。
- `src/i18n/format.ts` で標準 `Intl` API（`Intl.NumberFormat` / `Intl.DateTimeFormat` / `Intl.RelativeTimeFormat`）による数値・バイトサイズ・日時・時刻・相対時間のロケール書式化を提供。

---

## 7. テスト・ビルド検証結果

Vitest（Vite+ 1.8.0 / Vitest 5.1.1）で単体・統合・Phase 0 回帰テストを実行し、全項目通過を確認した。

| 項目                 | コマンド             | 結果                             |
| -------------------- | -------------------- | -------------------------------- |
| TypeScript 型検査    | `pnpm typecheck`     | **合格**（0 errors）             |
| Lint                 | `pnpm lint`          | **合格**（0 warnings, 0 errors） |
| テスト全体           | `pnpm test -- --run` | **合格**（9 files / 39 passed）  |
| フロントエンドビルド | `pnpm build`         | **合格**（0 warnings）           |

テスト内訳：

- 状態管理: `src/state/__tests__/appStore.test.ts`（7 件）
- Action Registry: `src/actions/__tests__/registry.test.ts`（3 件）
- データ検証: `src/validation/__tests__/validation.test.ts`（4 件）
- エラー処理: `src/errors/__tests__/appError.test.ts`（4 件）
- ログ管理: `src/logging/__tests__/logger.test.ts`（3 件）
- 国際化: `src/i18n/__tests__/i18n.test.ts`（4 件）
- モック IPC 統合: `src/ipc/__tests__/ipc.integration.test.ts`（1 件）
- Phase 0 回帰（IPC ガード・最適化）: `src/terminal/__tests__/poc.test.ts`（11 件）
- Phase 0 回帰（Unicode・IME）: `src/terminal/__tests__/unicode.test.mjs`（2 件）

---

## 8. 統合時に必要な変更・確認事項（Codex 側 worktree との整合）

1. **`ts-rs` 出力先と型マッピングの合意**
   - 出力ディレクトリ案: `src/ipc/generated/`（`src/ipc/generated/index.ts` をバレルエクスポートとして維持）。
   - `u64`（`uptimeMs`, `createdMs`, `finishedMs`, `statusMs`, `turn`, `seq`, `live`, `finished`）について、`ts-rs` デフォルトの `bigint` ではなく `#[ts(type = "number")]` とすることで既存の JSON シリアライズ（JavaScript `number`）および `src/ipc/types.ts` と完全互換になる。
   - `CoreEvent` の各バリアントで `data: serde_json::Value` となっている箇所（`Lost`, `Finished`, `Status`, `Turn`, `Title`, `Size`, `Labels`）を Rust 側でも型付き構造体へ昇格させると、`ts-rs` でそのままタグ付きユニオンとして型生成でき、フロントエンド側の手動定義を完全に廃止できる。
2. **IPC エラーコードの列挙整合**
   - フロントエンド `src/errors/appError.ts` の `mapIpcCode()` は現在 `unavailable`, `connect_failed`, `herdr_core_not_found`, `core_start_failed`, `core_version_mismatch`, `invalid_core_path`, `spawn_failed`, `core_disconnected`, `not_found`, `invalid_attach`, `invalid_args`, `unsupported`, `untrusted_config`, `timeout`, `core_timeout`, `core_start_timeout`, `config_timeout`, `io_error` を個別マッピングしている（それ以外は `IPC_ERROR` へフォールバック）。Rust 側で新しい `IpcError.code` を追加した場合は、`mapIpcCode()` および `src/i18n/locales/{ja,en}.ts` へ対応を追加する。
3. **IPC 契約の変更有無**
   - 本作業では既存の Tauri Command 名・引数構造・バイナリフレーム形式（`0x00`/`0x01`/`0x02`）・Raw Body ヘッダ（`x-uiterm-attach-id`, `x-uiterm-input-kind`）を一切変更していないため、Codex 側の既存コマンドとそのまま結合可能である。
4. **統合後の確認項目**
   - `pnpm tauri dev` / リリースビルドにて、Tauri 実ウィンドウ上で `ActionRegistry` 経由のタブ作成・切替・再同期・クローズ、`events_subscribe` → `appStore.sessions` / `agentStates` の実同期、および `vite.config.ts` のチャンク分割（`vendor-react` / `vendor-terminal`）が WKWebView / CSP 下で正常ロードされることを確認する。
