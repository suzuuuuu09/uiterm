---
description: "uitermの開発ロードマップをまとめる"
---

## Phase 0 — 技術検証

採用予定の技術が要件を満たすか検証する。

- [x] Tauri・React・Vite+の最小構成（Tauri 2.12.7・React 19.3.1 + React Compiler・Vite+ 1.8.0・pnpm 12.9.0、macOS `.app` バンドルビルド確認済み）
- [x] Nix Flakesによる開発環境の構築（macOS arm64 実測済み・`x86_64-linux` 評価確認済み、`herdr-core` v0.1.22 固定）
- [x] herdr-coreとの通信（Unix Socket・Framed Mode・JSON-RPC・Attach・Events・OSC 7501・`AUTOMATIC_INPUT` 分離・フロー制御・`attachId=0` 防御）
- [x] wtermによるターミナル描画（`@wterm/dom` + `@wterm/ghostty` 実装、Tauri 実ウィンドウ上で `herdr-core` と結合起動確認済み）
- [x] ターミナルの入出力・リサイズ（`stty size` 反映・バックエンド起点リサイズループ防止・再アタッチ再同期・`fish`/`nvim`/`codex` 互換性確認済み）
- [x] VTエンジンの二重動作による負荷の測定（Tauri 実ウィンドウ全プロセス RSS 237.75 MiB / CPU 0.1%、1〜10 端末スケーリング、20 秒連続出力でのメモリ安定性を実測済み）
- [ ] macOS・NixOS / Hyprlandでの動作確認（macOS 実ウィンドウ起動・結合は確認済み。NixOS / Hyprland 実機は未検証・再現手順を [docs/poc/integration.md](../poc/integration.md) に整備）
- [ ] 日本語IMEの動作確認（`wterm` ソース解析・診断 PoC・実ウィンドウ起動環境は整備済み。macOS WKWebView / Linux Fcitx5 での手動対話テストは未完了）
- [x] TypeScript設定の実行方式の検証（外部 Node.js 24 LTS ネイティブ型除去 + JSON-RPC over stdio で実測済み）
- [x] メモリ使用量・ディスク容量・起動時間の測定（配布 `.app` 13.73 MiB、ウォーム起動 534.1 ms、Ghostty / Herdr との比較を含め [docs/poc/integration.md](../poc/integration.md) に記録）

検証結果の詳細は [docs/poc/interface.md](../poc/interface.md)・[docs/poc/backend.md](../poc/backend.md)・[docs/poc/frontend.md](../poc/frontend.md)・[docs/poc/integration.md](../poc/integration.md) を参照。残る手動実機確認（macOS WKWebView 日本語 IME・NixOS / Hyprland）を完了して Phase 1 へ移行する。

## Phase 1 — 基盤構築

アプリケーションの共通基盤を構築する。

- [ ] ディレクトリ構成の設計
- [ ] Tauri IPC
- [ ] herdr-coreアダプター
- [ ] プロセスのライフサイクル管理
- [ ] 状態管理
- [ ] Action Registry
- [ ] 型定義・データ検証
- [ ] エラーハンドリング
- [ ] ログ管理
- [ ] 国際化の基盤
- [ ] テスト環境

## Phase 2 — ターミナル

基本的なターミナルとして利用できる状態にする。

- [ ] シェルの起動・終了
- [ ] ターミナルの描画・入力
- [ ] スクロールバック
- [ ] コピー・ペースト
- [ ] リサイズ
- [ ] フォント・カラー・カーソル
- [ ] 日本語IME・文字幅・フォントフォールバック
- [ ] fish・Neovim・Codexとの互換性確認

## Phase 3 — ワークスペース

複数のプロジェクトを並行して管理できる環境を構築する。

- [ ] Window → Workspace → Tab の階層構造
- [ ] ワークスペースの作成・切り替え・削除
- [ ] サイドバー
- [ ] タブの作成・切り替え・削除・並べ替え
- [ ] ペイン分割・移動
- [ ] Gitブランチの表示
- [ ] ワークスペース切り替え時のプロセス維持
- [ ] レイアウトの保存・復元
- [ ] コンテキストメニュー
- [ ] キーボード操作

## Phase 4 — カスタマイズ

TypeScriptによる設定・拡張を実現する。

- [ ] TypeScript設定ファイルの読み込み
- [ ] 設定API・型定義
- [ ] テーマ・フォントの設定
- [ ] キーバインドの設定
- [ ] ヘルパー関数の提供
- [ ] ユーザー定義関数の実行
- [ ] 独自アクションの登録
- [ ] サイドバーのカスタマイズ
- [ ] CLIによるアプリケーション操作
- [ ] 設定エラーからの復旧

## Phase 5 — AIエージェント管理

既存のAIエージェントCLIを効率的に管理できるようにする。

- [ ] エージェント状態の取得
- [ ] 状態の共通化
- [ ] Codexとの連携
- [ ] サイドバーへの状態表示
- [ ] エージェントへの素早い移動
- [ ] 入力待ち・承認待ちの表示
- [ ] 複数エージェントの並行管理

## Phase 6 — MVP完成

日常的に利用できる品質まで改善する。

- [ ] 日本語・英語のUI表示
- [ ] macOSでの動作確認
- [ ] NixOS / Hyprlandでの動作確認
- [ ] 日本語入力の実機テスト
- [ ] メモリ・CPU使用量の最適化
- [ ] ディスク容量の最適化
- [ ] 起動時間の最適化
- [ ] 長時間利用時の安定性確認
- [ ] Ghostty・Herdrとの性能比較
- [ ] ビルド・パッケージ化
- [ ] ドキュメントの整備
- [ ] 日常的な開発環境としての利用

## Post-MVP — 将来的な拡張

実際の利用経験に基づいて必要性を判断する。

- Agent専用タブ
- ACP・既存ハーネスとの連携
- 内蔵ブラウザ
- セッション永続化・再接続
- リモート操作
- イベントフックの拡充
- プラグインシステム
- Git worktree管理
- 設定のホットリロード
- 自動アップデート

## 開発方針

- 各Phaseの完了条件を満たしてから次のPhaseへ進む。
- 技術選定に問題がある場合は早期に見直す。
- 日常的に利用しながら問題を発見・改善する。
- 将来的な機能のために現段階の実装を複雑化しない。
- 開発状況に応じてロードマップを更新する。

