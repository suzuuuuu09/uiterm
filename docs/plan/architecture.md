---
description: "uitermのアーキテクチャをまとめる"
---

## 技術スタック

| 分類 | 用途 | 技術 |
|---|---|---|
| Core | メイン言語 | TypeScript |
| Core | ネイティブ処理 | Rust |
| Core | デスクトップ基盤 | Tauri 2 |
| Frontend | UI | React |
| Frontend | 最適化 | React Compiler |
| Frontend | CSS | Tailwind CSS v4 |
| Frontend | UIプリミティブ | Base UI |
| Frontend | 状態管理 | Zustand Vanilla |
| Frontend | 国際化 | TypeScript 辞書 + Intl API |
| Frontend | アイコン | Lucide |
| Frontend | スキーマ検証 | Valibot |
| Terminal | セッション管理 | herdr-core |
| Terminal | VTエンジン | libghostty-vt |
| Terminal | 描画 | wterm |
| Backend | 非同期処理 | Tokio |
| Backend | シリアライズ | Serde |
| Backend | 型生成 | ts-rs |
| CLI | 引数解析 | clap |
| Config | 設定言語 | TypeScript |
| Config | 実行環境 | Node.js LTS |
| Storage | データ永続化 | JSON |
| Development | 開発ツールチェーン | Vite+ |
| Development | パッケージ管理 | pnpm / Cargo |
| Development | 開発環境 | Nix Flakes |
| Testing | 単体テスト | Vitest / Cargo Test |
| Testing | E2E | Playwright / WebdriverIO |
| Distribution | Rustビルド | crane |
| Distribution | CI/CD | GitHub Actions |
| Distribution | 依存関係更新 | Renovate |

## アーキテクチャ方針

- TypeScriptを主要な開発言語とする。
- RustはOS連携やプロセス管理、IPCなどに利用する。
- UIとアプリケーションのロジックを分離する。
- 外部ライブラリへの依存をアダプターに集約する。
- GUI・CLI・設定ファイルから共通の操作APIを利用する。
- 将来的な拡張を考慮しつつ、不要な抽象化を避ける。

## ターミナル基盤

herdr-coreをPTY・プロセス・セッション管理の基盤として利用する。

- Unix Domain Socketを介して通信する。
- 公開されているJSON-RPC・バイナリプロトコルを利用する。
- herdr-coreへの依存を専用のアダプターに集約する。
- 使用するバージョンを固定する。
- セッションの実行状態はherdr-coreを正とする。
- GUIのレイアウトや表示状態はuiterm側で管理する。

## ターミナル描画

wtermを第一候補とし、libghosttyを利用するWASMバックエンドを採用する。

- ターミナル描画とセッション管理を分離する。
- ターミナル出力をReactやZustandの状態管理に流さない。
- 不要な再描画を抑える。
- 非表示のターミナルによるリソース消費を抑える。
- 日本語IMEや文字幅の正確性を重視する。

wtermに重大な問題がある場合は、gespenst・xterm.jsなどを代替候補とする。

## UI

Reactを利用してアプリケーションのUIを構築し、Window → Workspace → Tab の階層構造を採用する。

- Window：アプリケーションウィンドウ
- Workspace：リポジトリ・作業ディレクトリ
- Tab：用途に応じた作業領域

UIプリミティブにはBase UIを利用し、外観はTailwind CSSで構築する。特定のコンポーネントライブラリのデザインに依存せず、uiterm独自のUIを作る。

## 状態管理

Zustand Vanillaで以下の状態を管理する。

- ワークスペース
- タブ・ペイン
- 選択状態
- エージェントの状態
- UIの表示状態
- ユーザー設定

Reactに依存しない状態管理を行い、共通操作APIからも利用できるようにする。ターミナルの出力やVT状態は専用の描画ライブラリで管理する。

## 操作API

Action Registryを実装し、アプリケーション内の操作を共通化する。以下のインターフェースから共通のアクションを呼び出す。

- GUI
- CLI
- キーバインド
- TypeScript設定
- イベントフック

アクションには型付きの引数と戻り値を定義する。アプリケーションの機能を個別のUIコンポーネントに閉じ込めず、再利用可能な操作として提供する。

## TypeScript設定

TypeScriptによるプログラマブルな設定を提供する。設定の実行には外部のNode.jsを利用し、アプリケーションには同梱しない。

- 型安全な設定API
- ヘルパー関数
- ユーザー定義関数
- 独自アクション
- イベントフック
- サイドバーのカスタマイズ

