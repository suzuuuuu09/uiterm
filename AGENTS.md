# AGENTS.md

## 作業前に読む文書

- [開発状況](docs/status.md)
- [アーキテクチャ](docs/architecture.md)
- [開発手順](docs/development.md)
- [通信・操作契約](docs/interfaces.md)
- [ADR一覧](docs/adr/README.md)

## 実装上の制約

- CLI First, Not CLI Only。独自AIハーネスを実装せず、既存CLIを利用する。
- 製品GUIと端末描画はRust・GPUI、VT処理はネイティブ版libghostty-vtへ移行する。TypeScriptと外部Node.jsは設定・拡張に使う。
- 共通Action APIを維持し、端末出力とVT状態はアプリの状態管理から分離する。
- herdr-coreをセッション状態の正とし、ユーザー入力と自動応答を混同しない。
- 他のインスタンスを停止したり、既定のHerdrソケットを利用したりしない。
- 未信頼設定を自動実行しない。端末入力、IME文字列、認証情報をログに保存しない。
- 日本語IME、Nerd Font、Unicode幅、macOS・ネイティブWayland対応を退行させない。
- pnpm・Cargo・Nixを使用し、固定版とロックファイルの整合を維持する。
- 実装変更に対応するテストを追加し、検証済み・未検証を区別する。

## 開発の進め方

[ADR-0010](docs/adr/0010-evaluate-native-gpui-terminal.md) に基づき、Phase 2でGPUIを製品実装へ移行して検証する。Phase 3はPhase 2完了まで保留する。mainのコードと既存の実装レポートを参照し、ローカルの検証結果をマージ済みと扱わない。旧GUIの整理は [ロードマップ](docs/plan/roadmap.md) に従う。

重要な設計変更はADRに記録する。検証手順は [docs/development.md](docs/development.md) を参照。
