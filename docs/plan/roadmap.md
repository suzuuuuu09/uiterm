---
description: "uitermの開発ロードマップをまとめる"
---

## Phase 0 — 技術検証（完了）

採用予定の技術が要件を満たすか検証する。

- [x] Tauri・React・Vite+の最小構成（Tauri 2.12.7・React 19.3.1 + React Compiler・Vite+ 1.8.0・pnpm 12.9.0、macOS `.app` 4.22 MiB ビルド確認済み）
- [x] Nix Flakesによる開発環境の構築（macOS arm64 実測済み・`x86_64-linux`/`aarch64-linux` 評価確認済み、`herdr-core` v0.1.22 固定）
- [x] herdr-coreとの通信（Unix Socket・Framed Mode・JSON-RPC・Attach・Events・OSC 7501・`AUTOMATIC_INPUT` 分離・フロー制御・`attachId=0` 防御）
- [x] wtermによるターミナル描画（`@wterm/dom` + `@wterm/ghostty` へ単一化完了、Tauri 実ウィンドウ上で `herdr-core` と結合起動確認済み）
- [x] ターミナルの入出力・リサイズ（`stty size` 反映・バックエンド起点リサイズループ防止・再アタッチ再同期・`fish`/`nvim`/`codex` 互換性確認済み）
- [x] VT エンジンの二重動作による負荷測定（実ウィンドウ 1 端末アイドル・5 MiB×3 バースト・60 秒連続出力 soak、およびバックエンド+`fish` / フロントエンド単体の 1・5・10 端末スケーリングを実測。30 分 soak・UI 5/10 タブ自動計測は Phase 1〜6 で継続検証）
- [x] 統合後の macOS 実ウィンドウで端末の入出力・`fish`/`nvim` 動作・GUI 正常終了時のリソース解放を確認する。NixOS / Hyprland は静的検証までを Phase 0 の対象とし、実機検証は MVP 完成条件として維持する。
- [x] 統合後の macOS WKWebView で日本語 IME を手動確認する（入力・変換・確定・確定時 Enter 非誤送信・Backspace 削除・変換候補位置を確認済み）。Linux Fcitx5 は MVP 完成までに実機で確認する。
- [x] TypeScript設定の実行方式の検証（外部 Node.js 24 LTS ネイティブ型除去 + JSON-RPC over stdio で実測済み）
- [x] 統合後の配布ビルドでメモリ使用量・ディスク容量・起動時間を基準値として計測する（`.app` 4.22 MiB + `herdr-core` 6.30 MiB = 10.51 MiB、1 端末アイドル Summary Footprint 120.48 MiB、ウォーム起動中央値 158.5 ms。真のログイン後コールド起動および Ghostty/Herdr 同一条件比較は Phase 6 で実施）。