設定ファイルの実行環境とアプリケーションは分離し、JSON-RPCを介して通信する。設定ファイルに記述されたコードはユーザー権限で実行されるため、信頼できない設定ファイルを自動実行しない。設定の読み込みに失敗した場合も、アプリケーションを利用できるようにする。

## プロセス間通信

| 通信経路 | 方式 |
|---|---|
| Frontend → Tauri | Tauri Commands |
| Tauri → Frontend | Tauri Channels |
| Tauri ↔ herdr-core | Unix Socket / JSON-RPC / Binary Stream |
| CLI ↔ uiterm | Unix Domain Socket |
| 設定ランタイム ↔ uiterm | JSON-RPC over stdio |

ターミナル出力はストリーミング方式で処理し、通信の滞留や過剰なバッファリングを防止する。IPCへのアクセスは原則として同一ユーザーに限定する。

## AIエージェント連携

herdr-core・OSC 7501・エージェント固有のHooksを利用する。

- 既存のCLIベースのエージェントを利用する。
- エージェントの状態取得と実行処理を分離する。
- 状態を共通の形式に変換する。
- サイドバーなどから状態を確認できるようにする。
- 特定のAIハーネスへの依存を避ける。

独自のAIハーネスは実装しない。将来的なAgentタブの実装ではACPや既存の公式APIを検討する。

## 国際化

表示文言はキーを型で管理する TypeScript 辞書にまとめる。日付、数値、相対時間の書式化には選択中のロケールを渡して標準の Intl API を使う。初期対応言語は日本語・英語とする。

- メニュー
- コンテキストメニュー
- サイドバー
- 設定画面
- 通知
- エラーメッセージ
- アクション名

UIに表示する文字列を直接ハードコードせず、翻訳リソースとして管理する。

## データ永続化

MVPではJSONによるファイル保存を採用する。

- ワークスペース構成
- タブ・ペイン構成
- ウィンドウ状態
- アプリケーションの内部設定

保存データにはスキーマバージョンを持たせる。ファイルの破損を防止するため、アトミックな書き込みを行う。ユーザー設定とアプリケーションが生成する状態データは分離する。

## 軽量性

- Chromiumを同梱しない。
- 不要なランタイムを同梱しない。
- ターミナルの出力履歴や通信バッファに上限を設ける。
- 非表示のUIによる不要な処理を抑える。
- 使用していない機能を常時実行しない。
- メモリ使用量・CPU使用率・ディスク容量・起動時間を計測する。
- アプリケーション本体だけでなく、関連プロセスや依存関係を含めて評価する。

## セキュリティ

- Tauriの権限を必要最小限にする。
- IPCへのアクセスを制限する。
- 外部から受け取るデータを検証する。
- 信頼できないコードを自動実行しない。
- エージェントの既存設定を無断で変更しない。
- 将来的に外部Webページを表示する場合は、アプリケーションの特権APIから分離する。

## 開発環境

Nix Flakesを利用し、macOS・NixOS間で再現可能な開発環境を構築する。パッケージ管理にはpnpmとCargoを利用する。

JavaScript・TypeScriptの開発ツールチェーンにはVite+を利用する。

- Vite：開発サーバー
- Rolldown：バンドル
- Vitest：テスト
- Oxlint：Lint
- Oxfmt：フォーマット
- tsdown：ライブラリのビルド（必要に応じて利用）

Rustにはrustfmt・Clippyを利用する。

## テスト

- TypeScript・Rustの単体テスト
- IPCの統合テスト
- herdr-coreとの通信互換性テスト
- GUIのE2Eテスト
- 日本語IMEの実機テスト
- CLI・TUIの互換性テスト
- メモリ使用量・描画性能の測定

## 対応プラットフォーム

### macOS

- Apple Silicon
- WKWebView
- macOS標準の日本語IME

### Linux

- NixOS
- Hyprland
- ネイティブWayland
- WebKitGTK
- Fcitx5 + Mozc

## 技術検証

以下の項目は本格的な実装に進む前に検証する。

- herdr-coreとwtermの通信互換性
- VTエンジンの二重動作によるリソース消費
- ターミナルの自動応答とユーザー入力の分離
- 日本語IME・全角文字の処理
- 複数ターミナルの描画性能
- TypeScript設定ランタイムの実行コスト
- NixOSにおけるビルドと動作
- Vite+とTauri・Nixの連携

## 将来的な検討事項

- 内蔵ブラウザ
- Agent専用タブ
- セッション永続化・再接続
- リモート操作
- プラグインシステム
- 設定ランタイムの組み込み