検証結果と Phase 1 への申し送り事項は [docs/poc/integration.md](../poc/integration.md)（§7〜§9）・[docs/poc/optimization.md](../poc/optimization.md)・[docs/poc/validation.md](../poc/validation.md) を参照。Phase 0 を完了とし、Phase 1 へ移行する。NixOS / Hyprland の実機確認は [MVP の完成条件](./mvp.md#完成条件)（Phase 6）として維持する。

## Phase 1 — 基盤構築（完了）

アプリケーションの共通基盤を構築する。

- [x] ディレクトリ構成の設計（`src/{app,state,actions,ipc,terminal,validation,errors,logging,i18n,ui}` へ製品コードを整理し、Phase 0 診断資材を `src/poc/` へ隔離。Rust は `src-tauri/src/{app_state,bindings,commands,config_runtime,os,herdr/*}` へ責務分離し、`poc_bench` を `examples/` へ移設）
- [x] Tauri IPC（Raw Binary IPC・Tauri Commands / Channels・`terminal-ipc` 最小権限 Capabilities・ローカル `main` WebView 限定・CSP・遷移制限・入力検証）
- [x] herdr-coreアダプター（`herdr-core` v0.1.22 固定、`--version` および `hello.version` 検証、Framed Mode・クレジット制フロー制御・`AUTOMATIC_INPUT` 分離・再接続とスナップショット再同期）
- [x] プロセスのライフサイクル管理（専用 0700 ディレクトリ・専用ソケット・パイプ監視による監視プロセスで、通常終了・SIGTERM・SIGINT・SIGKILL 時の確実な終了と core クラッシュ復旧を macOS 実機検証済み）
- [x] 状態管理（React 非依存の Zustand Vanilla ストア `src/state/` で Window → Workspace → Tab → Pane・選択状態・`herdr-core` セッションミラー・OSC 7501 エージェント状態・UI/設定を管理。端末出力はストア非経由を厳守）
- [x] Action Registry（`src/actions/` にて `ActionCaller` / `ActionOwner`・Valibot 引数検証・`canExecute`・エラー正規化・i18n ラベルを備えた組み込みアクション 9 種を実装）
- [x] 型定義・データ検証（Rust 所有 DTO を `ts-rs` で `src-tauri/bindings/ipc.ts` へ自動生成し `cargo test` でドリフト検知。`src/ipc/types.ts` から生成型を再エクスポートし、`src/validation/` の Valibot スキーマで外部入力および `CoreEvent.data` を実行時検証）
- [x] エラーハンドリング（`AppError` でユーザー向け国際化文言と開発者向け診断情報を分離し、Rust 側の全主要 `IpcError` コードをマッピング、`runSafeAsync` で非同期例外を捕捉）
- [x] ログ管理（`src/logging/logger.ts` にてレベルフィルタ・リングバッファ上限・認証情報/端末入力/IME テキストの自動マスクを実装）
- [x] 国際化の基盤（TypeScript 辞書 `ja.ts` / `en.ts` のキー型強制・テンプレート補間・`Intl` API フォーマットラッパーを実装）
- [x] テスト環境（Vitest による単体・統合・Phase 0 回帰テスト、Rust 単体テスト・モック IPC 権限テスト・`herdr-core` 結合テスト・監視プロセス結合テスト、Clippy、Flake 静的評価を整備）

実装・検証の詳細は [docs/implementation/phase1-backend.md](../implementation/phase1-backend.md)・[docs/implementation/phase1-frontend.md](../implementation/phase1-frontend.md) を参照。macOS arm64 での統合レビュー・検証を完了し、Phase 2 へ移行可能。NixOS / Hyprland の実機検証（ビルド・GTK/WebKitGTK・Wayland・シグナル終了・Fcitx5）は引き続き保留とし、[MVP の完成条件](./mvp.md#完成条件)（Phase 6）として維持する。

## Phase 2 — ターミナル（未完了・一部実機検証待ち）

基本的なターミナルとして利用できる状態にする。

- [x] シェルの起動・終了（リリース .app、専用 core と fish の起動、SIGTERM 後のプロセス解放。再起動の競合と切断後の購読復旧を自動テスト）
- [x] ターミナルの描画・入力（wterm DOM / Ghostty、Action Registry、PoC 診断ドロワー。VT とキー入力を自動テスト）
- [x] スクロールバック（既定 2,000 行、ヘッドレスで 10,000 行出力と破棄を測定）
- [ ] コピー・ペースト（アクション・ブラケットペーストの生成・再起動との競合を自動テスト。macOS のネイティブクリップボードは手動検証待ち）
- [ ] リサイズ・フォーカス（PTY の初期行列数と IPC のフォーカス通知を確認。macOS のドラッグリサイズ、ズーム、Neovim の実フォーカスは手動検証待ち）
- [x] フォント・カラー・カーソルの内部設定（システムフォント、行間、ANSI パレット、形状・点滅。グリフとセルの目視確認は下記）
- [ ] Nerd Font・文字幅・日本語 IME（Ghostty の PUA・日本語・絵文字・結合文字のセルと IME イベント回帰は確認済み。ユーザーによる修正後 macOS リリースビルド実機検証で Nerd Font 表示および macSKK フォーカス復帰時の未確定文字表示は正常確認済み。文字幅 3 パターン、標準日本語入力、候補位置、確定 Enter、Option 競合は手動確認待ち）
- [ ] fish・Neovim・Codex の対話互換性（隔離 PTY の fish・Neovim と `codex --version` は確認済み。WKWebView での対話と実打鍵を確認する）

- [x] カノニカル入力での UTF-8 消去（uiterm 側 `--uiterm-pty-init` で macOS/Linux の新規 PTY に `IUTF8` を設定する暫定修正・回帰テスト・リリース `.app` 検証に加え、ユーザーによる実機検証でも文字化け修正を確認済み。上流修正後に撤去予定）
- [ ] macSKK・標準日本語入力の実打鍵確認（blur 処理修正・連続確定/未確定削除の回帰テスト追加済み。ユーザー実機検証で macSKK のフォーカス復帰時の未確定文字表示は正常確認済み。Esc 取消・確定フローおよび標準日本語入力の実打鍵確認待ち）
- [ ] macism による Neovim 起動遅延（自動比較および修正後ビルドのユーザー実機検証のいずれでも再現せず。ただし根本原因未特定のため恒久的な解決済みとは扱わず、既知の調査事項として維持し再発時の `sample` スタック採取待ち）

実機検証結果・残存チェックリスト・性能測定・IUTF8 の暫定対応・macism の調査事項は [Phase 2 実装・検証レポート](../implementation/phase2-terminal.md) を参照。一部実機検証が未完了のため Phase 2 は未完了扱いとし、Phase 3 へは移行しない。NixOS / Hyprland 実機と 30 分の連続利用時のメモリ測定は Phase 6 に残す。

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
